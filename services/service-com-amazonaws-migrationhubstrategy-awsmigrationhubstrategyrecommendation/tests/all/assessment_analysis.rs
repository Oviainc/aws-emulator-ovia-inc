// SPDX-License-Identifier: CC0-1.0

//! The ANALYSIS an assessment produces, end to end over HTTP.
//!
//! `StartAssessment` "starts the assessment of an on-premises environment", so an
//! assessment that reaches `AssessmentStatus` `COMPLETE` has analysed the portfolio it
//! covered. The rule under test is that one, for ANY portfolio rather than for one
//! request: at the instant the completion falls due the service records, on every resource
//! the assessment covered, the recommendation the account's SAVED preferences select —
//! the target destination the preference names, the strategy that destination implies and
//! the transformation tool AWS documents for that move — together with the instant it was
//! analysed, and every read of those resources reports it (`ListServers`,
//! `GetServerDetails`, `GetServerStrategies`, `GetApplicationComponentDetails`,
//! `GetApplicationComponentStrategies`). An assessment that does NOT complete analyses
//! nothing.
//!
//! The tables below therefore span the class the rule generalises over rather than the one
//! preference the issue cited:
//!
//! * every value of every destination enumeration a `ManagementPreference` variant
//!   declares — `AwsManagedTargetDestination`, `SelfManageTargetDestination` and
//!   `NoPreferenceTargetDestination` — plus the portfolio with no preferences saved at
//!   all;
//! * every value of `TargetDatabaseEngine` as the engine a database preference names,
//!   crossed with both `DatabaseManagementPreference` directions and with the engine the
//!   database runs today;
//! * every value of `ApplicationMode`, and both values of `InclusionStatus`, as the
//!   members that decide WHICH components the analysis covers.
//!
//! A server and an application component enter this service only through the account's
//! data being brought in (the import loop, filed separately), so the portfolio is seeded
//! through the shared engine — the same `servers` and `application_components` maps an
//! ingestion writes and these operations read — while the preferences, the assessment and
//! every assertion go through the router, exactly as a client reaches them.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::state::{
    ApplicationComponentRecord,
    ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState, ServerRecord,
    COMPLETION_SPAN,
};
use service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::import_and_assess::{
    at, call, get_assessment, minimal_assessment, start_assessment, stop_assessment, Answer,
    TEST_REGION,
};

/// `StrategyRecommendation`'s value for a strategy the service itself recommends, from
/// `smithy_query(query="shape",shape="StrategyRecommendation")`.
const RECOMMENDED: &str = "recommended";

/// The `AssessmentStatus` an assessment that analysed the portfolio reached.
const ASSESSMENT_COMPLETE: &str = "COMPLETE";

/// Every value `AwsManagedTargetDestination` declares, from
/// `smithy_query(query="shape",shape="AwsManagedTargetDestination")`.
const AWS_MANAGED_DESTINATIONS: &[&str] = &["None specified", "AWS Elastic BeanStalk", "AWS Fargate"];

/// Every value `SelfManageTargetDestination` declares, from
/// `smithy_query(query="shape",shape="SelfManageTargetDestination")`.
const SELF_MANAGE_DESTINATIONS: &[&str] = &[
    "None specified",
    "Amazon Elastic Cloud Compute (EC2)",
    "Amazon Elastic Container Service (ECS)",
    "Amazon Elastic Kubernetes Service (EKS)",
];

/// Every value `NoPreferenceTargetDestination` declares, from
/// `smithy_query(query="shape",shape="NoPreferenceTargetDestination")`.
const NO_PREFERENCE_DESTINATIONS: &[&str] = &[
    "None specified",
    "AWS Elastic BeanStalk",
    "AWS Fargate",
    "Amazon Elastic Cloud Compute (EC2)",
    "Amazon Elastic Container Service (ECS)",
    "Amazon Elastic Kubernetes Service (EKS)",
];

/// Every value `TargetDatabaseEngine` declares, from
/// `smithy_query(query="shape",shape="TargetDatabaseEngine")`.
const TARGET_DATABASE_ENGINES: &[&str] = &[
    "None specified",
    "Amazon Aurora",
    "AWS PostgreSQL",
    "MySQL",
    "Microsoft SQL Server",
    "Oracle Database",
    "MariaDB",
    "SAP",
    "Db2 LUW",
    "MongoDB",
];

/// Every value `ApplicationMode` declares, from
/// `smithy_query(query="shape",shape="ApplicationMode")`.
const APPLICATION_MODES: &[&str] = &["ALL", "KNOWN", "UNKNOWN"];

/// Every value `DatabaseManagementPreference` declares, from
/// `smithy_query(query="shape",shape="DatabaseManagementPreference")`.
const DATABASE_MANAGEMENT_PREFERENCES: &[&str] = &["AWS-managed", "Self-manage", "No preference"];

