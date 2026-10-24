// SPDX-License-Identifier: CC0-1.0

//! Enum membership on the enum-targeting request members of the service's OTHER
//! implemented operations, end to end over HTTP.
//!
//! The rule is the service-wide one `enum_membership.rs` pins on the two start operations:
//! a request member whose target shape is a Smithy enumeration accepts only the values
//! that enumeration declares — including a member nested inside a list element or inside a
//! union variant — and a value it does not declare is refused with `ValidationException`
//! (HTTP 400) and the service's one `Invalid request body` sentence. The Smithy `enum`
//! trait's value list is not a constraint the boundary validator enforces, so each handler
//! is what holds a value to the catalog.
//!
//! This suite is the REST of that class: every enum-targeting member of `ListServers`,
//! `StartRecommendationReportGeneration`, `PutPortfolioPreferences` and
//! `UpdateServerConfig`, so the rule is asserted over the whole service rather than over
//! the operations one issue happened to cite. Each member is a row, probed with EVERY
//! value its own enumeration declares (accepted) and with a family of values it does not
//! (refused), and a third table pins that an ABSENT enum-targeting member is not refused:
//! an enumeration constrains the values a member may carry, not whether it is carried.
//!
//! Every catalog below is spelled out from `smithy_query`, independently of the crate's
//! own `*_VALUES` constants, so a wrong catalog in `src/ops/types/` fails here. Every row
//! goes through the router, so each case exercises routing, the boundary validator, the
//! handler and error serialization — the same path a client takes.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::enum_support::undeclared_values;
use crate::import_and_assess::call;
use crate::server_inventory::{emulator_with, server};

/// This service's one sentence for a value that fails a rule it enforces, spelled out
/// here rather than imported from the crate so a wrong message fails this suite instead of
/// agreeing with itself.
const INVALID_REQUEST_BODY: &str = "Invalid request body";

/// Every value `SortOrder` declares, from `smithy_query(query="shape",shape="SortOrder")`.
const SORT_ORDER: &[&str] = &["ASC", "DESC"];

/// Every value `ServerCriteria` declares, from
/// `smithy_query(query="shape",shape="ServerCriteria")`.
const SERVER_CRITERIA: &[&str] = &[
    "ANALYSIS_STATUS",
    "DESTINATION",
    "ERROR_CATEGORY",
    "NOT_DEFINED",
    "OS_NAME",
    "SERVER_ID",
    "STRATEGY",
];

/// Every value `GroupName` declares, from `smithy_query(query="shape",shape="GroupName")`.
const GROUP_NAME: &[&str] = &["ExternalId", "ExternalSourceType"];

/// Every value `OutputFormat` declares, from
/// `smithy_query(query="shape",shape="OutputFormat")`.
const OUTPUT_FORMAT: &[&str] = &["Excel", "Json"];

/// Every value `ApplicationMode` declares, from
/// `smithy_query(query="shape",shape="ApplicationMode")`.
const APPLICATION_MODE: &[&str] = &["ALL", "KNOWN", "UNKNOWN"];

/// Every value `DatabaseManagementPreference` declares, from
/// `smithy_query(query="shape",shape="DatabaseManagementPreference")`.
const DATABASE_MANAGEMENT_PREFERENCE: &[&str] = &["AWS-managed", "No preference", "Self-manage"];

/// Every value `AwsManagedTargetDestination` declares, from
/// `smithy_query(query="shape",shape="AwsManagedTargetDestination")` — the enumeration the
/// `awsManagedResources` union variant's own `targetDestination` list targets, which is
/// how deep this service binds an enum.
const AWS_MANAGED_TARGET_DESTINATION: &[&str] =
    &["AWS Elastic BeanStalk", "AWS Fargate", "None specified"];

/// Every value `Strategy` declares, from `smithy_query(query="shape",shape="Strategy")`.
const STRATEGY: &[&str] = &[
    "Refactor",
    "Rehost",
    "Relocate",
    "Replatform",
    "Repurchase",
    "Retain",
    "Retirement",
];

/// Every value `TargetDestination` declares, from
/// `smithy_query(query="shape",shape="TargetDestination")`.
const TARGET_DESTINATION: &[&str] = &[
    "AWS Elastic BeanStalk",
    "AWS Fargate",
    "Amazon DocumentDB",
    "Amazon DynamoDB",
    "Amazon Elastic Cloud Compute (EC2)",
    "Amazon Elastic Container Service (ECS)",
    "Amazon Elastic Kubernetes Service (EKS)",
    "Amazon Relational Database Service",
    "Amazon Relational Database Service on MySQL",
    "Amazon Relational Database Service on PostgreSQL",
    "Aurora MySQL",
    "Aurora PostgreSQL",
    "Babelfish for Aurora PostgreSQL",
    "None specified",
];

