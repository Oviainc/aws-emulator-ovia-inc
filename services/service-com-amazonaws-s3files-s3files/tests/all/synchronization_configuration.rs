// SPDX-License-Identifier: CC0-1.0

//! The synchronization configuration of an S3 File System, end to end over HTTP:
//! `PutSynchronizationConfiguration` and `GetSynchronizationConfiguration`, each asserted
//! against what the service PERSISTED rather than against what one request carried.
//!
//! The suite reuses `file_system_lifecycle`'s harness, so every case goes through the
//! same `MultiServiceRouter` the emulator boots: routing, the restJson1 wire shapes, the
//! modelled response statuses and the modelled error envelopes are all exercised.
//!
//! The rules under test, each stated once and then exercised over a SET of inputs:
//!
//! 1. An accepted put REPLACES the stored configuration with exactly the rules it
//!    carried — every member, in order — and a read answers those stored rules beside the
//!    version number the write recorded them under; each accepted write advances that
//!    number by one, starting at the first version.
//! 2. Optimistic concurrency, from `latestVersionNumber`'s own documentation: a
//!    first-time create names no version, a later write names the version the stored
//!    configuration currently carries, and a write that names anything else — a stale
//!    version, a version ahead of the stored one, no version at all, or a version on a
//!    file system that holds no configuration — is refused with the modelled
//!    `ConflictException` (409) having changed nothing, so of two writes naming one
//!    version only the first is accepted.
//! 3. `fileSystemId` accepts either form the model allows — the bare id or the file
//!    system's ARN — and both name the same configuration sub-resource.
//! 4. The configuration is a sub-resource: it does not exist until a put creates it, a
//!    read of an absent one — or of one on a file system this service does not hold — is
//!    the modelled `ResourceNotFoundException` (404), and it is released with its file
//!    system.
//! 5. A request outside a modelled constraint on the rules — the list lengths, the
//!    `ImportTrigger` value set, the `sizeLessThan` and `daysAfterLastAccess` ranges, the
//!    `prefix` pattern — is refused with the modelled `ValidationException` (400).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::file_system_lifecycle::{
    call, create_file_system, created_file_system_id, new_router, Answer, ROLE_ARN,
};

/// The version number the first accepted put records, from the service's own constant.
const FIRST_VERSION: i32 =
    service_com_amazonaws_s3files_s3files::state::FIRST_SYNCHRONIZATION_VERSION;

/// The `ImportTrigger` values the model declares, read from the service's own catalogue
/// of the modelled enum (`ops::types::IMPORT_TRIGGER_VALUES`, from the `ImportTrigger`
/// enum shape: `ON_DIRECTORY_FIRST_ACCESS | ON_FILE_ACCESS`).
const TRIGGERS: &[&str] = service_com_amazonaws_s3files_s3files::ops::types::IMPORT_TRIGGER_VALUES;

/// `ImportDataRule$sizeLessThan` is `@range(min: 0, max: 52673613135872)`
/// (smithy_query: shape ImportDataRule).
const SIZE_MIN: i64 = 0;
const SIZE_MAX: i64 = 52_673_613_135_872;

/// `ExpirationDataRule$daysAfterLastAccess` is `@range(min: 1, max: 365)`
/// (smithy_query: shape ExpirationDataRule).
const DAYS_MIN: i32 = 1;
const DAYS_MAX: i32 = 365;

/// `PutSynchronizationConfigurationRequest$importDataRules` is `@length(min: 1, max: 10)`
/// and `$expirationDataRules` is `@length(min: 1, max: 1)` (smithy_query: operation
/// PutSynchronizationConfiguration).
const MAX_IMPORT_RULES: usize = 10;

/// A well-formed `fs-` id (`FileSystemId`'s `^fs-[0-9a-f]{17,40}$`) that no create ever
/// minted, so it passes the boundary validator and reaches the handler's lookup.
const UNKNOWN_FILE_SYSTEM_ID: &str = "fs-0123456789abcdef0";

