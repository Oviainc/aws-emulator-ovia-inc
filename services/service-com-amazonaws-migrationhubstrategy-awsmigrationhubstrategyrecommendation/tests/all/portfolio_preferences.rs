// SPDX-License-Identifier: CC0-1.0

//! The account's portfolio preferences, saved and read back end to end over HTTP.
//!
//! The rule under test is the one the two operations state for ANY preferences, not for
//! one request: `PutPortfolioPreferences` SAVES the preferences the request carries, so
//! the account's one preferences record becomes exactly that request — every member it
//! names and no member it does not — and `GetPortfolioPreferences` reports that record
//! member for member, until a later save replaces it. An account that has never saved
//! holds no such record, so the read reports the absent resource with 404
//! `ResourceNotFoundException`; and a value an enumeration of the request does not declare
//! is refused with 400 `ValidationException`, leaving the saved record as it was.
//!
//! So the tables below span the class the rule generalises over rather than the one
//! recorded put/get pair: every `ApplicationMode` value, every `ManagementPreference` and
//! `DatabaseMigrationPreference` variant with every target its own enumeration declares,
//! every `DatabaseManagementPreference` value, each `BusinessGoals` member at each
//! published bound of its rank, a request carrying all four preference members at once and
//! one carrying none. Each round trip asserts the WHOLE reported record against the
//! request, so a member dropped, defaulted or carried over from a previous save fails
//! here.
//!
//! Every catalog below is spelled out from the model rather than from the crate's
//! constants, so a wrong value list in `src/` fails here instead of agreeing with itself.

use serde_json::{json, Value};

use crate::import_and_assess::{call, emulator, Answer};

/// Every value `ApplicationMode` admits, from
/// `smithy_query(query="closure",operation="PutPortfolioPreferences")` -> `ApplicationMode`.
const APPLICATION_MODES: &[&str] = &["ALL", "KNOWN", "UNKNOWN"];

/// Every value `AwsManagedTargetDestination` admits (same closure).
const AWS_MANAGED_TARGET_DESTINATIONS: &[&str] =
    &["None specified", "AWS Elastic BeanStalk", "AWS Fargate"];

/// Every value `SelfManageTargetDestination` admits.
const SELF_MANAGE_TARGET_DESTINATIONS: &[&str] = &[
    "None specified",
    "Amazon Elastic Cloud Compute (EC2)",
    "Amazon Elastic Container Service (ECS)",
    "Amazon Elastic Kubernetes Service (EKS)",
];

/// Every value `NoPreferenceTargetDestination` admits.
const NO_PREFERENCE_TARGET_DESTINATIONS: &[&str] = &[
    "None specified",
    "AWS Elastic BeanStalk",
    "AWS Fargate",
    "Amazon Elastic Cloud Compute (EC2)",
    "Amazon Elastic Container Service (ECS)",
    "Amazon Elastic Kubernetes Service (EKS)",
];

/// Every value `DatabaseManagementPreference` admits.
const DATABASE_MANAGEMENT_PREFERENCES: &[&str] = &["AWS-managed", "Self-manage", "No preference"];

/// Every value `HeterogeneousTargetDatabaseEngine` admits; `TargetDatabaseEngine` (the
/// `noPreference` variant's list) declares the same set.
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

/// The one value `HomogeneousTargetDatabaseEngine` admits.
const HOMOGENEOUS_TARGET_DATABASE_ENGINES: &[&str] = &["None specified"];

/// Every member of `BusinessGoals`, each targeting `BusinessGoalsInteger` `@range(1, 5)`.
const BUSINESS_GOALS: &[&str] = &[
    "speedOfMigration",
    "reduceOperationalOverheadWithManagedServices",
    "modernizeInfrastructureWithCloudNativeTechnologies",
    "licenseCostReduction",
];

/// The published bounds of a `BusinessGoals` rank.
const RANK_MIN: i64 = 1;
const RANK_MAX: i64 = 5;

/// This service's one message for a value that fails a rule the model declares.
const INVALID_REQUEST_BODY: &str = "Invalid request body";

