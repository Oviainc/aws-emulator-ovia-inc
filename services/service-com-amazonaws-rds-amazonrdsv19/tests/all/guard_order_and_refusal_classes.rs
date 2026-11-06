// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHICH refusal a request gets when it trips more than one guard,
//! and for the wording of the refusals this service words itself.
//!
//! Every call goes over the wire: the service is mounted on a `MultiServiceRouter` exactly
//! as the production router mounts it (`crate::db_instances::emulator`), each request is a
//! real awsQuery form POST carrying the SigV4 credential scope the router keys on, and
//! every assertion reads the XML the emulator answered with.
//!
//! The rules under test, each asserted over a TABLE spanning what it generalises over
//! rather than over one request:
//!
//! 1. A request is refused in the order the service evaluates it: what the REQUEST itself
//!    carries (a value outside a published set, a pair that cannot hold together) before
//!    the resources it NAMES are resolved, those before what this emulator does not
//!    implement — so a request AWS answers with a client error never gets a server-fault
//!    class instead.
//! 2. Acting on a custom DB engine version the account does not hold is withheld rather
//!    than reported as missing, with one fixed sentence that discloses nothing.
//! 3. A parameter group Amazon RDS provides itself exists for every engine-version family
//!    it publishes: a request that NAMES one resolves, and a request that would CHANGE one
//!    is refused as protected rather than as missing.
//! 4. An event subscription's categories and source identifiers belong to its source type,
//!    so a request carrying either without it is refused from the request alone.
//! 5. An identifier that breaks the identifier grammar is refused with the sentence pair
//!    that names the member and states the rule, except for the members Amazon RDS words by
//!    their own subject.

use service_com_amazonaws_rds_amazonrdsv19::ops::types::SOURCE_TYPE_VALUES;
use service_com_amazonaws_rds_amazonrdsv19::state::EVENT_SUBSCRIPTION_SOURCE_TYPES;

use crate::db_instances::{call, call_in, emulator, expect_error, expect_ok, text, REGION};

/// A symmetric-encryption KMS key identifier of the form `KMSKeyId` takes.
const KMS_KEY_ID: &str = "arn:aws:kms:us-west-2:000000000000:key/12345678-1234-1234-1234-123456789012";

/// The sentence Amazon RDS refuses a value that names no DB engine with: fixed text, with
/// nothing from the request in it.
const INVALID_DB_ENGINE: &str = "Invalid DB engine";

/// The Query API error code a value outside what its member accepts is refused with.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValue";

/// The sentence a request this service will not serve against a custom engine version is
/// refused with. Spelled out here so the wording is pinned by the test rather than by the
/// code under test.
const CEV_DENIAL: &str = "The custom engine version you requested may not exist or you \
                          don't have the required permission.";

/// The sentence Amazon RDS refuses a change to one of its own parameter groups with.
const CANNOT_MODIFY_DEFAULT: &str = "Cannot modify a default parameter group.";

/// The sentence Amazon RDS refuses event categories carrying no source type with.
const CATEGORIES_NEED_A_SOURCE_TYPE: &str = "If SourceType is null, Categories must also be null.";

/// The sentence Amazon RDS refuses source identifiers carrying no source type with, which
/// is the constraint `CreateEventSubscriptionMessage$SourceIds` states in those words.
const SOURCE_IDS_NEED_A_SOURCE_TYPE: &str =
    "If SourceIds are supplied, SourceType must also be provided.";

/// The sentence Amazon RDS refuses a `SourceType` no subscription may name with: fixed
/// text, every valid type single-quoted, in the order the service lists them.
const INVALID_EVENT_SOURCE_TYPE: &str = "Invalid event source type. Valid types are \
                                         'db-instance', 'db-security-group', \
                                         'db-parameter-group', 'db-snapshot', 'db-cluster', \
                                         'db-cluster-snapshot', 'custom-engine-version', \
                                         'db-proxy', 'blue-green-deployment', \
                                         'db-shard-group', 'zero-etl'";

/// The identifier-rule refusal, which names the request member at fault and states the
/// whole rule; nothing else in it varies and the rejected value never appears.
fn not_a_valid_identifier(member: &str) -> String {
    format!(
        "The parameter {member} is not a valid identifier. Identifiers must begin with a \
         letter; must contain only ASCII letters, digits, and hyphens; and must not end with \
         a hyphen or contain two consecutive hyphens."
    )
}