/// What one analysis must have recommended for one resource: the three members
/// `RecommendationSet` declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Expected {
    strategy: &'static str,
    destination: &'static str,
    tool: Option<&'static str>,
}

/// The recommendation an APPLICATION workload — a server, or a component that is not a
/// database — must carry when the saved management preference names `destination`, and
/// `windows` is whether that workload runs on Windows.
///
/// Spelled out here independently of the crate's own tables, so a wrong strategy or tool
/// in `src/state/analysis.rs` fails this suite instead of agreeing with itself. The
/// grounding of each row: Amazon EC2 runs the workload as it is, so the move is a rehost
/// and AWS Application Migration Service performs it; the managed container services put
/// it on another platform, so the move is a replatform and AWS App2Container containerises
/// it; AWS Elastic Beanstalk is a replatform too, and the catalog's tool for it — the
/// Windows Web Application Migration Assistant, documented for .NET web applications on
/// IIS — applies to a Windows workload and to no other, so nothing is named for the
/// others; and the enumerations' own "None specified" names no destination, so nothing is
/// moved anywhere.
fn expected_application(destination: &'static str, windows: bool) -> Expected {
    match destination {
        "None specified" => Expected {
            strategy: "Retain",
            destination: "None specified",
            tool: None,
        },
        "Amazon Elastic Cloud Compute (EC2)" => Expected {
            strategy: "Rehost",
            destination,
            tool: Some("Application Migration Service"),
        },
        "Amazon Elastic Container Service (ECS)"
        | "Amazon Elastic Kubernetes Service (EKS)"
        | "AWS Fargate" => Expected {
            strategy: "Replatform",
            destination,
            tool: Some("App2Container"),
        },
        "AWS Elastic BeanStalk" => Expected {
            strategy: "Replatform",
            destination,
            tool: windows.then_some("Windows Web Application Migration Assistant"),
        },
        other => panic!("this suite names a destination with no expectation: {other:?}"),
    }
}

/// The `TargetDestination` an AWS-managed database move to the engine `engine` must land
/// on, for a database running `app_type` today.
///
/// Spelled out here for the same reason: each engine maps onto the AWS managed service
/// that RUNS it — Amazon RDS for MySQL and for PostgreSQL, Amazon DocumentDB for MongoDB
/// (whose API it speaks), and Amazon RDS for every other engine the enumeration names —
/// while `Amazon Aurora` names a family, so the engine the database runs today picks the
/// compatible edition, Babelfish being the Aurora PostgreSQL edition AWS documents for a
/// Microsoft SQL Server source.
fn expected_database_destination(engine: &str, app_type: &str) -> &'static str {
    match engine {
        "MySQL" => "Amazon Relational Database Service on MySQL",
        "AWS PostgreSQL" => "Amazon Relational Database Service on PostgreSQL",
        "MongoDB" => "Amazon DocumentDB",
        "Amazon Aurora" => match app_type {
            "MySQL" | "Maria DB" => "Aurora MySQL",
            "SQLServer" => "Babelfish for Aurora PostgreSQL",
            _ => "Aurora PostgreSQL",
        },
        _ => "Amazon Relational Database Service",
    }
}

/// One seeded server: its id, whether it runs Windows, and the records the portfolio holds
/// for it.
struct Seeded {
    id: &'static str,
    windows: bool,
}

/// The servers every case runs against: one Windows and one Linux machine, so the one
/// destination whose tool depends on the operating system is covered on both sides.
const SERVERS: &[Seeded] = &[
    Seeded {
        id: "0a",
        windows: true,
    },
    Seeded {
        id: "0b",
        windows: false,
    },
];

/// One seeded application component and what the analysis has to do with it.
struct Component {
    id: &'static str,
    /// `ResourceSubType`, which is what decides whether the DATABASE half of the saved
    /// preferences is the one that selects this component's destination.
    sub_type: &'static str,
    /// `AppType`, which also disambiguates an `Amazon Aurora` database target.
    app_type: Option<&'static str>,
    /// `osVersion`, the component's only operating-system member.
    os_version: Option<&'static str>,
    /// The server it runs on.
    server: &'static str,
    /// Whether it runs on Windows, for the Elastic Beanstalk tool.
    windows: bool,
    /// `InclusionStatus`: a component excluded from the assessment is recommended nothing.
    included: bool,
}