async fn put_preferences(
    router: &smithy_server_runtime::MultiServiceRouter,
    body: Value,
) -> Answer {
    call(router, "POST", "/put-portfolio-preferences", Some(body)).await
}

async fn get_preferences(router: &smithy_server_runtime::MultiServiceRouter) -> Answer {
    call(router, "GET", "/get-portfolio-preferences", None).await
}

/// The record the read reports, as JSON. A record with no member set serializes to an
/// empty object, which an empty body and a literal `{}` both mean.
fn reported(answer: &Answer) -> Value {
    if answer.body.is_null() {
        json!({})
    } else {
        answer.body.clone()
    }
}

/// An `applicationPreferences` naming one management preference variant and one target.
fn application_preferences(variant: &str, destination: &str) -> Value {
    json!({
        "applicationPreferences": {
            "managementPreference": {variant: {"targetDestination": [destination]}}
        }
    })
}

/// A `databasePreferences` naming one migration preference variant and one engine.
fn database_migration(variant: &str, engine: &str) -> Value {
    json!({
        "databasePreferences": {
            "databaseMigrationPreference": {variant: {"targetDatabaseEngine": [engine]}}
        }
    })
}

/// A request carrying all four preference members at once — the widest record the
/// operation admits.
fn every_member() -> Value {
    json!({
        "applicationMode": APPLICATION_MODES[1],
        "applicationPreferences": {
            "managementPreference": {
                "selfManageResources": {"targetDestination": [SELF_MANAGE_TARGET_DESTINATIONS[3]]}
            }
        },
        "databasePreferences": {
            "databaseManagementPreference": DATABASE_MANAGEMENT_PREFERENCES[0],
            "databaseMigrationPreference": {
                "heterogeneous": {"targetDatabaseEngine": [TARGET_DATABASE_ENGINES[9]]}
            }
        },
        "prioritizeBusinessGoals": {
            "businessGoals": {
                "speedOfMigration": RANK_MIN,
                "reduceOperationalOverheadWithManagedServices": 2,
                "modernizeInfrastructureWithCloudNativeTechnologies": 3,
                "licenseCostReduction": RANK_MAX,
            }
        },
    })
}

/// Every save this suite round-trips: what the row probes, and the request that saves it.
///
/// The rows differ along what the rule generalises over — which of the four members are
/// present at all, which union variant each preference names, which value of that
/// variant's own enumeration it carries, how many business goals are ranked and at which
/// rank — so no row is a rename of another.
fn saved_cases() -> Vec<(String, Value)> {
    let mut cases: Vec<(String, Value)> = Vec::new();

    cases.push(("no member at all".to_string(), json!({})));

    for mode in APPLICATION_MODES {
        cases.push((
            format!("applicationMode {mode:?}"),
            json!({"applicationMode": mode}),
        ));
    }

    for destination in AWS_MANAGED_TARGET_DESTINATIONS {
        cases.push((
            format!("awsManagedResources to {destination:?}"),
            application_preferences("awsManagedResources", destination),
        ));
    }
    for destination in SELF_MANAGE_TARGET_DESTINATIONS {
        cases.push((
            format!("selfManageResources to {destination:?}"),
            application_preferences("selfManageResources", destination),
        ));
    }
    for destination in NO_PREFERENCE_TARGET_DESTINATIONS {
        cases.push((
            format!("management noPreference to {destination:?}"),
            application_preferences("noPreference", destination),
        ));
    }

    for preference in DATABASE_MANAGEMENT_PREFERENCES {
        cases.push((
            format!("databaseManagementPreference {preference:?}"),
            json!({"databasePreferences": {"databaseManagementPreference": preference}}),
        ));
    }
    for engine in TARGET_DATABASE_ENGINES {
        cases.push((
            format!("heterogeneous migration to {engine:?}"),
            database_migration("heterogeneous", engine),
        ));
        cases.push((
            format!("migration noPreference to {engine:?}"),
            database_migration("noPreference", engine),
        ));
    }
    for engine in HOMOGENEOUS_TARGET_DATABASE_ENGINES {
        cases.push((
            format!("homogeneous migration to {engine:?}"),
            database_migration("homogeneous", engine),
        ));
    }
    // `Homogeneous$targetDatabaseEngine` is the one of these lists the model admits empty
    // (`@length(min: 0, max: 1)`), and the one it leaves optional.
    cases.push((
        "homogeneous migration naming no engine".to_string(),
        json!({
            "databasePreferences": {"databaseMigrationPreference": {"homogeneous": {}}}
        }),
    ));

    for goal in BUSINESS_GOALS {
        for rank in [RANK_MIN, RANK_MAX] {
            cases.push((
                format!("{goal} ranked {rank}, a published bound"),
                json!({"prioritizeBusinessGoals": {"businessGoals": {(*goal): rank}}}),
            ));
        }
    }

    cases.push(("all four preference members".to_string(), every_member()));
    cases
}