/// Assert the status, the wire error `<Code>` and the exact `<Message>` of one refusal.
fn expect_refusal(case: &str, response: (u16, String), status: u16, code: &str, message: &str) {
    let body = expect_error(case, response, status, code);
    let answered = text(&body, "Message");
    assert_eq!(
        answered, message,
        "case {case:?}: expected the message {message:?}, got {answered:?} in {body}"
    );
}

/// One row of the custom-engine-version create table: what the request names, and the
/// refusal it must get even though it also trips a later guard.
struct CevCreateCase {
    case: &'static str,
    engine: &'static str,
    engine_version: &'static str,
    extra: &'static [(&'static str, &'static str)],
    /// The wire error `<Code>` of the refusal; `InvalidParameterValue` for a value no
    /// engine build answers to, `InvalidParameterCombination` for members that cannot hold
    /// together.
    code: &'static str,
    /// `None` for a request this service accepts.
    refusal: Option<&'static str>,
}

/// The rows of that table.
///
/// `CreateCustomDBEngineVersion$Engine` publishes the engines a CEV can be created for, and
/// `$EngineVersion` the name format one carries (*major*.*minor*.*customized_string* for
/// the SQL Server engines). A CEV is built ON a published engine build, so the rows span
/// all three halves of what names one — the engine's own name, whether this service
/// publishes a build of that engine, and the CEV name's form — and both of the later guards
/// they would otherwise trip, the image RDS Custom for SQL Server requires and the KMS key
/// identifier a named key has to be, so what is asserted is WHICH refusal the request gets.
const CEV_CREATE_CASES: &[CevCreateCase] = &[
    CevCreateCase {
        case: "an engine AWS provides the versions of, with no KMS key either",
        engine: "postgres",
        engine_version: "16.4.my_cev1",
        extra: &[],
        code: INVALID_PARAMETER_VALUE,
        refusal: Some(INVALID_DB_ENGINE),
    },
    CevCreateCase {
        case: "a published custom engine in another case, which names no engine",
        engine: "Custom-Oracle-EE",
        engine_version: "19.my_cev1",
        extra: &[("KMSKeyId", KMS_KEY_ID)],
        code: INVALID_PARAMETER_VALUE,
        refusal: Some(INVALID_DB_ENGINE),
    },
    CevCreateCase {
        case: "a value that is no engine at all",
        engine: "custom-oracle",
        engine_version: "19.my_cev1",
        extra: &[("KMSKeyId", KMS_KEY_ID)],
        code: INVALID_PARAMETER_VALUE,
        refusal: Some(INVALID_DB_ENGINE),
    },
    CevCreateCase {
        case: "a CEV name carrying no major engine version, with no KMS key either",
        engine: "custom-oracle-ee",
        engine_version: "my_cev1",
        extra: &[],
        code: INVALID_PARAMETER_VALUE,
        refusal: Some(INVALID_DB_ENGINE),
    },
    CevCreateCase {
        case: "a CEV name whose leading component is not a version",
        engine: "custom-oracle-se2",
        engine_version: "cev-19.my_cev1",
        extra: &[("KMSKeyId", KMS_KEY_ID)],
        code: INVALID_PARAMETER_VALUE,
        refusal: Some(INVALID_DB_ENGINE),
    },
    CevCreateCase {
        case: "a CEV name that is a major version and nothing else",
        engine: "custom-sqlserver-ee",
        engine_version: "15",
        extra: &[("KMSKeyId", KMS_KEY_ID), ("ImageId", "ami-0abcdef1234567890")],
        code: INVALID_PARAMETER_VALUE,
        refusal: Some(INVALID_DB_ENGINE),
    },
    CevCreateCase {
        case: "a CEV name with a major version and an empty customized string",
        engine: "custom-sqlserver-ee",
        engine_version: "15.",
        extra: &[("KMSKeyId", KMS_KEY_ID), ("ImageId", "ami-0abcdef1234567890")],
        code: INVALID_PARAMETER_VALUE,
        refusal: Some(INVALID_DB_ENGINE),
    },
    // An engine the member publishes and this service publishes no build of: there is
    // nothing for the CEV to be built on, so the availability of the ENGINE is what the
    // request is refused for — before the image and the KMS key it does not carry either.
    CevCreateCase {
        case: "a published custom engine this service publishes no build of",
        engine: "custom-oracle-ee",
        engine_version: "19.no_build_cev",
        extra: &[],
        code: INVALID_PARAMETER_VALUE,
        refusal: Some(INVALID_DB_ENGINE),
    },
    CevCreateCase {
        case: "the same engine with every other member it could trip a guard on",
        engine: "custom-oracle-se2",
        engine_version: "19.every_member_cev",
        extra: &[
            ("KMSKeyId", "not-a-kms-key"),
            ("ImageId", "ami-0abcdef1234567890"),
            ("UseAwsProvidedLatestImage", "true"),
        ],
        code: INVALID_PARAMETER_VALUE,
        refusal: Some(INVALID_DB_ENGINE),
    },
    // The control rows: the pair names an engine build this service publishes, so the
    // request reaches the guards that come after it.
    CevCreateCase {
        case: "RDS Custom for SQL Server and a well-formed name, with no image",
        engine: "custom-sqlserver-web",
        engine_version: "15.00.4249.2.no_ami_cev",
        extra: &[("KMSKeyId", KMS_KEY_ID)],
        code: "InvalidParameterCombination",
        refusal: Some(
            "exactly one of ImageId, UseAwsProvidedLatestImage or \
             SourceCustomDbEngineVersionIdentifier must be specified: an AMI ID is required \
             to create a CEV for RDS Custom for SQL Server",
        ),
    },
    CevCreateCase {
        case: "every prerequisite met: the CEV is created",
        engine: "custom-sqlserver-ee",
        engine_version: "15.00.4385.2.served_cev",
        extra: &[("KMSKeyId", KMS_KEY_ID), ("ImageId", "ami-0abcdef1234567890")],
        code: INVALID_PARAMETER_VALUE,
        refusal: None,
    },
];

