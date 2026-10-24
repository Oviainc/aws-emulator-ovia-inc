// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetImport` over the CloudTrail import registry.
//!
//! Every case goes over the wire through the shared [`super::emulator`] harness —
//! an `awsJson1_1` `POST /` dispatched by the same `MultiServiceRouter` the
//! production router serves — so what is asserted here is what a client sees,
//! including the HTTP status and the error code.
//!
//! The rules under test:
//!
//! * a read reports the STORED import: every member of the response is the
//!   persisted value — whatever its status, destinations, source, event-time bounds
//!   or statistics — and a member the record does not carry stays absent
//!   (model: `GetImportResponse`);
//! * every status the model's `ImportStatus` enum declares is reported as that
//!   status, not only the ones an import passes through on its way to `COMPLETED`;
//! * an import belongs to the Region it was created in, so an id naming no import
//!   in the caller's Region is refused exactly like an id naming no import at all,
//!   with `ImportNotFoundException`, the status the real service answers
//!   ([`IMPORT_NOT_FOUND_HTTP_STATUS`]) and a message naming the id from the
//!   request ([`import_not_found_message`]);
//! * reading changes nothing: a read is repeatable and the registry is untouched
//!   afterwards, so a later caller — including `StopImport` — sees exactly what the
//!   first reader did.
//!
//! The boundary constraint on `ImportId` (the modelled `UUID`: length 36..36,
//! pattern `^[a-f0-9\-]+$`) is refused before any import handler runs and is
//! covered for the whole import surface by `tests/all/import_stop.rs`, so it is not
//! re-asserted here.
//!
//! The registry is seeded through the service's public `state` module because
//! `StartImport` — the operation that creates an import — is still a scaffold stub
//! with its own issue (`20261022T230533Z-cloudtrail-startimport-stub.md`), so the
//! persisted records a read computes from cannot yet be made over the wire. A
//! seeded record is exactly what that handler will insert: the same registry, the
//! same key. The read itself is exercised only through the router.

use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use service_com_amazonaws_cloudtrail_cloudtrail_20131101::ops::types::{
    ImportSource, ImportStatistics, S3ImportSource, IMPORT_STATUS_VALUES,
};
use service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::{
    event_data_store_arn, import_not_found_message, ImportRecord, IMPORT_NOT_FOUND_HTTP_STATUS,
    IMPORT_STATUS_COMPLETED, IMPORT_STATUS_FAILED, IMPORT_STATUS_INITIALIZING,
    IMPORT_STATUS_IN_PROGRESS, IMPORT_STATUS_STOPPED,
};

use super::emulator::{Emulator, Reply, HOME_REGION};

/// A Region other than [`HOME_REGION`], for the cases about which Region an import
/// is reachable from.
const OTHER_REGION: &str = "us-west-2";

/// An instant, as a fixed offset from the epoch so stored times are comparable.
fn at(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds)
}

/// The epoch seconds a stored instant renders as on the wire.
fn epoch_seconds(at: SystemTime) -> f64 {
    at.duration_since(UNIX_EPOCH)
        .expect("a stored timestamp is at or after the epoch")
        .as_secs_f64()
}

/// The source S3 bucket an import reads from, as `StartImport` was given it.
fn source(bucket: &str, region: &str) -> ImportSource {
    ImportSource {
        s3: S3ImportSource {
            s3_location_uri: format!("s3://{bucket}/AWSLogs/000000000000/CloudTrail"),
            s3_bucket_region: region.to_string(),
            s3_bucket_access_role_arn: format!("arn:aws:iam::000000000000:role/{bucket}-import"),
        },
    }
}

/// Statistics carrying every member the model's `ImportStatistics` declares.
fn statistics(
    prefixes_found: i64,
    prefixes_completed: i64,
    files_completed: i64,
    events_completed: i64,
    failed_entries: i64,
) -> ImportStatistics {
    ImportStatistics {
        prefixes_found: Some(prefixes_found),
        prefixes_completed: Some(prefixes_completed),
        files_completed: Some(files_completed),
        events_completed: Some(events_completed),
        failed_entries: Some(failed_entries),
    }
}