/// The components every case runs against: processes and databases, on both servers, with
/// a Windows-ness coming from the app type on one row and from the OS version text on
/// another, an unknown app type and a missing one, a database of each `ResourceSubType`
/// that is one, and a component kept out of the assessment altogether.
const COMPONENTS: &[Component] = &[
    Component {
        id: "1a",
        sub_type: "Process",
        app_type: Some("IIS"),
        os_version: None,
        server: "0a",
        windows: true,
        included: true,
    },
    Component {
        id: "1b",
        sub_type: "Process",
        app_type: Some("Java"),
        os_version: Some("Ubuntu 20.04"),
        server: "0b",
        windows: false,
        included: true,
    },
    Component {
        id: "2a",
        sub_type: "Process",
        app_type: Some("Tomcat"),
        os_version: Some("WindowsServer 2019"),
        server: "0a",
        windows: true,
        included: true,
    },
    Component {
        id: "2b",
        sub_type: "Database",
        app_type: Some("SQLServer"),
        os_version: None,
        server: "0a",
        windows: false,
        included: true,
    },
    Component {
        id: "3a",
        sub_type: "DatabaseProcess",
        app_type: Some("MySQL"),
        os_version: None,
        server: "0b",
        windows: false,
        included: true,
    },
    Component {
        id: "3b",
        sub_type: "Database",
        app_type: Some("Oracle"),
        os_version: None,
        server: "0b",
        windows: false,
        included: true,
    },
    Component {
        id: "4a",
        sub_type: "Process",
        app_type: Some("Unknown"),
        os_version: None,
        server: "0a",
        windows: false,
        included: true,
    },
    Component {
        id: "4b",
        sub_type: "Process",
        app_type: None,
        os_version: None,
        server: "0b",
        windows: false,
        included: true,
    },
    Component {
        id: "ab",
        sub_type: "Process",
        app_type: Some("Spring"),
        os_version: None,
        server: "0a",
        windows: false,
        included: false,
    },
];

impl Component {
    fn is_database(&self) -> bool {
        self.sub_type == "Database" || self.sub_type == "DatabaseProcess"
    }

    /// Whether `AppType` recognises what this component is — what `ApplicationMode`'s
    /// `KNOWN` selects and `UNKNOWN` leaves out.
    fn known(&self) -> bool {
        self.app_type.is_some_and(|app_type| app_type != "Unknown")
    }

    fn record(&self) -> ApplicationComponentRecord {
        ApplicationComponentRecord {
            id: self.id.to_string(),
            name: Some(format!("component-{}", self.id)),
            app_type: self.app_type.map(str::to_string),
            resource_sub_type: Some(self.sub_type.to_string()),
            inclusion_status: Some(
                if self.included {
                    "includeInAssessment"
                } else {
                    "excludeFromAssessment"
                }
                .to_string(),
            ),
            os_version: self.os_version.map(str::to_string),
            associated_server_ids: vec![self.server.to_string()],
            ..Default::default()
        }
    }
}

/// A server record for the seeded inventory, holding only what the analysis reads: its id
/// and the `OSType` the tool for one destination depends on.
fn server_record(seeded: &Seeded) -> ServerRecord {
    ServerRecord {
        id: seeded.id.to_string(),
        name: Some(format!("server-{}", seeded.id)),
        os_type: Some(if seeded.windows { "WINDOWS" } else { "LINUX" }.to_string()),
        ..Default::default()
    }
}

/// One emulator over one shared engine, with the servers and components above seeded into
/// the Region every request here is signed for — the same maps an ingestion writes.
///
/// The engine is handed back beside the router so a case can bring the completion the
/// clock owes an assessment forward ([`complete_the_span`]) instead of sleeping through it.
fn portfolio() -> (Arc<engine::InMemoryEngine>, MultiServiceRouter) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    {
        let state = engine
            .state::<ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState>();
        let mut store = state.lock();
        let region = store.region_entry(TEST_REGION);
        for seeded in SERVERS {
            region.servers.insert(seeded.id.to_string(), server_record(seeded));
        }
        for component in COMPONENTS {
            region
                .application_components
                .insert(component.id.to_string(), component.record());
        }
    }
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine.clone()));
    (engine, router)
}

/// Move the instant the clock owes every assessment of this portfolio its completion into
/// the PAST, so the next read materialises it.
///
/// It is the same thing the passing of the completion span does — the due instant is what
/// the store compares `now` against — without a case spending that span asleep. The
/// completion the reads then observe is still the stored one, timestamps included.
fn complete_the_span(engine: &Arc<engine::InMemoryEngine>) {
    let state =
        engine.state::<ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState>();
    let mut store = state.lock();
    let region = store.region_entry(TEST_REGION);
    for assessment in region.assessments.values_mut() {
        if let Some(due) = assessment.completion_due {
            assessment.completion_due = Some(due - COMPLETION_SPAN - Duration::from_millis(100));
        }
    }
}