/// A save stores exactly the request, and the read reports exactly the save.
///
/// The rows run against ONE emulator in sequence, which is also what pins the replacement
/// half of the rule: each row asserts the whole reported record equals ITS request, so a
/// member a previous row saved and this one does not name has to be gone.
#[tokio::test]
async fn every_saved_preference_is_reported_back_member_for_member() {
    let router = emulator();

    for (case, request) in saved_cases() {
        let saved = put_preferences(&router, request.clone()).await;
        assert_eq!(
            saved.status, 200,
            "case {case:?}: expected PutPortfolioPreferences to answer HTTP 200, got {} with {:?}",
            saved.status, saved.body
        );

        let got = get_preferences(&router).await;
        assert_eq!(
            got.status, 200,
            "case {case:?}: expected GetPortfolioPreferences to answer HTTP 200, got {} with {:?}",
            got.status, got.body
        );
        assert_eq!(
            reported(&got),
            request,
            "case {case:?}: expected the saved record {request:?}, got {:?}",
            reported(&got)
        );
    }
}

/// Reading the preferences does not change what a later caller sees: the same record
/// comes back however many times it is read, and a read before any save does not create
/// one.
#[tokio::test]
async fn reading_the_preferences_changes_nothing() {
    let router = emulator();

    let before = get_preferences(&router).await;
    assert_eq!(
        before.status, 404,
        "case \"read before any save\": expected HTTP 404, got {} with {:?}",
        before.status, before.body
    );
    // The read must not have minted a record: the next read answers the same way.
    let again = get_preferences(&router).await;
    assert_eq!(
        again.status, 404,
        "case \"second read before any save\": expected HTTP 404, got {} with {:?}",
        again.status, again.body
    );

    let request = every_member();
    let saved = put_preferences(&router, request.clone()).await;
    assert_eq!(
        saved.status, 200,
        "case \"save before repeated reads\": expected HTTP 200, got {} with {:?}",
        saved.status, saved.body
    );

    for read in 1..=3 {
        let got = get_preferences(&router).await;
        assert_eq!(
            reported(&got),
            request,
            "case \"read {read} of the same record\": expected {request:?}, got {:?}",
            reported(&got)
        );
    }
}

/// An account that has never saved preferences has no record to report, so the read
/// answers the modelled not-found rather than a 200 of absent members.
#[tokio::test]
async fn an_account_with_no_saved_preferences_is_reported_as_not_found() {
    let router = emulator();

    let got = get_preferences(&router).await;
    assert_eq!(
        got.status, 404,
        "case \"fresh account\": expected HTTP 404, got {} with {:?}",
        got.status, got.body
    );
    assert_eq!(
        got.error_code(),
        "ResourceNotFoundException",
        "case \"fresh account\": expected ResourceNotFoundException, got {:?}",
        got.body
    );
    assert!(
        !got.string("/message").is_empty(),
        "case \"fresh account\": the not-found carried no message: {:?}",
        got.body
    );
}