/// An import record under `id` destined for an event data store in `region`,
/// carrying the values each row then varies.
fn record(id: &str, region: &str, status: &str) -> ImportRecord {
    ImportRecord {
        import_id: id.to_string(),
        // `ImportDestinations` is bounded to exactly one entry by the model.
        destinations: vec![event_data_store_arn(
            region,
            "aaaaaaaa-1111-4111-8111-aaaaaaaaaaaa",
        )],
        import_source: Some(source("trail-archive", region)),
        start_event_time: None,
        end_event_time: None,
        status: status.to_string(),
        created_timestamp: at(1_700_000_000),
        updated_timestamp: at(1_700_000_000),
        // Nothing is owed this record by the clock: a read reports the status the
        // row seeded rather than materialising an `INITIALIZING` -> `IN_PROGRESS`
        // move, whose own cases live in `tests/all/import_start.rs`.
        ingests_at: at(4_000_000_000),
        statistics: None,
    }
}

/// The table of imports a read must report back exactly, each with the Region its
/// registry entry is keyed under.
///
/// The rows differ along the members the response carries: every status the model's
/// `ImportStatus` enum declares (`ops::types::IMPORT_STATUS_VALUES` — asserted
/// exhaustive below), absent, full and partial statistics, absent and present
/// event-time bounds, an absent and a present source, an update time equal to and
/// later than the creation time, and the Region the import was created in.
fn stored_imports() -> Vec<(&'static str, &'static str, ImportRecord)> {
    let initializing = record(
        "11111111-1111-4111-8111-111111111111",
        HOME_REGION,
        IMPORT_STATUS_INITIALIZING,
    );

    let mut in_progress = record(
        "22222222-2222-4222-8222-222222222222",
        HOME_REGION,
        IMPORT_STATUS_IN_PROGRESS,
    );
    in_progress.statistics = Some(statistics(4, 1, 12, 340, 0));
    in_progress.start_event_time = Some(at(1_600_000_000));
    in_progress.end_event_time = Some(at(1_650_000_000));
    in_progress.updated_timestamp = at(1_700_000_600);

    let mut completed = record(
        "33333333-3333-4333-8333-333333333333",
        HOME_REGION,
        IMPORT_STATUS_COMPLETED,
    );
    completed.statistics = Some(statistics(4, 4, 51, 9_001, 0));
    completed.updated_timestamp = at(1_700_003_600);

    let mut failed = record(
        "44444444-4444-4444-8444-444444444444",
        HOME_REGION,
        IMPORT_STATUS_FAILED,
    );
    failed.statistics = Some(statistics(2, 1, 3, 7, 11));
    failed.updated_timestamp = at(1_700_001_200);

    let mut stopped = record(
        "55555555-5555-4555-8555-555555555555",
        HOME_REGION,
        IMPORT_STATUS_STOPPED,
    );
    // Only the members CloudTrail has recorded so far are stored, so the rest stay
    // absent on the wire rather than becoming zeroes.
    stopped.statistics = Some(ImportStatistics {
        prefixes_found: Some(9),
        ..ImportStatistics::default()
    });
    stopped.start_event_time = Some(at(1_600_000_000));
    stopped.updated_timestamp = at(1_700_000_900);

    let mut elsewhere = record(
        "66666666-6666-4666-8666-666666666666",
        OTHER_REGION,
        IMPORT_STATUS_COMPLETED,
    );
    elsewhere.import_source = None;
    elsewhere.statistics = Some(statistics(1, 1, 1, 1, 0));

    vec![
        (
            "initializing, nothing recorded yet",
            HOME_REGION,
            initializing,
        ),
        (
            "in progress, bounded event times",
            HOME_REGION,
            in_progress,
        ),
        ("completed", HOME_REGION, completed),
        ("failed with failed entries", HOME_REGION, failed),
        ("stopped, partial statistics", HOME_REGION, stopped),
        ("in another Region", OTHER_REGION, elsewhere),
    ]
}

/// Persist `record` in `region`'s share of the registry every import operation
/// reads, as `StartImport` will once it is implemented.
fn seed(emulator: &Emulator, region: &str, record: &ImportRecord) {
    emulator
        .state()
        .lock()
        .imports
        .insert((record.import_id.clone(), region.to_string()), record.clone());
}

/// Every stored import, rendered — for the cases that assert a read left the
/// registry alone.
fn registry(emulator: &Emulator) -> BTreeMap<String, String> {
    emulator
        .state()
        .lock()
        .imports
        .iter()
        .map(|((id, region), record)| (format!("{id}@{region}"), format!("{record:?}")))
        .collect()
}

/// Call `GetImport` from `region` for `import_id`.
async fn get_in(emulator: &Emulator, region: &str, import_id: &str) -> Reply {
    emulator
        .call_in("GetImport", region, &json!({ "ImportId": import_id }))
        .await
}