/// The route both operations are bound to:
/// `PUT|GET /file-systems/{fileSystemId}/synchronization-configuration`.
fn sync_route(label: &str) -> String {
    format!("/file-systems/{label}/synchronization-configuration")
}

/// One `ImportDataRule`: `prefix` matches `^(|.*/)$`, `trigger` is an `ImportTrigger`
/// value and `sizeLessThan` is a byte count in the modelled range.
fn import_rule(prefix: &str, trigger: &str, size_less_than: i64) -> Value {
    json!({ "prefix": prefix, "trigger": trigger, "sizeLessThan": size_less_than })
}

/// One `ExpirationDataRule`.
fn expiration_rule(days_after_last_access: i32) -> Value {
    json!({ "daysAfterLastAccess": days_after_last_access })
}

/// A configuration body: the two `@required` rule lists, plus `latestVersionNumber` when
/// the caller names one.
fn configuration(import_rules: Value, expiration_rules: Value, version: Option<i32>) -> Value {
    let mut body = json!({
        "importDataRules": import_rules,
        "expirationDataRules": expiration_rules,
    });
    if let Some(version) = version {
        body["latestVersionNumber"] = json!(version);
    }
    body
}

async fn put_sync(router: &MultiServiceRouter, label: &str, body: Value) -> Answer {
    call(router, "PUT", &sync_route(label), Some(body)).await
}

async fn get_sync(router: &MultiServiceRouter, label: &str) -> Answer {
    call(router, "GET", &sync_route(label), None).await
}

/// Assert a read reports exactly the rules that were put, in order, at `version`.
fn assert_reports(case: &str, read: &Answer, import_rules: &Value, expiration_rules: &Value, version: i32) {
    assert_eq!(
        read.status, 200,
        "case {case:?}: expected 200 from GetSynchronizationConfiguration, got {} / {:?}",
        read.status, read.body
    );
    assert_eq!(
        read.field("importDataRules"),
        import_rules,
        "case {case:?}: the stored import rules must be reported member for member and in \
         order, got {:?}",
        read.field("importDataRules")
    );
    assert_eq!(
        read.field("expirationDataRules"),
        expiration_rules,
        "case {case:?}: the stored expiration rules must be reported member for member, got {:?}",
        read.field("expirationDataRules")
    );
    assert_eq!(
        read.field("latestVersionNumber").as_i64(),
        Some(i64::from(version)),
        "case {case:?}: expected the stored version {version}, got {:?}",
        read.field("latestVersionNumber")
    );
}

/// The configurations rule 1 is asserted over: `(case, importDataRules,
/// expirationDataRules)`.
///
/// The rows differ along everything the rule generalises over — how many import rules the
/// list holds (one, several, the modelled maximum), which `ImportTrigger` each carries,
/// the form and depth of each `prefix`, where `sizeLessThan` and `daysAfterLastAccess`
/// sit in their published ranges (each bound and in between), and the ORDER the rules
/// arrive in.
fn configuration_table() -> Vec<(&'static str, Value, Value)> {
    vec![
        (
            "one rule at the low end of every range",
            json!([import_rule("", TRIGGERS[0], SIZE_MIN)]),
            json!([expiration_rule(DAYS_MIN)]),
        ),
        (
            "one rule at the published ceilings",
            json!([import_rule("data/", TRIGGERS[1], SIZE_MAX)]),
            json!([expiration_rule(DAYS_MAX)]),
        ),
        (
            "two rules, one per modelled trigger",
            json!([
                import_rule("logs/", TRIGGERS[0], 1),
                import_rule("logs/archive/", TRIGGERS[1], 1_048_576),
            ]),
            json!([expiration_rule(30)]),
        ),
        (
            "the same two rules in the opposite order",
            json!([
                import_rule("logs/archive/", TRIGGERS[1], 1_048_576),
                import_rule("logs/", TRIGGERS[0], 1),
            ]),
            json!([expiration_rule(30)]),
        ),
        (
            "a deep prefix and a non-ASCII one",
            json!([
                import_rule("a/b/c/d/e/", TRIGGERS[1], 4_096),
                import_rule("datensätze/", TRIGGERS[0], SIZE_MAX - 1),
            ]),
            json!([expiration_rule(180)]),
        ),
        (
            "the whole bucket and one scoped rule",
            json!([
                import_rule("", TRIGGERS[0], 512),
                import_rule("reports/2026/", TRIGGERS[1], 1_000_000_000),
            ]),
            json!([expiration_rule(DAYS_MAX - 1)]),
        ),
        (
            "ten rules — the ImportDataRuleList maximum",
            Value::Array(
                (0..MAX_IMPORT_RULES)
                    .map(|index| {
                        import_rule(
                            &format!("tier{index}/"),
                            TRIGGERS[index % TRIGGERS.len()],
                            (index as i64 + 1) * 1_024,
                        )
                    })
                    .collect(),
            ),
            json!([expiration_rule(7)]),
        ),
    ]
}