/// Push the instant the clock owes every assessment of this portfolio its completion a
/// long way into the FUTURE, so a case can act on an assessment that is certainly still
/// collecting however slow the run is.
///
/// The counterpart of [`complete_the_span`], and the same mechanism: the due instant is
/// what the store compares `now` against.
fn delay_the_span(engine: &Arc<engine::InMemoryEngine>) {
    let state =
        engine.state::<ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState>();
    let mut store = state.lock();
    let region = store.region_entry(TEST_REGION);
    for assessment in region.assessments.values_mut() {
        if let Some(due) = assessment.completion_due {
            assessment.completion_due = Some(due + Duration::from_secs(3_600));
        }
    }
}

async fn put_preferences(router: &MultiServiceRouter, body: Value) -> Answer {
    call(router, "POST", "/put-portfolio-preferences", Some(body)).await
}

async fn server_details(router: &MultiServiceRouter, id: &str) -> Answer {
    call(router, "GET", &format!("/get-server-details/{id}"), None).await
}

async fn server_strategies(router: &MultiServiceRouter, id: &str) -> Answer {
    call(router, "GET", &format!("/get-server-strategies/{id}"), None).await
}

async fn component_details(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/get-applicationcomponent-details/{id}"),
        None,
    )
    .await
}

async fn component_strategies(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/get-applicationcomponent-strategies/{id}"),
        None,
    )
    .await
}

/// Save `body` as the account's preferences and run an assessment to completion, answering
/// the instant the assessment completed at as the wire reports it.
///
/// The preferences are saved BEFORE the assessment starts, because they are the inputs the
/// analysis is computed from and a save made while a task is running is refused.
async fn analyze(
    case: &str,
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    body: Value,
) -> Value {
    if !body.as_object().is_some_and(|members| members.is_empty()) {
        let saved = put_preferences(router, body).await;
        assert_eq!(
            saved.status, 200,
            "case {case:?}: the preferences must be saved, got {} {:?}",
            saved.status, saved.body
        );
    }
    let started = start_assessment(router, minimal_assessment()).await;
    assert_eq!(
        started.status, 200,
        "case {case:?}: the assessment must start, got {} {:?}",
        started.status, started.body
    );
    let assessment_id = started.string("/assessmentId");
    complete_the_span(engine);

    let reported = get_assessment(router, &assessment_id).await;
    assert_eq!(
        at(&reported.body, "/dataCollectionDetails/status"),
        json!(ASSESSMENT_COMPLETE),
        "case {case:?}: the assessment the clock completed reports COMPLETE, got {:?}",
        at(&reported.body, "/dataCollectionDetails/status")
    );
    let completed_at = at(&reported.body, "/dataCollectionDetails/completionTime");
    assert!(
        completed_at.is_number(),
        "case {case:?}: a completed assessment reports its completion time, got {completed_at:?}"
    );
    completed_at
}

/// One resource as the two reads of it answer: its detail and its strategy list, with the
/// JSON pointer each is reported under and what the resource is called in a failure.
struct Reads {
    what: String,
    details: Answer,
    detail_prefix: &'static str,
    strategies: Answer,
    strategy_prefix: &'static str,
}

/// Read one server of the portfolio both ways: `GetServerDetails` and
/// `GetServerStrategies`.
async fn read_server(router: &MultiServiceRouter, id: &str) -> Reads {
    Reads {
        what: format!("server {id:?}"),
        details: server_details(router, id).await,
        detail_prefix: "/serverDetail",
        strategies: server_strategies(router, id).await,
        strategy_prefix: "/serverStrategies",
    }
}

/// Read one application component both ways: `GetApplicationComponentDetails` and
/// `GetApplicationComponentStrategies`.
async fn read_component(router: &MultiServiceRouter, id: &str) -> Reads {
    Reads {
        what: format!("component {id:?}"),
        details: component_details(router, id).await,
        detail_prefix: "/applicationComponentDetail",
        strategies: component_strategies(router, id).await,
        strategy_prefix: "/applicationComponentStrategies",
    }
}

/// Assert the `RecommendationSet` at `prefix` is the one `expected` names.
fn assert_recommendation(case: &str, read: &str, body: &Value, prefix: &str, expected: Expected) {
    for (member, want) in [
        ("strategy", json!(expected.strategy)),
        ("targetDestination", json!(expected.destination)),
        (
            "transformationTool/name",
            expected.tool.map_or(Value::Null, |tool| json!(tool)),
        ),
    ] {
        let pointer = format!("{prefix}/{member}");
        assert_eq!(
            at(body, &pointer),
            want,
            "case {case:?}: {read} must report {want:?} at {pointer}, got {:?}",
            at(body, &pointer)
        );
    }
}

