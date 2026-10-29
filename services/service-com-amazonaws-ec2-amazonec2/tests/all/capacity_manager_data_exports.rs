// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteCapacityManagerDataExport` — and for the data export id
//! space it shares with the rest of the Capacity Manager surface — driven over the wire the
//! service actually speaks: `ec2Query`, an `application/x-www-form-urlencoded` POST whose
//! body carries `Action=DeleteCapacityManagerDataExport`, answered with XML (README.md /
//! OPERATIONS.md "Wire protocol"). ec2Query is selected by the `ec2` signing name in the
//! request's SigV4 credential scope, so every call here carries one.
//!
//! Four rules are under test:
//!
//! 1. THE DELETE TAKES THE CONFIGURATION OUT OF THE STORE: it answers the id of the
//!    configuration it removed, that id is gone from the account afterwards, every other
//!    configuration is still held, and the count `GetCapacityManagerAttributes` reports
//!    drops by exactly one — so the answer reports a removal that happened rather than one
//!    the handler assumed.
//! 2. A WELL-FORMED ID THE ACCOUNT HOLDS NO CONFIGURATION FOR IS REFUSED, not answered as a
//!    no-op: HTTP 400 `InvalidCapacityManagerDataExportId.NotFound`, worded
//!    `The capacity-manager-data-export ID '<id>' does not exist` from the id the request
//!    sent, and nothing else the account holds is touched.
//! 3. A VALUE THAT IS NOT AN EXPORT ID AT ALL IS A BAD ID, NOT A MISSING RESOURCE: HTTP 400
//!    `InvalidCapacityManagerDataExportId.Malformed` with this service's one unreadable-id
//!    sentence, refused BEFORE any lookup — so the refusal does not depend on what the
//!    account holds.
//! 4. `DryRun` ASKS ABOUT THE CALLER: it is answered HTTP 412 `DryRunOperation` and removes
//!    nothing, for a configuration the account holds and for a well-formed id it does not,
//!    while a value that is not an id at all is still refused as malformed.
//!
//! Every table row is a distinct input along what those rules generalise over — how many
//! configurations the account holds and which one is named, which id form the request
//! carries (the long form, the short form, the all-zero id), the WAY a value falls outside
//! that form, and where the `DryRun` falls against the lookup — so no single recorded call
//! is the specification. `CreateCapacityManagerDataExport` is still a scaffold in this
//! emulator, so the configurations a row needs are written straight into the store the way
//! that operation will, as [`super::image_usage_reports`] seeds AMI usage reports.

use std::time::{Duration, UNIX_EPOCH};

use ec2::ops::types::{CapacityManagerDataExportResponse, Tag};
use ec2::state::{
    CAPACITY_MANAGER_DATA_EXPORT_ID_PREFIX, ComAmazonawsEc2Amazonec2State as Ec2State,
    INVALID_CAPACITY_MANAGER_DATA_EXPORT_ID_MALFORMED,
    INVALID_CAPACITY_MANAGER_DATA_EXPORT_ID_NOT_FOUND, MAX_ECHOED_RESOURCE_ID_CHARS,
    system_time_to_date_time,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, error_code, error_message};

/// The Region every call here is signed for. A data export configuration is a fact of the
/// ACCOUNT's Capacity Manager and carries no Region member, so the Region only selects the
/// ec2Query envelope.
const REGION: &str = "us-east-1";

/// The status EC2 answers a refusal about the request with.
const REFUSAL_STATUS: u16 = 400;

/// The status EC2 answers a `DryRun` request that would have succeeded with.
const DRY_RUN_STATUS: u16 = 412;

/// One seeded data export configuration. The fixtures differ along every member the
/// modelled `CapacityManagerDataExportResponse` carries, so a delete that removed the wrong
/// one, or that left a projection of one behind, is visible.
struct ExportFixture {
    s3_bucket_name: &'static str,
    s3_bucket_prefix: Option<&'static str>,
    /// `Schedule`'s one member — `hourly`
    /// (`smithy_query(query="shape",shape="Schedule")`).
    schedule: &'static str,
    /// An `OutputFormat` member — `csv` or `parquet`
    /// (`smithy_query(query="shape",shape="OutputFormat")`).
    output_format: &'static str,
    created_at_millis: u64,
    /// A `CapacityManagerDataExportStatus` member, or `None` for a configuration no export
    /// has run for yet
    /// (`smithy_query(query="shape",shape="CapacityManagerDataExportStatus")`).
    latest_delivery_status: Option<&'static str>,
    tags: Vec<(&'static str, &'static str)>,
}