#[tokio::test]
async fn a_custom_engine_version_create_names_an_engine_build_before_anything_else() {
    let router = emulator();
    for row in CEV_CREATE_CASES {
        let mut params: Vec<(&str, String)> = vec![
            ("Engine", row.engine.to_string()),
            ("EngineVersion", row.engine_version.to_string()),
        ];
        params.extend(
            row.extra
                .iter()
                .map(|(name, value)| (*name, (*value).to_string())),
        );
        let response = call(&router, "CreateCustomDBEngineVersion", &params).await;
        match row.refusal {
            None => {
                let created = expect_ok(row.case, response);
                assert_eq!(
                    text(&created, "EngineVersion"),
                    row.engine_version,
                    "case {:?}: expected the CEV the request named, got {created}",
                    row.case
                );
            }
            Some(message) => {
                expect_refusal(row.case, response, 400, row.code, message);
                // A refused create stored nothing: the pair is still unheld, so managing
                // it is denied.
                expect_refusal(
                    row.case,
                    call(
                        &router,
                        "ModifyCustomDBEngineVersion",
                        &[
                            ("Engine", row.engine),
                            ("EngineVersion", row.engine_version),
                            ("Status", "available"),
                        ],
                    )
                    .await,
                    403,
                    "AccessDenied",
                    CEV_DENIAL,
                );
            }
        }
    }
}

/// One row of the unheld-CEV table: a `(Engine, EngineVersion)` pair naming no custom
/// engine version the account holds in the region the call is signed for.
struct UnheldCev {
    case: &'static str,
    region: &'static str,
    engine: &'static str,
    engine_version: &'static str,
}

/// The ways a pair can name no held CEV, against the one CEV this suite creates
/// (`sqlserver-ee`/`16.00.4175.1.held_cev` in [`REGION`]).
const UNHELD_CEVS: &[UnheldCev] = &[
    UnheldCev {
        case: "a CEV name nothing created",
        region: REGION,
        engine: "sqlserver-ee",
        engine_version: "16.00.4175.1.never_created_cev",
    },
    UnheldCev {
        case: "the held CEV name under another engine",
        region: REGION,
        engine: "sqlserver-se",
        engine_version: "16.00.4175.1.held_cev",
    },
    UnheldCev {
        case: "the held CEV in another region",
        region: "eu-west-1",
        engine: "sqlserver-ee",
        engine_version: "16.00.4175.1.held_cev",
    },
    UnheldCev {
        case: "an engine version this service publishes rather than one an account minted",
        region: REGION,
        engine: "postgres",
        engine_version: "16.4",
    },
];