/// Rule 1: each accepted put replaces the stored configuration with the rules it carried
/// and advances the version by one; a read answers from what was stored.
#[tokio::test]
async fn an_accepted_put_is_persisted_and_read_back_at_its_version() {
    let router = new_router();
    let id = created_file_system_id(
        &router,
        "arn:aws:s3:::s3files-sync-roundtrip",
        Some("sync-roundtrip"),
    )
    .await;

    // The first write names no version; every later one names the version the previous
    // write recorded. The version a write lands at is its row's index from the first
    // version.
    let mut previous: Option<(Value, Value)> = None;
    for (version, (case, import_rules, expiration_rules)) in
        (FIRST_VERSION..).zip(configuration_table())
    {
        let named = (version != FIRST_VERSION).then(|| version - 1);
        let put = put_sync(
            &router,
            &id,
            configuration(import_rules.clone(), expiration_rules.clone(), named),
        )
        .await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: expected the modelled 200 from PutSynchronizationConfiguration, \
             got {} / {:?}",
            put.status, put.body
        );

        let read = get_sync(&router, &id).await;
        assert_reports(case, &read, &import_rules, &expiration_rules, version);

        // "Creates or updates": the previous rules are gone, not merged with.
        if let Some((earlier_import, earlier_expiration)) =
            previous.replace((import_rules.clone(), expiration_rules.clone()))
        {
            if earlier_import != import_rules {
                assert_ne!(
                    read.field("importDataRules"),
                    &earlier_import,
                    "case {case:?}: a put replaces the stored import rules, it does not keep \
                     the earlier ones"
                );
            }
            if earlier_expiration != expiration_rules {
                assert_ne!(
                    read.field("expirationDataRules"),
                    &earlier_expiration,
                    "case {case:?}: a put replaces the stored expiration rules"
                );
            }
        }

        // A read reports, it does not consume: a second read answers the same thing,
        // version included.
        let again = get_sync(&router, &id).await;
        assert_reports(case, &again, &import_rules, &expiration_rules, version);
    }
}