/// Assert the resource `reads` answers for carries `expected` — on its detail, on its
/// strategy list (with the `recommended` status, since it is what the service recommends)
/// and as the analysis instant `analyzed_at`.
fn assert_analyzed(case: &str, reads: &Reads, expected: Expected, analyzed_at: &Value) {
    let Reads {
        what,
        details,
        detail_prefix,
        strategies,
        strategy_prefix,
    } = reads;
    assert_recommendation(
        case,
        &format!("{what}'s detail"),
        &details.body,
        &format!("{detail_prefix}/recommendationSet"),
        expected,
    );
    assert_eq!(
        at(&details.body, &format!("{detail_prefix}/lastAnalyzedTimestamp")),
        *analyzed_at,
        "case {case:?}: {what} was analysed when the assessment completed, got {:?}",
        at(&details.body, &format!("{detail_prefix}/lastAnalyzedTimestamp"))
    );
    assert_recommendation(
        case,
        &format!("{what}'s strategies"),
        &strategies.body,
        &format!("{strategy_prefix}/0/recommendation"),
        expected,
    );
    assert_eq!(
        at(&strategies.body, &format!("{strategy_prefix}/0/status")),
        json!(RECOMMENDED),
        "case {case:?}: the strategy the analysis produced for {what} is the recommended \
         one, got {:?}",
        at(&strategies.body, &format!("{strategy_prefix}/0/status"))
    );
    let listed = at(&strategies.body, strategy_prefix);
    assert_eq!(
        listed.as_array().map(Vec::len),
        Some(1),
        "case {case:?}: nobody chose a preference for {what}, so the analysis is the one \
         strategy reported, got {listed:?}"
    );
}

/// An `applicationPreferences` naming one management preference variant and one target.
fn application_preferences(variant: &str, destination: &str) -> Value {
    json!({
        "applicationPreferences": {
            "managementPreference": {variant: {"targetDestination": [destination]}}
        }
    })
}

/// Every (preferences, destination) row of the application table: each variant of
/// `ManagementPreference` crossed with every value of its OWN destination enumeration,
/// plus the portfolio that saved no preference at all — which selects no destination, so
/// the analysis recommends retaining.
fn application_rows() -> Vec<(String, Value, &'static str)> {
    let mut rows: Vec<(String, Value, &'static str)> = vec![(
        "no preferences saved at all".to_string(),
        json!({}),
        "None specified",
    )];
    for (variant, catalog) in [
        ("awsManagedResources", AWS_MANAGED_DESTINATIONS),
        ("selfManageResources", SELF_MANAGE_DESTINATIONS),
        ("noPreference", NO_PREFERENCE_DESTINATIONS),
    ] {
        for destination in catalog {
            rows.push((
                format!("{variant} to {destination:?}"),
                application_preferences(variant, destination),
                destination,
            ));
        }
    }
    rows
}

/// A completed assessment recommends, for every server and every application component of
/// the portfolio, what the saved application preference selects — and a database component
/// nothing selected a database destination for is left where it is.
#[tokio::test]
async fn the_analysis_follows_the_saved_application_preference() {
    for (case, preferences, destination) in application_rows() {
        let (engine, router) = portfolio();
        let analyzed_at = analyze(&case, &router, &engine, preferences).await;

        for seeded in SERVERS {
            let expected = expected_application(destination, seeded.windows);
            assert_analyzed(&case, &read_server(&router, seeded.id).await, expected, &analyzed_at);
        }

        for component in COMPONENTS.iter().filter(|component| component.included) {
            // No database preference is saved in this table, so a database component has
            // nothing selecting a destination for it: the application preference governs
            // application workloads and the database preference databases.
            let expected = if component.is_database() {
                expected_application("None specified", false)
            } else {
                expected_application(destination, component.windows)
            };
            assert_analyzed(
                &case,
                &read_component(&router, component.id).await,
                expected,
                &analyzed_at,
            );
        }
    }
}

/// A `databasePreferences` naming a management direction and, when `engine` is given, a
/// migration preference of `variant` carrying it.
fn database_preferences(management: &str, variant: &str, engine: Option<&str>) -> Value {
    let migration = match engine {
        Some(engine) => json!({variant: {"targetDatabaseEngine": [engine]}}),
        None => json!({variant: {}}),
    };
    json!({
        "databasePreferences": {
            "databaseManagementPreference": management,
            "databaseMigrationPreference": migration,
        }
    })
}

/// What a database component must be recommended for one row of the database table.
enum DatabaseExpectation {
    /// Every database of the portfolio gets this one recommendation.
    Flat(Expected),
    /// The destination is the one the `TargetDatabaseEngine` this row names lands on, as
    /// the engine each database runs today disambiguates it, reached with `tool`.
    Target {
        engine: &'static str,
        tool: &'static str,
    },
    /// The destination is the managed service that runs the engine each database runs
    /// today — a move that keeps the engine.
    SameEngine,
}

/// One row of the database table: the preferences it saves, and what the databases of the
/// portfolio must then be recommended.
struct DatabaseRow {
    what: String,
    preferences: Value,
    expectation: DatabaseExpectation,
}