/// The configurations the tables below are written against: four, differing in bucket, in
/// whether a key prefix is set, in output format, in whether a delivery has run, and in how
/// many tags they carry.
fn seeded_fixtures() -> Vec<ExportFixture> {
    vec![
        ExportFixture {
            s3_bucket_name: "capacity-exports-one",
            s3_bucket_prefix: Some("hourly/"),
            schedule: "hourly",
            output_format: "csv",
            // 2026-01-01T00:00:00Z.
            created_at_millis: 1_767_225_600_000,
            latest_delivery_status: Some("delivered"),
            tags: vec![("team", "capacity"), ("cost-centre", "4711")],
        },
        ExportFixture {
            s3_bucket_name: "capacity-exports-two",
            s3_bucket_prefix: None,
            schedule: "hourly",
            output_format: "parquet",
            // 2026-02-01T00:00:00Z.
            created_at_millis: 1_769_904_000_000,
            latest_delivery_status: Some("pending"),
            tags: Vec::new(),
        },
        ExportFixture {
            s3_bucket_name: "capacity-exports-three",
            s3_bucket_prefix: Some("exports/capacity/"),
            schedule: "hourly",
            output_format: "parquet",
            // 2026-03-01T12:00:00Z.
            created_at_millis: 1_772_366_400_000,
            latest_delivery_status: None,
            tags: vec![("team", "platform")],
        },
        ExportFixture {
            s3_bucket_name: "capacity-exports-four",
            s3_bucket_prefix: None,
            schedule: "hourly",
            output_format: "csv",
            // 2026-03-02T12:00:00Z.
            created_at_millis: 1_772_452_800_000,
            latest_delivery_status: Some("failed"),
            tags: Vec::new(),
        },
    ]
}

/// Write one configuration into the engine's store, the way
/// `CreateCapacityManagerDataExport` will, and answer the id it was given.
fn seed_one(guard: &mut Ec2State, fixture: &ExportFixture) -> String {
    let export_id = guard.next_id(CAPACITY_MANAGER_DATA_EXPORT_ID_PREFIX);
    let created_at = system_time_to_date_time(UNIX_EPOCH + Duration::from_millis(fixture.created_at_millis));
    guard.capacity_manager_data_exports.insert(
        export_id.clone(),
        CapacityManagerDataExportResponse {
            capacity_manager_data_export_id: Some(export_id.clone()),
            s3_bucket_name: Some(fixture.s3_bucket_name.to_string()),
            s3_bucket_prefix: fixture.s3_bucket_prefix.map(str::to_string),
            schedule: Some(fixture.schedule.to_string()),
            output_format: Some(fixture.output_format.to_string()),
            create_time: Some(created_at),
            latest_delivery_status: fixture.latest_delivery_status.map(str::to_string),
            latest_delivery_status_message: None,
            latest_delivery_s3_location_uri: fixture.latest_delivery_status.map(|_| {
                format!("s3://{}/capacity-export.csv", fixture.s3_bucket_name)
            }),
            latest_delivery_time: fixture.latest_delivery_status.map(|_| created_at),
            tags: Some(
                fixture
                    .tags
                    .iter()
                    .map(|(key, value)| Tag {
                        key: Some((*key).to_string()),
                        value: Some((*value).to_string()),
                    })
                    .collect(),
            ),
        },
    );
    export_id
}

/// Write the first `count` seeded configurations into the store, answering the ids they were
/// given in store order.
fn seed(engine: &engine::InMemoryEngine, count: usize) -> Vec<String> {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    seeded_fixtures()
        .iter()
        .take(count)
        .map(|fixture| seed_one(&mut guard, fixture))
        .collect()
}

/// Every configuration id the store holds, in store order.
fn stored_ids(engine: &engine::InMemoryEngine) -> Vec<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .capacity_manager_data_exports
        .keys()
        .cloned()
        .collect()
}

/// One `DeleteCapacityManagerDataExport` call naming `export_id`, with whatever extra
/// parameters the row needs.
async fn delete(
    router: &MultiServiceRouter,
    export_id: &str,
    extra: &[(&str, &str)],
) -> (u16, String) {
    let mut params = vec![(
        "CapacityManagerDataExportId".to_string(),
        export_id.to_string(),
    )];
    for (name, value) in extra {
        params.push(((*name).to_string(), (*value).to_string()));
    }
    call(router, REGION, "DeleteCapacityManagerDataExport", &params).await
}