/// Every value `TransformationToolName` declares, from
/// `smithy_query(query="shape",shape="TransformationToolName")`.
const TRANSFORMATION_TOOL_NAME: &[&str] = &[
    "App2Container",
    "Application Migration Service",
    "Database Migration Service",
    "End of Support Migration",
    "In Place Operating System Upgrade",
    "Native SQL Server Backup/Restore",
    "Porting Assistant For .NET",
    "Schema Conversion Tool",
    "Strategy Recommendation Support",
    "Windows Web Application Migration Assistant",
];

/// The enumerations this suite probes, so a row can also be given a value a SIBLING
/// enumeration of this service declares — a legal value of the service, just not of this
/// member.
const CATALOGS: &[&[&str]] = &[
    APPLICATION_MODE,
    AWS_MANAGED_TARGET_DESTINATION,
    DATABASE_MANAGEMENT_PREFERENCE,
    GROUP_NAME,
    OUTPUT_FORMAT,
    SERVER_CRITERIA,
    SORT_ORDER,
    STRATEGY,
    TARGET_DESTINATION,
    TRANSFORMATION_TOOL_NAME,
];

/// The id of the one server every `UpdateServerConfig` row runs against: a `ResourceId`
/// (`^[0-9a-b]+`) short enough to also be a `ServerId`.
const SEEDED_SERVER: &str = "0a";

/// One enum-targeting request member of an implemented operation.
struct EnumMember {
    /// `<Operation>$<member path>`, named in every assertion.
    member: &'static str,
    /// The route the operation is bound to.
    uri: &'static str,
    /// A request carrying the given value in that member, every other member legal.
    request: fn(&str) -> Value,
    /// Every value the member's target enumeration declares.
    declared: &'static [&'static str],
}

/// A `ListServers` request carrying `value` in the named member.
fn servers_with(value: &str, member: &str) -> Value {
    match member {
        // `filterValue` is always named, so no row rests on a criteria with nothing to
        // compare against.
        "serverCriteria" => json!({"serverCriteria": value, "filterValue": SEEDED_SERVER}),
        "sort" => json!({"sort": value}),
        "groupIdFilter" => json!({"groupIdFilter": [{"name": value, "value": "wave-1"}]}),
        other => panic!("this table has no ListServers member {other:?}"),
    }
}

/// An `UpdateServerConfig` request whose `strategyOption` carries `value` in the named
/// member, against the server this suite seeds.
fn update_with(value: &str, member: &str) -> Value {
    json!({
        "serverId": SEEDED_SERVER,
        "strategyOption": {member: value},
    })
}

/// Every enum-targeting request member of the implemented operations
/// `enum_membership.rs` leaves to their own suite, from
/// `smithy_query(query="operation",operation="ListServers,StartRecommendationReportGeneration,PutPortfolioPreferences,UpdateServerConfig")`
/// read against `smithy_query(query="shape",shape="Group,StrategyOption,DatabasePreferences,ManagementPreference,AwsManagedResources")`.
///
/// The members of operations that still answer 501 (`ListApplicationComponents`,
/// `ListAnalyzableServers`, `UpdateApplicationComponentConfig`,
/// `GetApplicationComponentStrategies`, `ListCollectors`, `GetPortfolioSummary`) belong to
/// their own stub issues; the other implemented operations take only ids and pagination,
/// so they carry no enum-targeting member.
fn enum_members() -> Vec<EnumMember> {
    vec![
        EnumMember {
            member: "ListServers$serverCriteria",
            uri: "/list-servers",
            request: |value| servers_with(value, "serverCriteria"),
            declared: SERVER_CRITERIA,
        },
        EnumMember {
            member: "ListServers$sort",
            uri: "/list-servers",
            request: |value| servers_with(value, "sort"),
            declared: SORT_ORDER,
        },
        EnumMember {
            member: "ListServers$groupIdFilter[0].name",
            uri: "/list-servers",
            request: |value| servers_with(value, "groupIdFilter"),
            declared: GROUP_NAME,
        },
        EnumMember {
            member: "StartRecommendationReportGeneration$outputFormat",
            uri: "/start-recommendation-report-generation",
            request: |value| json!({"outputFormat": value}),
            declared: OUTPUT_FORMAT,
        },
        EnumMember {
            member: "StartRecommendationReportGeneration$groupIdFilter[0].name",
            uri: "/start-recommendation-report-generation",
            request: |value| json!({"groupIdFilter": [{"name": value, "value": "wave-1"}]}),
            declared: GROUP_NAME,
        },
        EnumMember {
            member: "PutPortfolioPreferences$applicationMode",
            uri: "/put-portfolio-preferences",
            request: |value| json!({"applicationMode": value}),
            declared: APPLICATION_MODE,
        },
        EnumMember {
            member: "PutPortfolioPreferences$databasePreferences.databaseManagementPreference",
            uri: "/put-portfolio-preferences",
            request: |value| {
                json!({"databasePreferences": {"databaseManagementPreference": value}})
            },
            declared: DATABASE_MANAGEMENT_PREFERENCE,
        },
        EnumMember {
            // An enumeration nested in a union variant's own list: as deep as this
            // service binds one.
            member: "PutPortfolioPreferences$applicationPreferences.managementPreference\
                     .awsManagedResources.targetDestination[0]",
            uri: "/put-portfolio-preferences",
            request: |value| json!({
                "applicationPreferences": {
                    "managementPreference": {
                        "awsManagedResources": {"targetDestination": [value]}
                    }
                }
            }),
            declared: AWS_MANAGED_TARGET_DESTINATION,
        },
        EnumMember {
            member: "UpdateServerConfig$strategyOption.strategy",
            uri: "/update-server-config/",
            request: |value| update_with(value, "strategy"),
            declared: STRATEGY,
        },
        EnumMember {
            member: "UpdateServerConfig$strategyOption.targetDestination",
            uri: "/update-server-config/",
            request: |value| update_with(value, "targetDestination"),
            declared: TARGET_DESTINATION,
        },
        EnumMember {
            member: "UpdateServerConfig$strategyOption.toolName",
            uri: "/update-server-config/",
            request: |value| update_with(value, "toolName"),
            declared: TRANSFORMATION_TOOL_NAME,
        },
    ]
}