/// The `TargetDestination` a move that KEEPS the engine `app_type` lands on.
///
/// Spelled out here independently of the crate: `TargetDestination` names Amazon RDS for
/// MySQL and for PostgreSQL specifically, and the managed relational service itself for
/// every other engine it runs.
fn expected_same_engine_destination(app_type: &str) -> &'static str {
    match app_type {
        "MySQL" => "Amazon Relational Database Service on MySQL",
        "PostgreSQLServer" => "Amazon Relational Database Service on PostgreSQL",
        _ => "Amazon Relational Database Service",
    }
}

/// Every row of the database table: each `TargetDatabaseEngine` value as a heterogeneous
/// and as a no-preference target, the homogeneous preference that names no other engine at
/// all, a management direction on its own, the direction that keeps the engine the
/// account's to run, and the portfolio that saved no database preference.
fn database_rows() -> Vec<DatabaseRow> {
    let mut rows: Vec<DatabaseRow> = vec![
        DatabaseRow {
            what: "no database preference saved".to_string(),
            preferences: json!({}),
            expectation: DatabaseExpectation::Flat(expected_application("None specified", false)),
        },
        DatabaseRow {
            // Self-managing the engine means the database moves to AWS as it is.
            what: format!(
                "databaseManagementPreference {:?}",
                DATABASE_MANAGEMENT_PREFERENCES[1]
            ),
            preferences: json!({
                "databasePreferences": {
                    "databaseManagementPreference": DATABASE_MANAGEMENT_PREFERENCES[1]
                }
            }),
            expectation: DatabaseExpectation::Flat(expected_application(
                "Amazon Elastic Cloud Compute (EC2)",
                false,
            )),
        },
        DatabaseRow {
            // An AWS-managed direction naming no migration preference names no other
            // engine either, so it is the same engine, managed.
            what: format!(
                "databaseManagementPreference {:?}",
                DATABASE_MANAGEMENT_PREFERENCES[0]
            ),
            preferences: json!({
                "databasePreferences": {
                    "databaseManagementPreference": DATABASE_MANAGEMENT_PREFERENCES[0]
                }
            }),
            expectation: DatabaseExpectation::SameEngine,
        },
        DatabaseRow {
            // `Homogeneous$targetDatabaseEngine` is the one of these lists the model lets
            // be empty, because moving to the same type of database into AWS names no
            // other engine: the destination is the managed service running the engine the
            // database runs today.
            what: "homogeneous migration naming no engine".to_string(),
            preferences: database_preferences(
                DATABASE_MANAGEMENT_PREFERENCES[0],
                "homogeneous",
                None,
            ),
            expectation: DatabaseExpectation::SameEngine,
        },
        DatabaseRow {
            // And with the one engine value that list declares, which chooses no other
            // engine either.
            what: format!(
                "homogeneous migration to {:?}",
                TARGET_DATABASE_ENGINES[0]
            ),
            preferences: database_preferences(
                DATABASE_MANAGEMENT_PREFERENCES[0],
                "homogeneous",
                Some(TARGET_DATABASE_ENGINES[0]),
            ),
            expectation: DatabaseExpectation::SameEngine,
        },
    ];
    for engine in TARGET_DATABASE_ENGINES {
        rows.push(DatabaseRow {
            // A heterogeneous move changes the engine, so the schema is converted first.
            what: format!("heterogeneous migration to {engine:?}"),
            preferences: database_preferences(
                DATABASE_MANAGEMENT_PREFERENCES[0],
                "heterogeneous",
                Some(engine),
            ),
            expectation: DatabaseExpectation::Target {
                engine,
                tool: "Schema Conversion Tool",
            },
        });
        rows.push(DatabaseRow {
            what: format!("migration noPreference to {engine:?}"),
            preferences: database_preferences(
                DATABASE_MANAGEMENT_PREFERENCES[2],
                "noPreference",
                Some(engine),
            ),
            expectation: DatabaseExpectation::Target {
                engine,
                tool: "Database Migration Service",
            },
        });
    }
    rows
}