#[tokio::test]
async fn managing_a_cev_the_account_does_not_hold_is_denied_by_every_such_call() {
    let router = emulator();
    expect_ok(
        "create the held CEV",
        call(
            &router,
            "CreateCustomDBEngineVersion",
            &[
                ("Engine", "sqlserver-ee"),
                ("EngineVersion", "16.00.4175.1.held_cev"),
                ("KMSKeyId", KMS_KEY_ID),
            ],
        )
        .await,
    );

    // The two calls that MANAGE a CEV answer the identical denial, so a caller cannot tell
    // from either one whether a version of that name exists.
    for action in ["ModifyCustomDBEngineVersion", "DeleteCustomDBEngineVersion"] {
        for row in UNHELD_CEVS {
            let case = format!("{action}: {}", row.case);
            let body = expect_error(
                &case,
                call_in(
                    &router,
                    row.region,
                    action,
                    &[("Engine", row.engine), ("EngineVersion", row.engine_version)],
                )
                .await,
                403,
                "AccessDenied",
            );
            let message = text(&body, "Message");
            assert_eq!(
                message, CEV_DENIAL,
                "case {case:?}: expected the fixed denial, got {message:?} in {body}"
            );
            for leaked in [row.region, row.engine, row.engine_version] {
                assert!(
                    !message.contains(leaked),
                    "case {case:?}: the denial must disclose nothing about the request, but \
                     it carries {leaked:?}: {message}"
                );
            }
        }
    }
}

/// One row of the cluster-create table: the DB cluster parameter group the request names,
/// the unsupported feature it also asks for, and the answer it must get.
struct ClusterGroupCase {
    case: &'static str,
    /// `None` for a request that names no parameter group at all.
    group: Option<&'static str>,
    /// The member this emulator does not implement that the request also carries.
    unsupported: Option<&'static str>,
    status: u16,
    code: &'static str,
}

/// The rows of that table.
///
/// `CreateDBCluster$DBClusterParameterGroupName` must "match the name of an existing DB
/// cluster parameter group", and the groups RDS provides per engine-version family
/// (`default.<family>`) are there without any request having created them. The rows span a
/// group this region holds, two of RDS's own groups, two absent names and no group at all,
/// each against a member this emulator answers `NotImplemented` for, so what is asserted
/// is which guard answers first.
///
/// `ManageMasterUserPassword` is SERVED (the secret RDS manages is minted in Secrets
/// Manager — see `managed_master_user_password.rs`), so the rows that carry it are the
/// ones where another refusal must come first: the group the request names resolves to
/// nothing, and no secret is minted for a create that is refused.
const CLUSTER_GROUP_CASES: &[ClusterGroupCase] = &[
    ClusterGroupCase {
        case: "an absent group beside ManageMasterUserPassword",
        group: Some("emu-absent-cpg"),
        unsupported: Some("ManageMasterUserPassword"),
        status: 404,
        code: "DBClusterParameterGroupNotFound",
    },
    ClusterGroupCase {
        case: "an absent group beside WithExpressConfiguration",
        group: Some("emu-absent-other-cpg"),
        unsupported: Some("WithExpressConfiguration"),
        status: 404,
        code: "DBClusterParameterGroupNotFound",
    },
    ClusterGroupCase {
        case: "an absent group alone",
        group: Some("emu-absent-plain-cpg"),
        unsupported: None,
        status: 404,
        code: "DBClusterParameterGroupNotFound",
    },
    ClusterGroupCase {
        case: "a group this region holds, beside WithExpressConfiguration",
        group: Some("emu-held-cpg"),
        unsupported: Some("WithExpressConfiguration"),
        status: 501,
        code: "NotImplemented",
    },
    ClusterGroupCase {
        case: "a group RDS provides itself, beside WithExpressConfiguration",
        group: Some("default.aurora-mysql8.0"),
        unsupported: Some("WithExpressConfiguration"),
        status: 501,
        code: "NotImplemented",
    },
    ClusterGroupCase {
        case: "another of RDS's own groups, in mixed case, beside WithExpressConfiguration",
        group: Some("Default.Aurora-PostgreSQL15"),
        unsupported: Some("WithExpressConfiguration"),
        status: 501,
        code: "NotImplemented",
    },
    ClusterGroupCase {
        case: "no group named at all, beside WithExpressConfiguration",
        group: None,
        unsupported: Some("WithExpressConfiguration"),
        status: 501,
        code: "NotImplemented",
    },
];