/// Rule 2: the optimistic-concurrency rule, over every way a write can name the wrong
/// version.
#[tokio::test]
async fn a_write_naming_the_wrong_version_is_the_modelled_conflict() {
    let router = new_router();
    let id = created_file_system_id(
        &router,
        "arn:aws:s3:::s3files-sync-optimistic-lock",
        Some("sync-optimistic-lock"),
    )
    .await;

    let import_rules = json!([import_rule("first/", TRIGGERS[0], 1_024)]);
    let expiration_rules = json!([expiration_rule(14)]);

    // A file system holding no configuration: naming any version at all is a conflict.
    for named in [FIRST_VERSION, FIRST_VERSION - 1, FIRST_VERSION + 5, 0, -1] {
        let refused = put_sync(
            &router,
            &id,
            configuration(import_rules.clone(), expiration_rules.clone(), Some(named)),
        )
        .await;
        assert_conflict(&format!("creating while naming version {named}"), &refused);
        let absent = get_sync(&router, &id).await;
        assert_eq!(
            absent.status, 404,
            "case \"creating while naming version {named}\": a refused create must have \
             stored nothing, got {} / {:?}",
            absent.status, absent.body
        );
    }

    // The first-time create, naming no version.
    let created = put_sync(
        &router,
        &id,
        configuration(import_rules.clone(), expiration_rules.clone(), None),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case \"the first-time create\": expected 200, got {} / {:?}",
        created.status, created.body
    );
    assert_reports(
        "the first-time create",
        &get_sync(&router, &id).await,
        &import_rules,
        &expiration_rules,
        FIRST_VERSION,
    );

    // Now a configuration exists at FIRST_VERSION. Every row below names something other
    // than that, so each must be refused and leave the stored configuration untouched.
    let rejected_rules = json!([import_rule("rejected/", TRIGGERS[1], 2_048)]);
    for (case, named) in [
        ("no version at all on an update", None),
        ("a stale version", Some(FIRST_VERSION - 1)),
        ("a version one ahead", Some(FIRST_VERSION + 1)),
        ("a far-ahead version", Some(FIRST_VERSION + 1_000)),
        ("zero", Some(0)),
        ("a negative version", Some(-7)),
        ("the largest Integer", Some(i32::MAX)),
    ] {
        let refused = put_sync(
            &router,
            &id,
            configuration(rejected_rules.clone(), expiration_rules.clone(), named),
        )
        .await;
        assert_conflict(case, &refused);
        assert_reports(
            case,
            &get_sync(&router, &id).await,
            &import_rules,
            &expiration_rules,
            FIRST_VERSION,
        );
    }

    // Two writes naming the SAME stored version: the first is accepted and moves the
    // version on, so the second — the one holding a now-stale view — is refused.
    let winner = json!([import_rule("winner/", TRIGGERS[0], 8_192)]);
    let loser = json!([import_rule("loser/", TRIGGERS[1], 8_192)]);
    let first = put_sync(
        &router,
        &id,
        configuration(winner.clone(), expiration_rules.clone(), Some(FIRST_VERSION)),
    )
    .await;
    assert_eq!(
        first.status, 200,
        "case \"the first of two writes on one version\": expected 200, got {} / {:?}",
        first.status, first.body
    );
    let second = put_sync(
        &router,
        &id,
        configuration(loser, expiration_rules.clone(), Some(FIRST_VERSION)),
    )
    .await;
    assert_conflict("the second of two writes on one version", &second);
    assert_reports(
        "the second of two writes on one version",
        &get_sync(&router, &id).await,
        &winner,
        &expiration_rules,
        FIRST_VERSION + 1,
    );
}

/// The modelled `ConflictException` (409).
fn assert_conflict(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 409,
        "case {case:?}: expected the modelled 409, got {} / {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_type, "ConflictException",
        "case {case:?}: expected the modelled ConflictException, got {:?} / {:?}",
        answer.error_type, answer.body
    );
}