/// The number of data export configurations `GetCapacityManagerAttributes` reports for the
/// account — the reader the delete is round-tripped through.
async fn reported_count(router: &MultiServiceRouter, case: &str) -> Option<String> {
    let (status, body) = call(router, REGION, "GetCapacityManagerAttributes", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from GetCapacityManagerAttributes, got {status} \
         with body {body}"
    );
    element(&body, "dataExportCount")
}

/// The sentence this service refuses an unreadable resource id with, for `value`.
fn malformed_message(value: &str) -> String {
    let echoed: String = value.chars().take(MAX_ECHOED_RESOURCE_ID_CHARS).collect();
    format!("Invalid id: \"{echoed}\"")
}

/// The sentence EC2 refuses a data export id that names no configuration with, for `id`.
fn not_found_message(id: &str) -> String {
    let echoed: String = id.chars().take(MAX_ECHOED_RESOURCE_ID_CHARS).collect();
    format!("The capacity-manager-data-export ID '{echoed}' does not exist")
}

// ---------------------------------------------------------------------------
// RULE 1: the delete takes the configuration out of the store
// ---------------------------------------------------------------------------

/// Whichever configuration of however many the request names, the delete answers that id,
/// the account no longer holds it, every other configuration is still held, and the reader's
/// count drops by one.
#[tokio::test]
async fn the_delete_removes_exactly_the_configuration_the_request_names() {
    // `(case, how many configurations the account holds, which of them is deleted)`.
    let rows: [(&str, usize, usize); 5] = [
        ("the only configuration of the account", 1, 0),
        ("the first of two", 2, 0),
        ("the second of two", 2, 1),
        ("the middle of four", 4, 1),
        ("the last of four", 4, 3),
    ];

    for (case, held, position) in rows {
        let (router, engine) = super::ec2_query::emulator();
        let seeded = seed(&engine, held);
        let target = seeded[position].clone();

        let (status, body) = delete(&router, &target, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 deleting {target}, got {status} with body {body}"
        );
        assert_eq!(
            element(&body, "capacityManagerDataExportId").as_deref(),
            Some(target.as_str()),
            "case {case:?}: expected the deleted id {target:?} in the answer, got {body}"
        );

        let expected: Vec<String> = seeded
            .iter()
            .filter(|id| **id != target)
            .cloned()
            .collect();
        let remaining = stored_ids(&engine);
        assert_eq!(
            remaining, expected,
            "case {case:?}: expected the account to hold {expected:?} after the delete, \
             got {remaining:?}"
        );

        // ROUND TRIP through a reader: the count it reports is the store's own size, so the
        // delete is visible to a caller that never saw the response above.
        let reported = reported_count(&router, case).await;
        assert_eq!(
            reported.as_deref(),
            Some(expected.len().to_string().as_str()),
            "case {case:?}: expected GetCapacityManagerAttributes to report {} \
             configurations, got {reported:?}",
            expected.len()
        );
    }
}