#[tokio::test]
async fn a_cluster_create_resolves_the_group_it_names_before_refusing_a_feature() {
    let router = emulator();
    expect_ok(
        "create the held cluster parameter group",
        call(
            &router,
            "CreateDBClusterParameterGroup",
            &[
                ("DBClusterParameterGroupName", "emu-held-cpg"),
                ("DBParameterGroupFamily", "aurora-mysql8.0"),
                ("Description", "a-group-under-test"),
            ],
        )
        .await,
    );

    for (index, row) in CLUSTER_GROUP_CASES.iter().enumerate() {
        let identifier = format!("emu-guard-cluster-{index}");
        let mut params: Vec<(&str, String)> = vec![
            ("DBClusterIdentifier", identifier.clone()),
            ("Engine", "aurora-mysql".to_string()),
        ];
        if let Some(group) = row.group {
            params.push(("DBClusterParameterGroupName", group.to_string()));
        }
        if let Some(member) = row.unsupported {
            params.push((member, "true".to_string()));
        }
        let response = call(&router, "CreateDBCluster", &params).await;
        let body = expect_error(row.case, response, row.status, row.code);
        if row.status == 404 {
            let named = row.group.expect("a not-found row names a group");
            assert_eq!(
                text(&body, "Message"),
                format!("DBClusterParameterGroup not found: {named}"),
                "case {:?}: expected the not-found sentence naming the group the request \
                 spelled, got {body}",
                row.case
            );
        }
        // Either refusal stored nothing: the cluster the request named is not there.
        expect_error(
            row.case,
            call(
                &router,
                "DescribeDBClusters",
                &[("DBClusterIdentifier", identifier.as_str())],
            )
            .await,
            404,
            "DBClusterNotFoundFault",
        );
    }

    // The control: a group this region holds, and no unsupported member, is served.
    let served = expect_ok(
        "a held group and nothing unsupported",
        call(
            &router,
            "CreateDBCluster",
            &[
                ("DBClusterIdentifier", "emu-guard-cluster-served"),
                ("Engine", "aurora-mysql"),
                ("DBClusterParameterGroupName", "emu-held-cpg"),
            ],
        )
        .await,
    );
    assert_eq!(
        text(&served, "DBClusterParameterGroup"),
        "emu-held-cpg",
        "expected the cluster to carry the group the create named, got {served}"
    );
}

/// One row of the protected-parameter-group table: the operation, the member naming the
/// group, the name it carries and the answer it must get.
struct ProtectedGroupCase {
    case: &'static str,
    action: &'static str,
    member: &'static str,
    name: &'static str,
    status: u16,
    code: &'static str,
    message: &'static str,
}

/// The rows of that table: both modify operations, over the default groups of several
/// engine-version families (and one named in another case, since a group name is matched
/// without case), beside the names that are NOT RDS's own and are therefore still reported
/// as missing.
const PROTECTED_GROUP_CASES: &[ProtectedGroupCase] = &[
    ProtectedGroupCase {
        case: "the MySQL family's default DB parameter group",
        action: "ModifyDBParameterGroup",
        member: "DBParameterGroupName",
        name: "default.mysql8.0",
        status: 400,
        code: "InvalidParameterValue",
        message: CANNOT_MODIFY_DEFAULT,
    },
    ProtectedGroupCase {
        case: "the PostgreSQL family's default DB parameter group",
        action: "ModifyDBParameterGroup",
        member: "DBParameterGroupName",
        name: "default.postgres16",
        status: 400,
        code: "InvalidParameterValue",
        message: CANNOT_MODIFY_DEFAULT,
    },
    ProtectedGroupCase {
        case: "a default DB parameter group named in another case",
        action: "ModifyDBParameterGroup",
        member: "DBParameterGroupName",
        name: "Default.MySQL8.0",
        status: 400,
        code: "InvalidParameterValue",
        message: CANNOT_MODIFY_DEFAULT,
    },
    ProtectedGroupCase {
        case: "a name that is not one of RDS's own groups",
        action: "ModifyDBParameterGroup",
        member: "DBParameterGroupName",
        name: "emu-absent-pg",
        status: 404,
        code: "DBParameterGroupNotFound",
        message: "DBParameterGroup not found: emu-absent-pg",
    },
    ProtectedGroupCase {
        case: "the Aurora MySQL family's default DB cluster parameter group",
        action: "ModifyDBClusterParameterGroup",
        member: "DBClusterParameterGroupName",
        name: "default.aurora-mysql8.0",
        status: 400,
        code: "InvalidParameterValue",
        message: CANNOT_MODIFY_DEFAULT,
    },
    ProtectedGroupCase {
        case: "the Aurora PostgreSQL family's default DB cluster parameter group",
        action: "ModifyDBClusterParameterGroup",
        member: "DBClusterParameterGroupName",
        name: "default.aurora-postgresql15",
        status: 400,
        code: "InvalidParameterValue",
        message: CANNOT_MODIFY_DEFAULT,
    },
    ProtectedGroupCase {
        case: "a cluster group name that is not one of RDS's own groups",
        action: "ModifyDBClusterParameterGroup",
        member: "DBClusterParameterGroupName",
        name: "emu-absent-cluster-pg",
        status: 404,
        // The fault `ModifyDBClusterParameterGroup` models is the DB-parameter-group one,
        // so its wire code is that fault's.
        code: "DBParameterGroupNotFound",
        message: "DBClusterParameterGroup not found: emu-absent-cluster-pg",
    },
];