/// Rule 3: the bare id and the file system's ARN name the same configuration.
#[tokio::test]
async fn either_id_form_names_the_same_configuration() {
    let router = new_router();
    let created = create_file_system(
        &router,
        json!({ "bucket": "arn:aws:s3:::s3files-sync-id-forms", "roleArn": ROLE_ARN }),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "expected 201 from CreateFileSystem, got {} / {:?}",
        created.status, created.body
    );
    let id = created.text("fileSystemId");
    // A non-greedy `@httpLabel` carries the ARN percent-encoded, which is how an SDK
    // puts a value containing `/` and `:` into a path segment.
    let encoded_arn = created
        .text("fileSystemArn")
        .replace(':', "%3A")
        .replace('/', "%2F");

    let import_rules = json!([import_rule("shared/", TRIGGERS[0], 65_536)]);
    let expiration_rules = json!([expiration_rule(90)]);
    // The write is addressed by the ARN; both reads below then have to find it.
    let put = put_sync(
        &router,
        &encoded_arn,
        configuration(import_rules.clone(), expiration_rules.clone(), None),
    )
    .await;
    assert_eq!(
        put.status, 200,
        "case \"the ARN\": a put addressed by the ARN must be accepted, got {} / {:?}",
        put.status, put.body
    );

    for (case, label) in [
        ("the bare id", id.as_str()),
        ("the ARN", encoded_arn.as_str()),
    ] {
        assert_reports(
            case,
            &get_sync(&router, label).await,
            &import_rules,
            &expiration_rules,
            FIRST_VERSION,
        );
    }

    // And an update addressed by the bare id lands on the same record the ARN created.
    let updated = json!([import_rule("shared/v2/", TRIGGERS[1], 131_072)]);
    let update = put_sync(
        &router,
        &id,
        configuration(updated.clone(), expiration_rules.clone(), Some(FIRST_VERSION)),
    )
    .await;
    assert_eq!(
        update.status, 200,
        "case \"the bare id\": expected the update to be accepted, got {} / {:?}",
        update.status, update.body
    );
    assert_reports(
        "the ARN reads the update the bare id wrote",
        &get_sync(&router, &encoded_arn).await,
        &updated,
        &expiration_rules,
        FIRST_VERSION + 1,
    );
}

/// Rule 4: the configuration is a sub-resource — absent until a put creates it, never
/// there for a file system this service does not hold, and released with its file system.
#[tokio::test]
async fn an_absent_configuration_is_the_modelled_not_found() {
    let router = new_router();
    let id = created_file_system_id(
        &router,
        "arn:aws:s3:::s3files-sync-absent",
        Some("sync-absent"),
    )
    .await;
    let body = configuration(
        json!([import_rule("absent/", TRIGGERS[0], 4_096)]),
        json!([expiration_rule(21)]),
        None,
    );

    for (case, answer) in [
        (
            "reading a configuration no put has created",
            get_sync(&router, &id).await,
        ),
        (
            "reading the configuration of an unknown file system",
            get_sync(&router, UNKNOWN_FILE_SYSTEM_ID).await,
        ),
        (
            "putting a configuration on an unknown file system",
            put_sync(&router, UNKNOWN_FILE_SYSTEM_ID, body.clone()).await,
        ),
    ] {
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404, got {} / {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_type, "ResourceNotFoundException",
            "case {case:?}: expected the modelled ResourceNotFoundException, got {:?} / {:?}",
            answer.error_type, answer.body
        );
    }

    // The configuration sub-resource has no id of its own, so its 404 carries one
    // constant, request-independent sentence naming what is missing.
    let absent = get_sync(&router, &id).await;
    assert_eq!(
        absent.text("message"),
        service_com_amazonaws_s3files_s3files::state::SYNCHRONIZATION_CONFIGURATION_NOT_FOUND_MESSAGE,
        "case \"reading a configuration no put has created\": expected the constant \
         configuration-not-found message, got {:?}",
        absent.body
    );

    // Created, then its file system deleted: the configuration goes with it.
    assert_eq!(
        put_sync(&router, &id, body.clone()).await.status,
        200,
        "case \"released with its file system\": expected the put to be accepted"
    );
    assert_eq!(
        get_sync(&router, &id).await.status,
        200,
        "case \"released with its file system\": the stored configuration must be readable"
    );
    let dropped = call(&router, "DELETE", &format!("/file-systems/{id}"), None).await;
    assert_eq!(
        dropped.status, 204,
        "case \"released with its file system\": expected DeleteFileSystem to succeed, got {} / {:?}",
        dropped.status, dropped.body
    );
    let orphan = get_sync(&router, &id).await;
    assert_eq!(
        orphan.status, 404,
        "case \"released with its file system\": a deleted file system holds no \
         configuration, got {} / {:?}",
        orphan.status, orphan.body
    );
}