/// A completed assessment recommends, for every DATABASE component of the portfolio, what
/// the saved database preference selects — the engine it names deciding the destination and
/// whether the engine CHANGES deciding the tool — while the application components of the
/// same portfolio keep following the application preference.
#[tokio::test]
async fn the_analysis_follows_the_saved_database_preference() {
    // One application preference is saved in every row beside the database one, so each
    // row also shows the two halves selecting for their own resources.
    let application_destination = "AWS Fargate";
    for row in database_rows() {
        let (engine, router) = portfolio();
        let mut preferences = application_preferences("awsManagedResources", application_destination);
        if let Some(members) = row.preferences.as_object() {
            for (member, value) in members {
                preferences[member] = value.clone();
            }
        }
        let analyzed_at = analyze(&row.what, &router, &engine, preferences).await;

        for component in COMPONENTS.iter().filter(|component| component.included) {
            let expected = if component.is_database() {
                match row.expectation {
                    DatabaseExpectation::Flat(flat) => flat,
                    DatabaseExpectation::Target { engine, tool } => Expected {
                        strategy: "Replatform",
                        destination: expected_database_destination(
                            engine,
                            component.app_type.unwrap_or_default(),
                        ),
                        tool: Some(tool),
                    },
                    DatabaseExpectation::SameEngine => Expected {
                        strategy: "Replatform",
                        destination: expected_same_engine_destination(
                            component.app_type.unwrap_or_default(),
                        ),
                        tool: Some("Database Migration Service"),
                    },
                }
            } else {
                expected_application(application_destination, component.windows)
            };
            assert_analyzed(
                &row.what,
                &read_component(&router, component.id).await,
                expected,
                &analyzed_at,
            );
        }
    }
}

/// `applicationMode` is "the classification for application component types", so it
/// selects WHICH components the analysis recommends for — and a component excluded from the
/// assessment is recommended nothing whatever the mode.
///
/// One row per value of the catalog, plus the portfolio that names no mode, so every value
/// is checked on the side of the rule it falls.
#[tokio::test]
async fn the_application_mode_selects_the_components_the_analysis_covers() {
    let destination = "Amazon Elastic Container Service (ECS)";
    let modes: Vec<Option<&str>> = std::iter::once(None)
        .chain(APPLICATION_MODES.iter().map(|mode| Some(*mode)))
        .collect();
    for mode in modes {
        let case = format!("applicationMode {mode:?}");
        let (engine, router) = portfolio();
        let mut preferences = application_preferences("selfManageResources", destination);
        if let Some(mode) = mode {
            preferences["applicationMode"] = json!(mode);
        }
        let analyzed_at = analyze(&case, &router, &engine, preferences).await;

        for component in COMPONENTS {
            let covered = component.included
                && match mode {
                    Some("KNOWN") => component.known(),
                    Some("UNKNOWN") => !component.known(),
                    _ => true,
                };
            let reads = read_component(&router, component.id).await;
            if covered {
                let expected = if component.is_database() {
                    expected_application("None specified", false)
                } else {
                    expected_application(destination, component.windows)
                };
                assert_analyzed(&case, &reads, expected, &analyzed_at);
                continue;
            }
            // Outside the analysis: the component is left exactly as the ingestion stored
            // it, rather than given a recommendation nothing computed for it.
            for pointer in [
                "/applicationComponentDetail/recommendationSet",
                "/applicationComponentDetail/lastAnalyzedTimestamp",
            ] {
                assert_eq!(
                    at(&reads.details.body, pointer),
                    Value::Null,
                    "case {case:?}: component {:?} is not one the analysis covers, so it \
                     reports nothing at {pointer}, got {:?}",
                    component.id,
                    at(&reads.details.body, pointer)
                );
            }
            assert_eq!(
                at(&reads.strategies.body, "/applicationComponentStrategies"),
                json!([]),
                "case {case:?}: component {:?} is not one the analysis covers, so it has no \
                 strategy, got {:?}",
                component.id,
                at(&reads.strategies.body, "/applicationComponentStrategies")
            );
        }
    }
}

/// What a server must report for the components running on it once they are analysed: the
/// strategy summary, and the component count on its own strategy listing.
#[tokio::test]
async fn a_server_summarises_the_strategies_of_its_own_components() {
    let destination = "AWS Fargate";
    let case = format!("awsManagedResources to {destination:?}");
    let (engine, router) = portfolio();
    let analyzed_at = analyze(&case, &router, &engine, application_preferences("awsManagedResources", destination)).await;
    assert!(
        analyzed_at.is_number(),
        "case {case:?}: the assessment has to have completed for the summary to exist"
    );

    for seeded in SERVERS {
        // The expectation is counted off the seeded components themselves: the ones
        // running on this server that the assessment covered, grouped by the strategy the
        // analysis gave them. No database preference is saved, so the databases retain.
        let mut counts: std::collections::BTreeMap<&str, i64> = std::collections::BTreeMap::new();
        for component in COMPONENTS
            .iter()
            .filter(|component| component.included && component.server == seeded.id)
        {
            let expected = if component.is_database() {
                expected_application("None specified", false)
            } else {
                expected_application(destination, component.windows)
            };
            *counts.entry(expected.strategy).or_default() += 1;
        }
        let expected_summary: Vec<Value> = counts
            .iter()
            .map(|(strategy, count)| json!({"strategy": strategy, "count": count}))
            .collect();

        let details = server_details(&router, seeded.id).await;
        assert_eq!(
            at(&details.body, "/serverDetail/applicationComponentStrategySummary"),
            json!(expected_summary),
            "case {case:?}: server {:?} summarises the strategies of its own components, got {:?}",
            seeded.id,
            at(&details.body, "/serverDetail/applicationComponentStrategySummary")
        );

        // And the count beside the server's own strategy is the summary's entry for it.
        let own = expected_application(destination, seeded.windows).strategy;
        let strategies = server_strategies(&router, seeded.id).await;
        assert_eq!(
            at(&strategies.body, "/serverStrategies/0/numberOfApplicationComponents"),
            json!(counts.get(own).copied().unwrap_or_default()),
            "case {case:?}: server {:?} counts the components the analysis gave {own:?}, got {:?}",
            seeded.id,
            at(&strategies.body, "/serverStrategies/0/numberOfApplicationComponents")
        );
    }
}