#[tokio::test]
async fn a_parameter_group_rds_provides_is_protected_rather_than_missing() {
    let router = emulator();
    for row in PROTECTED_GROUP_CASES {
        expect_refusal(
            row.case,
            call(
                &router,
                row.action,
                &[
                    (row.member, row.name),
                    ("Parameters.Parameter.1.ParameterName", "max_connections"),
                    ("Parameters.Parameter.1.ParameterValue", "100"),
                    ("Parameters.Parameter.1.ApplyMethod", "immediate"),
                ],
            )
            .await,
            row.status,
            row.code,
            row.message,
        );
    }
}

/// One row of the subscription-pair table: the call, the members it carries beyond the
/// subscription name, and the sentence it must be refused with.
struct SubscriptionPairCase {
    case: &'static str,
    action: &'static str,
    /// Whether the row names the subscription this suite created or one nothing created,
    /// which is what shows the pair is read before the subscription is resolved.
    held: bool,
    extra: &'static [(&'static str, &'static str)],
    message: &'static str,
}

/// The rows of that table: both calls that take `EventCategories`, each against a held and
/// an unheld subscription name, plus the `SourceIds` half of the same rule.
const SUBSCRIPTION_PAIR_CASES: &[SubscriptionPairCase] = &[
    SubscriptionPairCase {
        case: "ModifyEventSubscription: categories without a source type, on a held name",
        action: "ModifyEventSubscription",
        held: true,
        extra: &[("EventCategories.EventCategory.1", "availability")],
        message: CATEGORIES_NEED_A_SOURCE_TYPE,
    },
    SubscriptionPairCase {
        case: "ModifyEventSubscription: categories without a source type, on an unheld name",
        action: "ModifyEventSubscription",
        held: false,
        extra: &[("EventCategories.EventCategory.1", "availability")],
        message: CATEGORIES_NEED_A_SOURCE_TYPE,
    },
    SubscriptionPairCase {
        case: "ModifyEventSubscription: two categories without a source type",
        action: "ModifyEventSubscription",
        held: false,
        extra: &[
            ("EventCategories.EventCategory.1", "availability"),
            ("EventCategories.EventCategory.2", "backup"),
        ],
        message: CATEGORIES_NEED_A_SOURCE_TYPE,
    },
    SubscriptionPairCase {
        case: "CreateEventSubscription: categories without a source type",
        action: "CreateEventSubscription",
        held: false,
        extra: &[("EventCategories.EventCategory.1", "availability")],
        message: CATEGORIES_NEED_A_SOURCE_TYPE,
    },
    SubscriptionPairCase {
        case: "CreateEventSubscription: source identifiers without a source type",
        action: "CreateEventSubscription",
        held: false,
        extra: &[("SourceIds.SourceId.1", "emu-guard-source")],
        message: SOURCE_IDS_NEED_A_SOURCE_TYPE,
    },
];

/// An SNS topic ARN of the form `SnsTopicArn` takes.
const TOPIC_ARN: &str = "arn:aws:sns:us-west-2:000000000000:emu-guard-events";