/// Rule 5: a request outside a modelled constraint on the rules is refused with the
/// modelled `ValidationException`, having changed nothing.
///
/// Each row names the constraint it steps past; the range rows sit one past a published
/// bound, which the accepted table above exercises from the inside.
#[tokio::test]
async fn a_configuration_outside_a_modelled_constraint_is_refused() {
    let router = new_router();
    let id = created_file_system_id(
        &router,
        "arn:aws:s3:::s3files-sync-validation",
        Some("sync-validation"),
    )
    .await;

    // A configuration that IS valid, so every refusal below can be shown to have changed
    // nothing.
    let import_rules = json!([import_rule("baseline/", TRIGGERS[0], 1_024)]);
    let expiration_rules = json!([expiration_rule(10)]);
    let put = put_sync(
        &router,
        &id,
        configuration(import_rules.clone(), expiration_rules.clone(), None),
    )
    .await;
    assert_eq!(
        put.status, 200,
        "expected the baseline put to be accepted, got {} / {:?}",
        put.status, put.body
    );

    let eleven_rules: Vec<Value> = (0..=MAX_IMPORT_RULES)
        .map(|index| import_rule(&format!("over{index}/"), TRIGGERS[0], 1_024))
        .collect();
    for (case, import_list, expiration_list) in [
        (
            "an empty importDataRules list",
            json!([]),
            expiration_rules.clone(),
        ),
        (
            "one import rule past the list maximum",
            Value::Array(eleven_rules),
            expiration_rules.clone(),
        ),
        (
            "an empty expirationDataRules list",
            import_rules.clone(),
            json!([]),
        ),
        (
            "two expiration rules, one past that list's maximum",
            import_rules.clone(),
            json!([expiration_rule(10), expiration_rule(20)]),
        ),
        (
            "a trigger outside the ImportTrigger value set",
            json!([import_rule("x/", "ON_EVERY_TUESDAY", 1_024)]),
            expiration_rules.clone(),
        ),
        (
            "sizeLessThan one below its minimum",
            json!([import_rule("x/", TRIGGERS[0], SIZE_MIN - 1)]),
            expiration_rules.clone(),
        ),
        (
            "sizeLessThan one above its maximum",
            json!([import_rule("x/", TRIGGERS[0], SIZE_MAX + 1)]),
            expiration_rules.clone(),
        ),
        (
            "daysAfterLastAccess one below its minimum",
            import_rules.clone(),
            json!([expiration_rule(DAYS_MIN - 1)]),
        ),
        (
            "daysAfterLastAccess one above its maximum",
            import_rules.clone(),
            json!([expiration_rule(DAYS_MAX + 1)]),
        ),
        (
            "a prefix that does not end in a slash",
            json!([import_rule("no-trailing-slash", TRIGGERS[0], 1_024)]),
            expiration_rules.clone(),
        ),
    ] {
        let refused = put_sync(
            &router,
            &id,
            configuration(import_list, expiration_list, Some(FIRST_VERSION)),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected 400, got {} / {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type, "ValidationException",
            "case {case:?}: expected the modelled ValidationException, got {:?} / {:?}",
            refused.error_type, refused.body
        );
        assert_reports(
            case,
            &get_sync(&router, &id).await,
            &import_rules,
            &expiration_rules,
            FIRST_VERSION,
        );
    }

    // `fileSystemId` that is neither a minted id nor an ARN is refused by the handler's
    // own resolution rather than stored under.
    let malformed = put_sync(
        &router,
        "fs-nothex",
        configuration(import_rules.clone(), expiration_rules.clone(), None),
    )
    .await;
    assert_eq!(
        malformed.status, 400,
        "case \"a fileSystemId that is neither an id nor an ARN\": expected 400, got {} / {:?}",
        malformed.status, malformed.body
    );
}