/// A save replaces the whole record rather than merging into it, in both directions:
/// a narrower request drops the members it does not name, and a wider one sets them.
#[tokio::test]
async fn a_save_replaces_the_whole_record() {
    let router = emulator();

    let wide = every_member();
    let narrow = json!({"applicationMode": APPLICATION_MODES[2]});

    for (case, request) in [
        ("the widest record", wide.clone()),
        ("a record naming one member", narrow.clone()),
        ("the widest record again", wide),
        ("a record naming nothing", json!({})),
        ("one member once more", narrow),
    ] {
        let saved = put_preferences(&router, request.clone()).await;
        assert_eq!(
            saved.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {:?}",
            saved.status, saved.body
        );
        let got = get_preferences(&router).await;
        assert_eq!(
            reported(&got),
            request,
            "case {case:?}: a save must replace the record, expected {request:?}, got {:?}",
            reported(&got)
        );
    }
}

/// Requests whose values no enumeration of the request declares, or whose choice list
/// carries more entries than the model admits. Each row differs in WHERE the undeclared
/// value sits, so nothing about the refusal can depend on one member.
fn refused_cases() -> Vec<(String, Value)> {
    let mut cases: Vec<(String, Value)> = vec![
        (
            "an applicationMode ApplicationMode does not declare".to_string(),
            json!({"applicationMode": "SOME"}),
        ),
        (
            "an applicationMode differing only in case".to_string(),
            json!({"applicationMode": "all"}),
        ),
        (
            "a databaseManagementPreference the model does not declare".to_string(),
            json!({"databasePreferences": {"databaseManagementPreference": "Managed"}}),
        ),
        (
            // A value another variant DOES declare, which is what makes the per-variant
            // enumeration the rule rather than one shared list.
            "awsManagedResources to a destination only selfManageResources declares"
                .to_string(),
            application_preferences("awsManagedResources", SELF_MANAGE_TARGET_DESTINATIONS[1]),
        ),
        (
            "selfManageResources to a destination only awsManagedResources declares".to_string(),
            application_preferences("selfManageResources", AWS_MANAGED_TARGET_DESTINATIONS[2]),
        ),
        (
            "homogeneous migration to an engine only heterogeneous declares".to_string(),
            database_migration("homogeneous", TARGET_DATABASE_ENGINES[1]),
        ),
        (
            "heterogeneous migration to an engine no list declares".to_string(),
            database_migration("heterogeneous", "Amazon Redshift"),
        ),
        (
            // Every one of these lists is bounded to a single entry.
            "two target destinations where the model admits one".to_string(),
            json!({
                "applicationPreferences": {"managementPreference": {"noPreference": {
                    "targetDestination": [
                        NO_PREFERENCE_TARGET_DESTINATIONS[0],
                        NO_PREFERENCE_TARGET_DESTINATIONS[1],
                    ]
                }}}
            }),
        ),
        (
            "an empty target destination list where the model requires one".to_string(),
            json!({
                "applicationPreferences": {
                    "managementPreference": {"awsManagedResources": {"targetDestination": []}}
                }
            }),
        ),
    ];

    // The rank of a business goal is `@range(1, 5)`: one past each published bound.
    for goal in BUSINESS_GOALS {
        for rank in [RANK_MIN - 1, RANK_MAX + 1] {
            cases.push((
                format!("{goal} ranked {rank}, past a published bound"),
                json!({"prioritizeBusinessGoals": {"businessGoals": {(*goal): rank}}}),
            ));
        }
    }
    cases
}

/// A refused save leaves the account's saved record exactly as it was — and is refused
/// with the service's 400 `ValidationException`, not stored and reported back.
#[tokio::test]
async fn a_value_the_model_does_not_declare_is_refused_and_saves_nothing() {
    let router = emulator();

    let established = every_member();
    let saved = put_preferences(&router, established.clone()).await;
    assert_eq!(
        saved.status, 200,
        "case \"the record a refusal must not disturb\": expected HTTP 200, got {} with {:?}",
        saved.status, saved.body
    );

    for (case, request) in refused_cases() {
        let refused = put_preferences(&router, request.clone()).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            refused.body
        );
        assert_eq!(
            refused.string("/message"),
            INVALID_REQUEST_BODY,
            "case {case:?}: expected the service's {INVALID_REQUEST_BODY:?}, got {:?}",
            refused.body
        );

        let got = get_preferences(&router).await;
        assert_eq!(
            reported(&got),
            established,
            "case {case:?}: a refused save changed the record, expected {established:?}, got {:?}",
            reported(&got)
        );
    }
}