#[tokio::test]
async fn subscription_categories_and_sources_are_refused_without_their_source_type() {
    let router = emulator();
    let held = "emu-guard-subscription";
    expect_ok(
        "create the held subscription",
        call(
            &router,
            "CreateEventSubscription",
            &[
                ("SubscriptionName", held),
                ("SnsTopicArn", TOPIC_ARN),
                ("SourceType", "db-instance"),
                ("EventCategories.EventCategory.1", "availability"),
            ],
        )
        .await,
    );

    for row in SUBSCRIPTION_PAIR_CASES {
        let name = if row.held {
            held.to_string()
        } else {
            format!("emu-guard-unheld-{}", row.extra.len())
        };
        let mut params: Vec<(&str, String)> = vec![("SubscriptionName", name.clone())];
        if row.action == "CreateEventSubscription" {
            params.push(("SnsTopicArn", TOPIC_ARN.to_string()));
        }
        params.extend(
            row.extra
                .iter()
                .map(|(member, value)| (*member, (*value).to_string())),
        );
        expect_refusal(
            row.case,
            call(&router, row.action, &params).await,
            400,
            "InvalidParameterCombination",
            row.message,
        );
    }

    // The refusals changed nothing: the held subscription still carries what its create
    // wrote, and no unheld name was created.
    let described = expect_ok(
        "the held subscription after the refusals",
        call(
            &router,
            "DescribeEventSubscriptions",
            &[("SubscriptionName", held)],
        )
        .await,
    );
    assert_eq!(
        text(&described, "SourceType"),
        "db-instance",
        "expected the held subscription to be unchanged, got {described}"
    );
    expect_error(
        "an unheld subscription name after the refusals",
        call(
            &router,
            "DescribeEventSubscriptions",
            &[("SubscriptionName", "emu-guard-unheld-1")],
        )
        .await,
        404,
        "SubscriptionNotFound",
    );
}

#[tokio::test]
async fn an_unpublished_event_source_type_is_refused_with_the_published_list() {
    let router = emulator();

    // The list the refusal names is the one the model's `SourceType` enum carries, so the
    // sentence this test pins and the set the service admits cannot drift apart: the two
    // are the same VALUES (the service lists them in its own order, which is why the
    // constant is not the generated one).
    let mut admitted: Vec<&str> = EVENT_SUBSCRIPTION_SOURCE_TYPES.to_vec();
    admitted.sort_unstable();
    let mut modelled: Vec<&str> = SOURCE_TYPE_VALUES.to_vec();
    modelled.sort_unstable();
    assert_eq!(
        admitted, modelled,
        "expected the admitted source types to be exactly the model's SourceType values, \
         got {admitted:?} against {modelled:?}"
    );
    for source_type in EVENT_SUBSCRIPTION_SOURCE_TYPES {
        assert!(
            INVALID_EVENT_SOURCE_TYPE.contains(&format!("'{source_type}'")),
            "expected the refusal to name {source_type:?}, got {INVALID_EVENT_SOURCE_TYPE:?}"
        );
    }

    // A value outside that set, each differing from it in another way: a type of the wider
    // enum `DescribeEvents` carries, a plural, a published type in the wrong case, a
    // resource type that is no event source, and a value that is no type at all.
    for (index, source_type) in [
        "db-replica",
        "db-instances",
        "DB-Instance",
        "db-subnet-group",
        "not-a-source-type",
    ]
    .into_iter()
    .enumerate()
    {
        let name = format!("emu-guard-type-{index}");
        expect_refusal(
            source_type,
            call(
                &router,
                "CreateEventSubscription",
                &[
                    ("SubscriptionName", name.as_str()),
                    ("SnsTopicArn", TOPIC_ARN),
                    ("SourceType", source_type),
                ],
            )
            .await,
            400,
            "InvalidParameterValue",
            INVALID_EVENT_SOURCE_TYPE,
        );
    }

    // Every type the set DOES carry is served, including the one the member's prose omits.
    for (index, source_type) in EVENT_SUBSCRIPTION_SOURCE_TYPES.iter().enumerate() {
        let name = format!("emu-guard-published-{index}");
        expect_ok(
            source_type,
            call(
                &router,
                "CreateEventSubscription",
                &[
                    ("SubscriptionName", name.as_str()),
                    ("SnsTopicArn", TOPIC_ARN),
                    ("SourceType", source_type),
                ],
            )
            .await,
        );
    }
}

/// One row of the identifier-wording table: the call, the member whose value breaks the
/// grammar, and whether RDS words that member's refusal by its own subject.
struct IdentifierCase {
    case: &'static str,
    action: &'static str,
    member: &'static str,
    value: &'static str,
    rest: &'static [(&'static str, &'static str)],
    /// The subject RDS names the member by, for the members it words that way.
    subject: Option<&'static str>,
}