/// One emulator holding the server `UpdateServerConfig`'s rows name, so those rows turn on
/// the enum value and not on an unresolvable `serverId`. Every row here is a read or an
/// update of that one record, so they share it.
fn router_with_one_server() -> MultiServiceRouter {
    emulator_with(vec![server(SEEDED_SERVER)])
}

/// A value the member's own enumeration does not declare is refused with 400
/// `ValidationException` and the service's one sentence — on every enum-targeting member
/// of these four operations, nested members included.
#[tokio::test]
async fn an_undeclared_enum_value_is_refused_on_every_other_enum_member() {
    let router = router_with_one_server();

    for EnumMember {
        member,
        uri,
        request,
        declared,
    } in enum_members()
    {
        let rows = undeclared_values(declared, CATALOGS);
        assert!(
            !rows.is_empty(),
            "case {member:?}: the table must probe at least one undeclared value"
        );
        for (shape, value) in rows {
            let case = format!("{member} = {value:?} ({shape})");
            let answer = call(&router, "POST", uri, Some(request(&value))).await;
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected HTTP 400, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.string("/message"),
                INVALID_REQUEST_BODY,
                "case {case:?}: expected the service's {INVALID_REQUEST_BODY:?} sentence, got {:?}",
                answer.body
            );
        }
    }
}

/// EVERY value each enumeration declares is accepted on the same path, so the refusal
/// above is scoped to values the catalog really does not hold.
#[tokio::test]
async fn every_value_these_enumerations_declare_is_accepted() {
    let router = router_with_one_server();

    for EnumMember {
        member,
        uri,
        request,
        declared,
    } in enum_members()
    {
        for value in declared {
            let answer = call(&router, "POST", uri, Some(request(value))).await;
            assert_eq!(
                answer.status, 200,
                "case {member} = {value:?}: a declared value is accepted, got {} with {:?}",
                answer.status, answer.body
            );
        }
    }
}

/// A member the request leaves out is not refused: an enumeration constrains the values a
/// member may carry, not whether it is carried at all — including an empty list, which
/// carries no entry whose member could be outside a catalog.
#[tokio::test]
async fn an_absent_enum_targeting_member_is_not_refused() {
    let router = router_with_one_server();

    let absent: Vec<(&str, &str, Value)> = vec![
        ("ListServers with no enum member", "/list-servers", json!({})),
        (
            "ListServers with an empty groupIdFilter",
            "/list-servers",
            json!({"groupIdFilter": []}),
        ),
        (
            "StartRecommendationReportGeneration with no outputFormat",
            "/start-recommendation-report-generation",
            json!({}),
        ),
        (
            "StartRecommendationReportGeneration with an empty groupIdFilter",
            "/start-recommendation-report-generation",
            json!({"groupIdFilter": []}),
        ),
        (
            "PutPortfolioPreferences with no member at all",
            "/put-portfolio-preferences",
            json!({}),
        ),
        (
            "PutPortfolioPreferences with a preference naming no enum member",
            "/put-portfolio-preferences",
            json!({"prioritizeBusinessGoals": {"businessGoals": {"speedOfMigration": 1}}}),
        ),
        (
            "UpdateServerConfig with an option naming no enum member",
            "/update-server-config/",
            json!({"serverId": SEEDED_SERVER, "strategyOption": {"isPreferred": true}}),
        ),
        (
            "UpdateServerConfig with no strategyOption",
            "/update-server-config/",
            json!({"serverId": SEEDED_SERVER}),
        ),
    ];

    for (case, uri, request) in absent {
        let answer = call(&router, "POST", uri, Some(request)).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );
    }
}