/// The error `Message` a reply carries, or `""`.
fn message(reply: &Reply) -> &str {
    reply
        .body
        .get("Message")
        .or_else(|| reply.body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// Assert `body` reports exactly what `record` persists.
fn assert_reports(case: &str, body: &Value, record: &ImportRecord) {
    let string = |member: &str| body.get(member).and_then(Value::as_str).map(str::to_string);
    let number = |member: &str| body.get(member).and_then(Value::as_f64);

    assert_eq!(
        string("ImportId").as_deref(),
        Some(record.import_id.as_str()),
        "case {case:?}: expected the stored import id {:?}, got {body:?}",
        record.import_id
    );
    assert_eq!(
        string("ImportStatus").as_deref(),
        Some(record.status.as_str()),
        "case {case:?}: expected the stored status {:?}, got {body:?}",
        record.status
    );
    assert_eq!(
        body.get("Destinations"),
        Some(&json!(record.destinations)),
        "case {case:?}: expected the stored destinations {:?}, got {body:?}",
        record.destinations
    );

    let s3 = body.get("ImportSource").and_then(|source| source.get("S3"));
    let s3_member = |member: &str| {
        s3.and_then(|s3| s3.get(member))
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    match &record.import_source {
        Some(stored) => {
            assert_eq!(
                s3_member("S3LocationUri").as_deref(),
                Some(stored.s3.s3_location_uri.as_str()),
                "case {case:?}: expected the stored source URI {:?}, got {body:?}",
                stored.s3.s3_location_uri
            );
            assert_eq!(
                s3_member("S3BucketRegion").as_deref(),
                Some(stored.s3.s3_bucket_region.as_str()),
                "case {case:?}: expected the stored source Region {:?}, got {body:?}",
                stored.s3.s3_bucket_region
            );
            assert_eq!(
                s3_member("S3BucketAccessRoleArn").as_deref(),
                Some(stored.s3.s3_bucket_access_role_arn.as_str()),
                "case {case:?}: expected the stored source role {:?}, got {body:?}",
                stored.s3.s3_bucket_access_role_arn
            );
        }
        None => assert!(
            body.get("ImportSource").is_none(),
            "case {case:?}: an import carrying no source must report none, got {body:?}"
        ),
    }

    assert_eq!(
        number("CreatedTimestamp"),
        Some(epoch_seconds(record.created_timestamp)),
        "case {case:?}: expected the stored creation time, got {body:?}"
    );
    assert_eq!(
        number("UpdatedTimestamp"),
        Some(epoch_seconds(record.updated_timestamp)),
        "case {case:?}: expected the stored update time, got {body:?}"
    );
    assert!(
        number("UpdatedTimestamp") >= number("CreatedTimestamp"),
        "case {case:?}: an import's UpdatedTimestamp must not precede its \
         CreatedTimestamp, got {body:?}"
    );
    assert_eq!(
        number("StartEventTime"),
        record.start_event_time.map(epoch_seconds),
        "case {case:?}: expected the stored StartEventTime {:?}, got {body:?}",
        record.start_event_time
    );
    assert_eq!(
        number("EndEventTime"),
        record.end_event_time.map(epoch_seconds),
        "case {case:?}: expected the stored EndEventTime {:?}, got {body:?}",
        record.end_event_time
    );

    let reported = body.get("ImportStatistics");
    match &record.statistics {
        Some(stored) => {
            let member = |member: &str| {
                reported
                    .and_then(|statistics| statistics.get(member))
                    .and_then(Value::as_i64)
            };
            // Every member of `ImportStatistics`, in the order the model declares
            // them.
            let expected = [
                ("PrefixesFound", stored.prefixes_found),
                ("PrefixesCompleted", stored.prefixes_completed),
                ("FilesCompleted", stored.files_completed),
                ("EventsCompleted", stored.events_completed),
                ("FailedEntries", stored.failed_entries),
            ];
            for (name, value) in expected {
                assert_eq!(
                    member(name),
                    value,
                    "case {case:?}: expected the stored {name} {value:?}, got {body:?}"
                );
            }
        }
        None => assert!(
            reported.is_none(),
            "case {case:?}: an import with no recorded statistics must report none, \
             got {body:?}"
        ),
    }
}

/// The row labelled `case`, looked up by its label rather than by position so a
/// reordering of [`stored_imports`] cannot silently change which import a case
/// addresses.
fn row(
    imports: &[(&'static str, &'static str, ImportRecord)],
    case: &str,
) -> (&'static str, ImportRecord) {
    imports
        .iter()
        .find(|(label, _, _)| *label == case)
        .map(|(_, region, record)| (*region, record.clone()))
        .unwrap_or_else(|| panic!("the table must carry the row {case:?}"))
}

#[tokio::test]
async fn a_read_reports_the_stored_import() {
    let emulator = Emulator::new();
    let imports = stored_imports();
    for (_, region, record) in &imports {
        seed(&emulator, region, record);
    }

    for (case, region, record) in &imports {
        let reply = get_in(&emulator, region, &record.import_id).await;
        assert_reports(case, reply.expect_ok(case), record);
    }

    // The table exercises the WHOLE modelled status catalog, so no status is
    // reported only because one row happened to carry it.
    let reported: BTreeSet<&str> = imports
        .iter()
        .map(|(_, _, record)| record.status.as_str())
        .collect();
    let modelled: BTreeSet<&str> = IMPORT_STATUS_VALUES.iter().copied().collect();
    assert_eq!(
        reported, modelled,
        "every modelled ImportStatus must be covered by a row, got {reported:?}"
    );
}

#[tokio::test]
async fn a_well_formed_id_naming_no_import_is_not_found() {
    let emulator = Emulator::new();
    let imports = stored_imports();
    for (_, region, record) in &imports {
        seed(&emulator, region, record);
    }
    let absent = "99999999-9999-4999-8999-999999999999";
    let (_, elsewhere) = row(&imports, "in another Region");
    let (_, home) = row(&imports, "initializing, nothing recorded yet");

    // An id no import carries, and a stored import addressed from a Region it was
    // not created in: neither names an import the registry holds for that Region.
    let cases = [
        ("absent id", HOME_REGION, absent.to_string()),
        ("absent id, another Region", OTHER_REGION, absent.to_string()),
        (
            "an import created in another Region",
            HOME_REGION,
            elsewhere.import_id.clone(),
        ),
        (
            "this Region's import, from another Region",
            OTHER_REGION,
            home.import_id.clone(),
        ),
        // `UUID` is `len 36..36, pattern ^[a-f0-9\-]+$`, so a value of 36 hex and
        // dash characters reaches the handler however little it looks like a UUID.
        (
            "a well-formed id of the right length",
            HOME_REGION,
            "------------------------------------".to_string(),
        ),
    ];

    for (case, region, import_id) in cases {
        let reply = get_in(&emulator, region, &import_id).await;
        reply.expect_error(
            case,
            IMPORT_NOT_FOUND_HTTP_STATUS,
            "ImportNotFoundException",
        );
        // The refusal is built from the request: it names the import id supplied.
        let expected = import_not_found_message(&import_id);
        assert_eq!(
            message(&reply),
            expected,
            "case {case:?}: expected the refusal to name the import job it addressed \
             ({expected:?}), got {:?}",
            message(&reply)
        );
    }
}

#[tokio::test]
async fn reading_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    let imports = stored_imports();
    for (_, region, record) in &imports {
        seed(&emulator, region, record);
    }
    let before = registry(&emulator);

    for (case, region, record) in &imports {
        let first = get_in(&emulator, region, &record.import_id).await;
        let first = first.expect_ok(case).clone();
        for repeat in 1..=3 {
            let case = format!("{case}, read {repeat}");
            let again = get_in(&emulator, region, &record.import_id).await;
            assert_eq!(
                again.expect_ok(&case),
                &first,
                "case {case:?}: a read must answer what the first read answered, got {:?}",
                again.body
            );
        }
    }
    // A refused read must not write either.
    get_in(
        &emulator,
        HOME_REGION,
        "99999999-9999-4999-8999-999999999999",
    )
    .await;

    assert_eq!(
        registry(&emulator),
        before,
        "the registry must be untouched by reads, got {:?}",
        registry(&emulator)
    );

    // A read left the import stoppable: the state a later mutating caller sees is
    // the one the first reader saw.
    let (region, running) = row(&imports, "in progress, bounded event times");
    let case = "in progress, bounded event times";
    let stopped = emulator
        .call_in(
            "StopImport",
            region,
            &json!({ "ImportId": running.import_id }),
        )
        .await;
    assert_eq!(
        stopped
            .expect_ok(case)
            .get("ImportStatus")
            .and_then(Value::as_str),
        Some(IMPORT_STATUS_STOPPED),
        "case {case:?}: a read must leave a running import stoppable, got {:?}",
        stopped.body
    );
}