/// The rows of that table.
///
/// The grammar is the one every identifier member of this service states ("an identifier
/// must begin with a letter and must contain only ASCII letters, digits, and hyphens. It
/// can't end with a hyphen or contain two consecutive hyphens"), and the rows span its
/// clauses — a trailing hyphen, two consecutive hyphens, a character outside the alphabet
/// and a leading digit — across the parameter-group names, a snapshot identifier and the
/// two members RDS words by their subject.
const IDENTIFIER_CASES: &[IdentifierCase] = &[
    IdentifierCase {
        case: "a DB parameter group name ending in a hyphen",
        action: "CreateDBParameterGroup",
        member: "DBParameterGroupName",
        value: "emu-guard-pg-",
        rest: &[
            ("DBParameterGroupFamily", "mysql8.0"),
            ("Description", "a-group-under-test"),
        ],
        subject: None,
    },
    IdentifierCase {
        case: "a DB parameter group name with two consecutive hyphens",
        action: "CreateDBParameterGroup",
        member: "DBParameterGroupName",
        value: "emu--guard-pg",
        rest: &[
            ("DBParameterGroupFamily", "mysql8.0"),
            ("Description", "a-group-under-test"),
        ],
        subject: None,
    },
    IdentifierCase {
        case: "a DB cluster parameter group name outside the alphabet",
        action: "CreateDBClusterParameterGroup",
        member: "DBClusterParameterGroupName",
        value: "emu_guard_cpg",
        rest: &[
            ("DBParameterGroupFamily", "aurora-mysql8.0"),
            ("Description", "a-group-under-test"),
        ],
        subject: None,
    },
    IdentifierCase {
        case: "a DB snapshot identifier starting with a digit",
        action: "CreateDBSnapshot",
        member: "DBSnapshotIdentifier",
        value: "9emu-guard-snap",
        rest: &[("DBInstanceIdentifier", "emu-guard-instance")],
        subject: None,
    },
    IdentifierCase {
        case: "a DB instance identifier, which RDS names by its subject",
        action: "CreateDBInstance",
        member: "DBInstanceIdentifier",
        value: "emu-guard-db-",
        rest: &[("Engine", "postgres"), ("DBInstanceClass", "db.m5.large")],
        subject: Some("database identifier"),
    },
    IdentifierCase {
        case: "a DB cluster identifier, which RDS names by its own subject",
        action: "CreateDBCluster",
        member: "DBClusterIdentifier",
        value: "1emu-guard-cl",
        rest: &[("Engine", "aurora-mysql")],
        subject: Some("database cluster identifier"),
    },
];

#[tokio::test]
async fn an_identifier_refusal_states_the_rule_and_names_the_member() {
    let router = emulator();
    for row in IDENTIFIER_CASES {
        let mut params: Vec<(&str, &str)> = vec![(row.member, row.value)];
        params.extend(row.rest.iter().copied());
        let expected = match row.subject {
            // The two spaces after the colon are part of RDS's own wording.
            Some(subject) => format!("Invalid {subject}:  {}", row.value),
            None => not_a_valid_identifier(row.member),
        };
        expect_refusal(
            row.case,
            call(&router, row.action, &params).await,
            400,
            "InvalidParameterValue",
            &expected,
        );
    }
}

/// One row of the blue/green source table: the ARN the request names and the kind of
/// database RDS says it is not a valid ARN of in this region.
struct SourceArnCase {
    case: &'static str,
    source: &'static str,
    kind: &'static str,
}

/// The rows of that table: a DB instance ARN and a DB cluster ARN, each of a region other
/// than the one the call is signed for.
const SOURCE_ARN_CASES: &[SourceArnCase] = &[
    SourceArnCase {
        case: "a DB instance ARN of another region",
        source: "arn:aws:rds:us-east-1:000000000000:db:emu-guard-other-region",
        kind: "DB Instance",
    },
    SourceArnCase {
        case: "a DB cluster ARN of another region",
        source: "arn:aws:rds:eu-west-1:000000000000:cluster:emu-guard-other-region",
        kind: "DB Cluster",
    },
];

#[tokio::test]
async fn a_blue_green_source_of_another_region_is_not_a_valid_arn_here() {
    let router = emulator();
    for (index, row) in SOURCE_ARN_CASES.iter().enumerate() {
        expect_refusal(
            row.case,
            call(
                &router,
                "CreateBlueGreenDeployment",
                &[
                    (
                        "BlueGreenDeploymentName",
                        format!("emu-guard-bg-{index}").as_str(),
                    ),
                    ("Source", row.source),
                ],
            )
            .await,
            400,
            "InvalidParameterValue",
            &format!("'{}' is not a valid {} ARN in this region", row.source, row.kind),
        );
    }
}