/// Deleting every configuration one after another empties the account, and each call answers
/// the id it removed — the store is what each answer is read from, not the request.
#[tokio::test]
async fn deleting_each_configuration_in_turn_empties_the_account() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine, 4);

    for (step, target) in seeded.iter().enumerate() {
        let case = format!("step {step}, deleting {target}");
        let (status, body) = delete(&router, target, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            element(&body, "capacityManagerDataExportId").as_deref(),
            Some(target.as_str()),
            "case {case:?}: expected the deleted id {target:?} in the answer, got {body}"
        );
        let expected: Vec<String> = seeded[step + 1..].to_vec();
        let remaining = stored_ids(&engine);
        assert_eq!(
            remaining, expected,
            "case {case:?}: expected {expected:?} still held, got {remaining:?}"
        );
    }

    let reported = reported_count(&router, "the emptied account").await;
    assert_eq!(
        reported.as_deref(),
        Some("0"),
        "case \"the emptied account\": expected no configurations reported, got {reported:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE 2: a well-formed id naming no configuration is refused
// ---------------------------------------------------------------------------

/// A well-formed export id the account holds no configuration for is refused
/// `InvalidCapacityManagerDataExportId.NotFound` — including the all-zero id, which is a
/// well-formed id of this kind rather than an unreadable one — and the account is left
/// exactly as it was.
#[tokio::test]
async fn a_well_formed_id_naming_no_configuration_is_not_found() {
    // `(case, the id the request names)`: the all-zero long id, another long id nothing was
    // stored under, the short id form EC2 also issues, and an upper-bound-length hex suffix.
    let rows: [(&str, &str); 4] = [
        ("the all-zero long id", "cmde-00000000000000000"),
        ("a long id nothing was stored under", "cmde-0ffffffffffffffff"),
        ("a short id nothing was stored under", "cmde-0a1b2c3d"),
        ("a long id of all-f digits", "cmde-fffffffffffffffff"),
    ];

    for (case, id) in rows {
        let (router, engine) = super::ec2_query::emulator();
        let seeded = seed(&engine, 3);

        let (status, body) = delete(&router, id, &[]).await;
        assert_eq!(
            status, REFUSAL_STATUS,
            "case {case:?}: expected HTTP {REFUSAL_STATUS} for {id}, got {status} with \
             body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, INVALID_CAPACITY_MANAGER_DATA_EXPORT_ID_NOT_FOUND,
            "case {case:?}: expected the code \
             {INVALID_CAPACITY_MANAGER_DATA_EXPORT_ID_NOT_FOUND:?}, got {code:?} in {body}"
        );
        let message = error_message(&body);
        let expected = not_found_message(id);
        assert_eq!(
            message, expected,
            "case {case:?}: expected the message {expected:?}, got {message:?}"
        );

        let remaining = stored_ids(&engine);
        assert_eq!(
            remaining, seeded,
            "case {case:?}: expected a refused delete to remove nothing, got {remaining:?}"
        );
    }
}

/// The id of a configuration that WAS held is refused the same way once it is gone: a delete
/// is not idempotent, and the second call tells the caller the configuration is no longer
/// there.
#[tokio::test]
async fn deleting_the_same_configuration_twice_refuses_the_second_call() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine, 2);
    let target = seeded[1].clone();

    let (status, body) = delete(&router, &target, &[]).await;
    assert_eq!(
        status, 200,
        "case \"the first delete\": expected HTTP 200, got {status} with body {body}"
    );

    let (status, body) = delete(&router, &target, &[]).await;
    assert_eq!(
        status, REFUSAL_STATUS,
        "case \"the second delete\": expected HTTP {REFUSAL_STATUS}, got {status} with \
         body {body}"
    );
    let code = error_code(&body);
    assert_eq!(
        code, INVALID_CAPACITY_MANAGER_DATA_EXPORT_ID_NOT_FOUND,
        "case \"the second delete\": expected the code \
         {INVALID_CAPACITY_MANAGER_DATA_EXPORT_ID_NOT_FOUND:?}, got {code:?} in {body}"
    );
    let message = error_message(&body);
    let expected = not_found_message(&target);
    assert_eq!(
        message, expected,
        "case \"the second delete\": expected the message {expected:?}, got {message:?}"
    );
    let remaining = stored_ids(&engine);
    assert_eq!(
        remaining,
        vec![seeded[0].clone()],
        "case \"the second delete\": expected only the untouched configuration to be held, \
         got {remaining:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE 3: a value that is no export id at all is malformed, before any lookup
// ---------------------------------------------------------------------------

/// A value outside the form EC2 issues a data export id in is refused
/// `InvalidCapacityManagerDataExportId.Malformed` with this service's one unreadable-id
/// sentence — whatever the account holds, and whether or not the request also carries a
/// `DryRun`.
#[tokio::test]
async fn a_value_that_is_no_export_id_is_refused_as_malformed() {
    // `(case, the value the request names)`: each row is a different WAY of falling outside
    // the `cmde-` plus 8-or-17-lowercase-hex-digit form.
    let rows: Vec<(&str, String)> = vec![
        ("the prefix alone", "cmde-".to_string()),
        ("the prefix without its separator", "cmde0a1b2c3d".to_string()),
        (
            "a suffix of neither published length",
            "cmde-0a1b2c".to_string(),
        ),
        (
            "a suffix one digit too long",
            "cmde-0a1b2c3d4e5f60718a".to_string(),
        ),
        (
            "a non-hexadecimal digit",
            "cmde-0a1b2c3d4e5f6071z".to_string(),
        ),
        ("an upper-case suffix", "cmde-0A1B2C3D".to_string()),
        (
            "another resource kind's id",
            "vpc-0a1b2c3d4e5f60718".to_string(),
        ),
        ("no prefix at all", "not-a-data-export".to_string()),
        (
            // Reported back inside an XML envelope, so a value carrying the characters that
            // are significant there is a form the rule has to hold for.
            "a value carrying XML metacharacters",
            "cmde-<&'\"0a1b2c3d".to_string(),
        ),
        (
            // Far longer than any id EC2 issues: the refusal names the leading
            // `MAX_ECHOED_RESOURCE_ID_CHARS` characters, so it cannot be made to carry an
            // unbounded value the caller chose.
            "a value longer than the message repeats back",
            "cmde-".to_string() + &"a".repeat(MAX_ECHOED_RESOURCE_ID_CHARS * 2),
        ),
    ];

    for (case, value) in rows {
        // Both with configurations held and with an empty account, and with the `DryRun`
        // flag both ways: the id is read for its FORM before anything else is decided.
        for (held, dry_run) in [(0usize, None), (3, None), (3, Some("true"))] {
            let (router, engine) = super::ec2_query::emulator();
            let seeded = seed(&engine, held);
            let case = format!("{case} with {held} held, DryRun={dry_run:?}");
            let extra: Vec<(&str, &str)> = dry_run
                .map(|flag| vec![("DryRun", flag)])
                .unwrap_or_default();

            let (status, body) = delete(&router, &value, &extra).await;
            assert_eq!(
                status, REFUSAL_STATUS,
                "case {case:?}: expected HTTP {REFUSAL_STATUS} for {value:?}, got {status} \
                 with body {body}"
            );
            let code = error_code(&body);
            assert_eq!(
                code, INVALID_CAPACITY_MANAGER_DATA_EXPORT_ID_MALFORMED,
                "case {case:?}: expected the code \
                 {INVALID_CAPACITY_MANAGER_DATA_EXPORT_ID_MALFORMED:?}, got {code:?} in {body}"
            );
            let message = error_message(&body);
            let expected = malformed_message(&value);
            assert_eq!(
                message, expected,
                "case {case:?}: expected the message {expected:?}, got {message:?}"
            );
            let remaining = stored_ids(&engine);
            assert_eq!(
                remaining, seeded,
                "case {case:?}: expected a refused delete to remove nothing, got {remaining:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE 4: DryRun asks about the caller
// ---------------------------------------------------------------------------

/// `DryRun` is answered `DryRunOperation` and removes nothing — for a configuration the
/// account holds and for a well-formed id it does not, because the flag asks whether the
/// caller MAY make the request rather than whether the resource is there. `DryRun=false` is
/// an ordinary delete.
#[tokio::test]
async fn a_dry_run_removes_nothing() {
    // `(case, whether the id names a held configuration)`.
    let rows: [(&str, bool); 2] = [
        ("a configuration the account holds", true),
        ("a well-formed id naming no configuration", false),
    ];

    for (case, names_held_in_store) in rows {
        let (router, engine) = super::ec2_query::emulator();
        let seeded = seed(&engine, 2);
        let id = if names_held_in_store {
            seeded[0].clone()
        } else {
            "cmde-0ffffffffffffffff".to_string()
        };

        let (status, body) = delete(&router, &id, &[("DryRun", "true")]).await;
        assert_eq!(
            status, DRY_RUN_STATUS,
            "case {case:?}: expected HTTP {DRY_RUN_STATUS} for a DryRun delete of {id}, \
             got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, "DryRunOperation",
            "case {case:?}: expected DryRunOperation, got {code:?} in {body}"
        );
        let remaining = stored_ids(&engine);
        assert_eq!(
            remaining, seeded,
            "case {case:?}: expected a DryRun to remove nothing, got {remaining:?}"
        );
    }

    // `DryRun=false` is an ordinary delete: the configuration goes.
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine, 2);
    let (status, body) = delete(&router, &seeded[0], &[("DryRun", "false")]).await;
    assert_eq!(
        status, 200,
        "case \"DryRun=false\": expected HTTP 200, got {status} with body {body}"
    );
    let remaining = stored_ids(&engine);
    assert_eq!(
        remaining,
        vec![seeded[1].clone()],
        "case \"DryRun=false\": expected the named configuration to be gone, got {remaining:?}"
    );
}