/// An assessment that does NOT complete analyses nothing: a stop ends the collection, so
/// the portfolio keeps the recommendation it had — none.
#[tokio::test]
async fn a_stopped_assessment_recommends_nothing() {
    let (engine, router) = portfolio();
    let saved = put_preferences(
        &router,
        application_preferences("selfManageResources", "Amazon Elastic Cloud Compute (EC2)"),
    )
    .await;
    assert_eq!(saved.status, 200, "the preferences must be saved, got {:?}", saved.body);

    let assessment_id = start_assessment(&router, minimal_assessment())
        .await
        .string("/assessmentId");
    // The stop has to reach an assessment that is still collecting, so the completion the
    // clock owes it is pushed out of the way first: a slow run must not turn this case
    // into one about a completion.
    delay_the_span(&engine);
    let stopped = stop_assessment(&router, &assessment_id).await;
    assert_eq!(
        stopped.status, 200,
        "the stop must be accepted, got {} {:?}",
        stopped.status, stopped.body
    );

    for seeded in SERVERS {
        let details = server_details(&router, seeded.id).await;
        for member in ["recommendationSet", "lastAnalyzedTimestamp"] {
            let pointer = format!("/serverDetail/{member}");
            assert_eq!(
                at(&details.body, &pointer),
                Value::Null,
                "a stopped assessment analysed nothing, so server {:?} reports no {member}, \
                 got {:?}",
                seeded.id,
                at(&details.body, &pointer)
            );
        }
        let strategies = server_strategies(&router, seeded.id).await;
        assert_eq!(
            at(&strategies.body, "/serverStrategies"),
            json!([]),
            "a stopped assessment analysed nothing, so server {:?} has no strategy, got {:?}",
            seeded.id,
            at(&strategies.body, "/serverStrategies")
        );
    }
}

/// The whole loop the issue describes, over HTTP and over the real completion span: save
/// the preferences, start the assessment, poll until it reports `COMPLETE`, and read the
/// recommendation the analysis produced off the server and off one of its components —
/// twice, because a read reports the analysis that ran rather than one of its own.
#[tokio::test]
async fn the_recommendation_is_readable_once_the_assessment_reports_complete() {
    let destination = "AWS Elastic BeanStalk";
    let case = format!("awsManagedResources to {destination:?}");
    let (_engine, router) = portfolio();
    let saved = put_preferences(
        &router,
        json!({
            "applicationPreferences": {
                "managementPreference": {"awsManagedResources": {"targetDestination": [destination]}}
            },
            "applicationMode": APPLICATION_MODES[0],
        }),
    )
    .await;
    assert_eq!(saved.status, 200, "the preferences must be saved, got {:?}", saved.body);

    let assessment_id = start_assessment(&router, minimal_assessment())
        .await
        .string("/assessmentId");

    // Poll, as a client does, until the assessment reports the status that means the
    // portfolio has been analysed.
    let mut reported = get_assessment(&router, &assessment_id).await;
    for _ in 0..40 {
        if at(&reported.body, "/dataCollectionDetails/status") == json!(ASSESSMENT_COMPLETE) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        reported = get_assessment(&router, &assessment_id).await;
    }
    assert_eq!(
        at(&reported.body, "/dataCollectionDetails/status"),
        json!(ASSESSMENT_COMPLETE),
        "the assessment must reach COMPLETE within the completion span, got {:?}",
        reported.body
    );
    let analyzed_at = at(&reported.body, "/dataCollectionDetails/completionTime");

    let server = &SERVERS[0];
    let component = &COMPONENTS[0];
    for round in ["the first read after the completion", "a second read"] {
        let case = format!("{case}, {round}");
        assert_analyzed(
            &case,
            &read_server(&router, server.id).await,
            expected_application(destination, server.windows),
            &analyzed_at,
        );
        assert_analyzed(
            &case,
            &read_component(&router, component.id).await,
            expected_application(destination, component.windows),
            &analyzed_at,
        );
    }
}
