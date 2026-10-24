// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `StartImport`, and for the `GetImport` read every case
//! round-trips the import through.
//!
//! Every case goes over the wire through the shared [`super::emulator`] harness —
//! an `awsJson1_1` `POST /` dispatched by the same `MultiServiceRouter` the
//! production router serves — so what is asserted here is what a client sees,
//! including the HTTP status and the error code.
//!
//! The rules under test:
//!
//! * a new import is PERSISTED and the reply is computed from the stored record:
//!   the id the service minted (a modelled `UUID`: 36 characters of
//!   `^[a-f0-9\-]+$`), the destinations and source the request gave, the time
//!   bounds it gave, `INITIALIZING`, and the two instants the service stamps —
//!   and `GetImport` reports the same record afterwards;
//! * a destination is "The ARN (or the ID suffix of the ARN) of an event data
//!   store", so every form addresses the same store; one that cannot be read is
//!   `EventDataStoreARNInvalidException`, one naming no store in the Region it
//!   addresses is `EventDataStoreNotFoundException` (its message naming the ARN
//!   addressed), and one naming a store awaiting deletion is
//!   `InactiveEventDataStoreException`;
//! * a request that is not a startable import is refused with the modelled error
//!   for what is wrong with it, and persists nothing: a new import missing
//!   `Destinations` or `ImportSource` (`InvalidParameterException`), a source that
//!   is not an S3 location (`InvalidImportSourceException`), a time period whose
//!   start is after its end (`InvalidParameterException`), and an `ImportId`
//!   naming no stored import (`ImportNotFoundException`);
//! * only one import runs at a time per Region
//!   (`AccountHasOngoingImportException`), and a stopped one no longer blocks a
//!   new one;
//! * the registry is bounded: a start past the emulator's own ceiling is
//!   `OperationNotPermittedException` and persists nothing;
//! * an `ImportId` retries the stored job under the same id: a job that stopped
//!   runs again, while one still running is refused rather than answered with the
//!   stored record;
//! * the status moves on the CLOCK, not on a read: a new import reads
//!   `INITIALIZING` until `IN_PROGRESS` is due, and it becomes due whether or not
//!   anybody read it;
//! * a started import is ONE record, so every read path — `GetImport` and
//!   `ListImports`, filtered and unfiltered — reports it in that one status.
//!
//! The event data store registry is seeded through the crate's public `state`
//! module because `CreateEventDataStore` is still a scaffold stub (it answers 501),
//! so a destination's store cannot yet be made over the wire. `StartImport` itself
//! is only ever called through the router. `GetImport`'s own rules — its projection
//! of a stored record over every `ImportStatus` value, and its refusals — are
//! covered by `tests/all/import_reads.rs`; this suite uses it for the round trip.

use std::collections::BTreeSet;
use std::thread::sleep;
use std::time::{Duration, UNIX_EPOCH};

use serde_json::{json, Value};

use service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::{
    event_data_store_arn, import_not_found_message, EventDataStoreRecord, EventDataStoreState,
    ImportRecord, IMPORT_INGESTION_STARTS_AFTER, IMPORT_NOT_FOUND_HTTP_STATUS, IMPORT_QUOTA,
    IMPORT_STATUS_INITIALIZING, IMPORT_STATUS_IN_PROGRESS, IMPORT_STATUS_STOPPED,
};

use super::emulator::{Emulator, Reply, HOME_REGION};

/// A Region other than [`HOME_REGION`], for the cases about which Region a
/// destination and an import belong to.
const OTHER_REGION: &str = "us-west-2";

/// The HTTP status this service answers `EventDataStoreNotFoundException` with: the
/// model annotates it `@httpError(404)` and the recorded CloudTrail response to a
/// `StartImport` naming an unknown event data store is 400.
///
/// Stated here rather than imported from `crate::state`, deliberately: the
/// expectation is what a client sees on the wire, so it must not move just because
/// the crate's own constant does.
const EVENT_DATA_STORE_NOT_FOUND_STATUS: u16 = 400;

/// The length the model's `UUID` shape pins `ImportId` to (length 36..36).
const IMPORT_ID_LEN: usize = 36;

/// The id of the active event data store most cases import into.
const ACTIVE_STORE: &str = "11111111-1111-4111-8111-111111111111";
/// A store that has stopped ingesting new events: still active.
const STOPPED_INGESTION_STORE: &str = "22222222-2222-4222-8222-222222222222";
/// A store awaiting deletion: inactive.
const PENDING_DELETION_STORE: &str = "33333333-3333-4333-8333-333333333333";
/// A store that lives only in [`OTHER_REGION`].
const OTHER_REGION_STORE: &str = "44444444-4444-4444-8444-444444444444";

/// A margin on top of [`IMPORT_INGESTION_STARTS_AFTER`], so a case that waits for
/// the clock-owed transition waits past it rather than up to it.
fn past_ingestion_start() -> Duration {
    IMPORT_INGESTION_STARTS_AFTER + Duration::from_millis(250)
}

/// Persist an event data store in the registry every Lake operation reads, as
/// `CreateEventDataStore` will once it is implemented.
fn seed_store(emulator: &Emulator, id: &str, region: &str, status: EventDataStoreState) {
    let record = EventDataStoreRecord {
        id: id.to_string(),
        name: format!("lake-{id}"),
        region: region.to_string(),
        status,
        ..EventDataStoreRecord::default()
    };
    emulator
        .state()
        .lock()
        .event_data_stores
        .insert(record.id.clone(), record);
}

/// An emulator holding one store of each status this suite addresses, and none of
/// its own imports.
fn seeded_emulator() -> Emulator {
    let emulator = Emulator::new();
    seed_store(
        &emulator,
        ACTIVE_STORE,
        HOME_REGION,
        EventDataStoreState::Enabled,
    );
    seed_store(
        &emulator,
        STOPPED_INGESTION_STORE,
        HOME_REGION,
        EventDataStoreState::StoppedIngestion,
    );
    seed_store(
        &emulator,
        PENDING_DELETION_STORE,
        HOME_REGION,
        EventDataStoreState::PendingDeletion,
    );
    seed_store(
        &emulator,
        OTHER_REGION_STORE,
        OTHER_REGION,
        EventDataStoreState::Enabled,
    );
    emulator
}

/// How many imports the registry holds, in every Region.
fn import_count(emulator: &Emulator) -> usize {
    emulator.state().lock().imports.len()
}

/// An `ImportSource` over `uri`, the way a client sends one.
fn import_source(uri: &str) -> Value {
    json!({
        "S3": {
            "S3LocationUri": uri,
            "S3BucketRegion": HOME_REGION,
            "S3BucketAccessRoleArn": "arn:aws:iam::000000000000:role/CloudTrailImport",
        }
    })
}

/// The `ImportSource` most cases import from.
fn default_source() -> Value {
    import_source("s3://trail-archive/AWSLogs/000000000000/CloudTrail")
}

/// `StartImport` with `body`, called from `region`.
async fn start_in(emulator: &Emulator, region: &str, body: Value) -> Reply {
    emulator.call_in("StartImport", region, &body).await
}

/// `StartImport` for a new import into `destination`, from `region`.
async fn start_into(emulator: &Emulator, region: &str, destination: &str) -> Reply {
    start_in(
        emulator,
        region,
        json!({ "Destinations": [destination], "ImportSource": default_source() }),
    )
    .await
}

/// `GetImport` for `import_id`, called from `region`.
async fn get_in(emulator: &Emulator, region: &str, import_id: &str) -> Reply {
    emulator
        .call_in("GetImport", region, &json!({ "ImportId": import_id }))
        .await
}

/// `ListImports` called from `region`, optionally filtered by `ImportStatus`.
async fn list_in(emulator: &Emulator, region: &str, status: Option<&str>) -> Reply {
    let body = match status {
        Some(status) => json!({ "ImportStatus": status }),
        None => json!({}),
    };
    emulator.call_in("ListImports", region, &body).await
}

/// The statuses `ListImports` reports for `import_id`, in the listing `reply`.
fn listed_statuses(reply: &Value, import_id: &str, case: &str) -> Vec<String> {
    reply
        .get("Imports")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected an Imports list in {reply:?}"))
        .iter()
        .filter(|item| item.get("ImportId") == Some(&json!(import_id)))
        .map(|item| string_member(item, "ImportStatus", case))
        .collect()
}

/// `StopImport` for `import_id`, called from `region`.
async fn stop_in(emulator: &Emulator, region: &str, import_id: &str) -> Reply {
    emulator
        .call_in("StopImport", region, &json!({ "ImportId": import_id }))
        .await
}

/// A string member of a reply body.
fn string_member(body: &Value, member: &str, case: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a string {member} in {body:?}"))
        .to_string()
}

/// A timestamp member of a reply body, in epoch seconds.
fn seconds_member(body: &Value, member: &str, case: &str) -> f64 {
    body.get(member)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case:?}: expected a numeric {member} in {body:?}"))
}

/// Assert `import_id` satisfies the modelled `UUID` constraint the service mints it
/// under: 36 characters of `^[a-f0-9\-]+$`.
fn assert_minted_id(case: &str, import_id: &str) {
    assert_eq!(
        import_id.chars().count(),
        IMPORT_ID_LEN,
        "case {case:?}: a minted ImportId is {IMPORT_ID_LEN} characters, got {import_id:?}"
    );
    assert!(
        import_id
            .chars()
            .all(|c| matches!(c, '0'..='9' | 'a'..='f' | '-')),
        "case {case:?}: a minted ImportId matches ^[a-f0-9\\-]+$, got {import_id:?}"
    );
}

/// One row of [`a_new_import_is_persisted_and_reported_from_the_stored_record`]:
/// the case name, the destination as the request writes it, the source URI, and the
/// time bounds the request states, if any.
type NewImportRow = (&'static str, String, &'static str, Option<(i64, i64)>);

/// RULE: a new import is persisted, and both the reply and the later `GetImport`
/// read report that stored record.
///
/// The rows differ along what the rule generalises over: the form the destination
/// is written in, the shape of the source URI, and whether the request bounds the
/// imported events in time. Each row runs on its own emulator, so one row's import
/// is never the ongoing import another row would be refused for — that rule has its
/// own case below.
#[tokio::test]
async fn a_new_import_is_persisted_and_reported_from_the_stored_record() {
    // (case, destination as written, the S3 location URI, the time bounds)
    let rows: [NewImportRow; 6] = [
        (
            "the destination's full ARN",
            event_data_store_arn(HOME_REGION, ACTIVE_STORE),
            "s3://trail-archive/AWSLogs/000000000000/CloudTrail",
            None,
        ),
        (
            "the destination's bare id",
            ACTIVE_STORE.to_string(),
            "s3://trail-archive/AWSLogs/000000000000/CloudTrail",
            None,
        ),
        (
            "the destination's id behind the resource prefix",
            format!("eventdatastore/{ACTIVE_STORE}"),
            "s3://trail-archive",
            None,
        ),
        (
            "a store that has stopped ingesting new events",
            STOPPED_INGESTION_STORE.to_string(),
            "s3://trail-archive/deep/prefix/inside/the/bucket",
            None,
        ),
        (
            "a bounded time period",
            ACTIVE_STORE.to_string(),
            "s3://trail-archive/AWSLogs",
            Some((1_600_000_000, 1_600_086_400)),
        ),
        (
            "a time period of one instant",
            ACTIVE_STORE.to_string(),
            "s3://trail-archive/AWSLogs",
            Some((1_600_000_000, 1_600_000_000)),
        ),
    ];

    let mut minted = BTreeSet::new();
    for (case, destination, uri, bounds) in rows {
        let emulator = seeded_emulator();
        let source = import_source(uri);
        let mut request = json!({
            "Destinations": [destination.clone()],
            "ImportSource": source.clone(),
        });
        if let Some((start, end)) = bounds {
            request["StartEventTime"] = json!(start);
            request["EndEventTime"] = json!(end);
        }

        let reply = start_in(&emulator, HOME_REGION, request).await;
        let started = reply.expect_ok(case).clone();

        let import_id = string_member(&started, "ImportId", case);
        assert_minted_id(case, &import_id);
        assert!(
            minted.insert(import_id.clone()),
            "case {case:?}: every import gets its own id, but {import_id:?} was minted twice"
        );
        assert_eq!(
            started.get("ImportStatus"),
            Some(&json!(IMPORT_STATUS_INITIALIZING)),
            "case {case:?}: a new import starts in {IMPORT_STATUS_INITIALIZING}, got {started:?}"
        );
        assert_eq!(
            started.get("Destinations"),
            Some(&json!([destination])),
            "case {case:?}: expected the Destinations from the request, got {started:?}"
        );
        assert_eq!(
            started.get("ImportSource"),
            Some(&source),
            "case {case:?}: expected the ImportSource from the request, got {started:?}"
        );
        match bounds {
            Some((start, end)) => {
                assert_eq!(
                    seconds_member(&started, "StartEventTime", case),
                    start as f64,
                    "case {case:?}: expected the StartEventTime from the request, got {started:?}"
                );
                assert_eq!(
                    seconds_member(&started, "EndEventTime", case),
                    end as f64,
                    "case {case:?}: expected the EndEventTime from the request, got {started:?}"
                );
            }
            None => {
                assert_eq!(
                    (started.get("StartEventTime"), started.get("EndEventTime")),
                    (None, None),
                    "case {case:?}: a request bounding nothing reports no bounds, got {started:?}"
                );
            }
        }
        assert_eq!(
            seconds_member(&started, "UpdatedTimestamp", case),
            seconds_member(&started, "CreatedTimestamp", case),
            "case {case:?}: nothing has changed a just-created import, got {started:?}"
        );
        // `StartImportResponse` models no `ImportStatistics` member.
        assert_eq!(
            started.get("ImportStatistics"),
            None,
            "case {case:?}: StartImport reports no ImportStatistics, got {started:?}"
        );

        // The write survived the call: the registry holds exactly this one import...
        assert_eq!(
            import_count(&emulator),
            1,
            "case {case:?}: the import must be persisted exactly once"
        );
        // ...and the read reports the same record.
        let read = get_in(&emulator, HOME_REGION, &import_id).await;
        let fetched = read.expect_ok(case).clone();
        for member in [
            "ImportId",
            "Destinations",
            "ImportSource",
            "StartEventTime",
            "EndEventTime",
            "ImportStatus",
            "CreatedTimestamp",
            "UpdatedTimestamp",
        ] {
            assert_eq!(
                fetched.get(member),
                started.get(member),
                "case {case:?}: GetImport must report the stored {member}, got {fetched:?}"
            );
        }
        // Nothing imported any events, so the job carries no counts.
        assert_eq!(
            fetched.get("ImportStatistics"),
            None,
            "case {case:?}: an import that imported nothing reports no statistics, got \
             {fetched:?}"
        );
    }
}

/// RULE: a destination resolves to an active event data store in the Region it
/// addresses, or the import is refused with the modelled error for why it did not.
#[tokio::test]
async fn a_destination_must_name_an_active_event_data_store() {
    let unknown_id = "99999999-9999-4999-8999-999999999999";
    // (case, destination as written, expected HTTP status, expected error code)
    let rows: Vec<(&str, String, u16, &str)> = vec![
        (
            "an active store's bare id",
            ACTIVE_STORE.to_string(),
            200,
            "",
        ),
        (
            "an active store's ARN",
            event_data_store_arn(HOME_REGION, ACTIVE_STORE),
            200,
            "",
        ),
        (
            "a store awaiting deletion",
            PENDING_DELETION_STORE.to_string(),
            400,
            "InactiveEventDataStoreException",
        ),
        (
            "an id no store carries",
            unknown_id.to_string(),
            EVENT_DATA_STORE_NOT_FOUND_STATUS,
            "EventDataStoreNotFoundException",
        ),
        (
            "an ARN no store carries",
            event_data_store_arn(HOME_REGION, unknown_id),
            EVENT_DATA_STORE_NOT_FOUND_STATUS,
            "EventDataStoreNotFoundException",
        ),
        (
            "a store this Region does not hold",
            OTHER_REGION_STORE.to_string(),
            EVENT_DATA_STORE_NOT_FOUND_STATUS,
            "EventDataStoreNotFoundException",
        ),
        (
            "an ARN of another resource type",
            format!("arn:aws:cloudtrail:{HOME_REGION}:000000000000:trail/{ACTIVE_STORE}"),
            400,
            "EventDataStoreARNInvalidException",
        ),
        (
            "an ARN of another service",
            format!("arn:aws:s3:{HOME_REGION}:000000000000:eventdatastore/{ACTIVE_STORE}"),
            400,
            "EventDataStoreARNInvalidException",
        ),
        (
            "an ARN missing its Region",
            format!("arn:aws:cloudtrail::000000000000:eventdatastore/{ACTIVE_STORE}"),
            400,
            "EventDataStoreARNInvalidException",
        ),
        (
            "an ARN missing its resource",
            format!("arn:aws:cloudtrail:{HOME_REGION}:000000000000"),
            400,
            "EventDataStoreARNInvalidException",
        ),
    ];

    for (case, destination, status, code) in rows {
        let emulator = seeded_emulator();
        let reply = start_into(&emulator, HOME_REGION, &destination).await;

        if code.is_empty() {
            reply.expect_ok(case);
            assert_eq!(
                import_count(&emulator),
                1,
                "case {case:?}: an accepted destination starts the import"
            );
            continue;
        }

        reply.expect_error(case, status, code);
        if code == "EventDataStoreNotFoundException" {
            // The refusal names the ARN the request addressed: the one it gave, or
            // the one a bare id completes to in the caller's Region.
            let addressed = if destination.starts_with("arn:") {
                destination.clone()
            } else {
                event_data_store_arn(HOME_REGION, &destination)
            };
            assert_eq!(
                reply.message(),
                format!("The event data store {addressed} was not found."),
                "case {case:?}: expected a message naming {addressed}, got {reply:?}"
            );
        }
        assert_eq!(
            import_count(&emulator),
            0,
            "case {case:?}: a refused StartImport must persist nothing"
        );
    }
}

/// RULE: a request that is not a startable import is refused with the modelled
/// error for what is wrong with it, and persists nothing.
#[tokio::test]
async fn a_request_that_cannot_start_an_import_is_refused() {
    let destination = json!([ACTIVE_STORE]);
    let unknown_import = "abcdef01-2345-6789-abcd-ef0123456789";

    // (case, the request body, expected HTTP status, expected error code)
    let rows: Vec<(&str, Value, u16, &str)> = vec![
        (
            "a new import with no Destinations",
            json!({ "ImportSource": default_source() }),
            400,
            "InvalidParameterException",
        ),
        (
            "a new import with no ImportSource",
            json!({ "Destinations": destination }),
            400,
            "InvalidParameterException",
        ),
        (
            "a new import with neither",
            json!({}),
            400,
            "InvalidParameterException",
        ),
        (
            "a source URI that is not an S3 URI",
            json!({
                "Destinations": destination,
                "ImportSource": import_source("https://trail-archive.example/logs"),
            }),
            400,
            "InvalidImportSourceException",
        ),
        (
            "a source URI naming no bucket",
            json!({
                "Destinations": destination,
                "ImportSource": import_source("s3:///AWSLogs"),
            }),
            400,
            "InvalidImportSourceException",
        ),
        (
            "a bare bucket name as the source URI",
            json!({
                "Destinations": destination,
                "ImportSource": import_source("trail-archive"),
            }),
            400,
            "InvalidImportSourceException",
        ),
        (
            "a bucket with no prefix as the source URI",
            json!({
                "Destinations": destination,
                "ImportSource": import_source("s3://trail-archive"),
            }),
            200,
            "",
        ),
        // The model pins no pattern on `S3LocationUri`, so a value that names a
        // bucket is accepted whatever the bucket's own name looks like: these rows
        // span the forms this service's STRICTER `DeliveryS3Uri` pattern would have
        // refused, because refusing them here would be an error AWS does not return.
        (
            "a bucket at the shortest name S3 admits",
            json!({
                "Destinations": destination,
                "ImportSource": import_source("s3://abc/AWSLogs"),
            }),
            200,
            "",
        ),
        (
            "a 63-character bucket name",
            json!({
                "Destinations": destination,
                "ImportSource": import_source(&format!("s3://{}/AWSLogs", "a".repeat(63))),
            }),
            200,
            "",
        ),
        (
            "a two-character bucket name",
            json!({
                "Destinations": destination,
                "ImportSource": import_source("s3://ab/AWSLogs"),
            }),
            200,
            "",
        ),
        (
            "a 64-character bucket name",
            json!({
                "Destinations": destination,
                "ImportSource": import_source(&format!("s3://{}/AWSLogs", "a".repeat(64))),
            }),
            200,
            "",
        ),
        (
            "a legacy bucket name in upper case",
            json!({
                "Destinations": destination,
                "ImportSource": import_source("s3://Trail-Archive/AWSLogs"),
            }),
            200,
            "",
        ),
        (
            "a bucket name carrying an underscore",
            json!({
                "Destinations": destination,
                "ImportSource": import_source("s3://trail_archive/AWSLogs"),
            }),
            200,
            "",
        ),
        (
            "a bucket name ending in a dash",
            json!({
                "Destinations": destination,
                "ImportSource": import_source("s3://trail-archive-/AWSLogs"),
            }),
            200,
            "",
        ),
        (
            "a time period that ends before it starts",
            json!({
                "Destinations": destination,
                "ImportSource": default_source(),
                "StartEventTime": 1_600_086_400,
                "EndEventTime": 1_600_000_000,
            }),
            400,
            "InvalidParameterException",
        ),
        (
            "an ImportId naming no stored import",
            json!({ "ImportId": unknown_import }),
            IMPORT_NOT_FOUND_HTTP_STATUS,
            "ImportNotFoundException",
        ),
        (
            "an ImportId naming no stored import, with a whole new import beside it",
            json!({
                "ImportId": unknown_import,
                "Destinations": destination,
                "ImportSource": default_source(),
            }),
            IMPORT_NOT_FOUND_HTTP_STATUS,
            "ImportNotFoundException",
        ),
    ];

    for (case, request, status, code) in rows {
        let emulator = seeded_emulator();
        let reply = start_in(&emulator, HOME_REGION, request).await;

        if code.is_empty() {
            reply.expect_ok(case);
            assert_eq!(
                reply.status, status,
                "case {case:?}: expected HTTP {status}, got {reply:?}"
            );
            assert_eq!(
                import_count(&emulator),
                1,
                "case {case:?}: an accepted request starts the import"
            );
            continue;
        }

        reply.expect_error(case, status, code);
        if code == "ImportNotFoundException" {
            assert_eq!(
                reply.message(),
                import_not_found_message(unknown_import),
                "case {case:?}: expected a message naming {unknown_import}, got {reply:?}"
            );
        }
        assert_eq!(
            import_count(&emulator),
            0,
            "case {case:?}: a refused StartImport must persist nothing"
        );
    }
}

/// RULE: one import runs at a time per Region — a new one is refused while a
/// previous one is still in progress, a stopped one blocks nothing, and another
/// Region's job is not in progress here.
#[tokio::test]
async fn a_new_import_is_refused_while_one_is_still_in_progress() {
    let emulator = seeded_emulator();

    let first = start_into(&emulator, HOME_REGION, ACTIVE_STORE).await;
    let first_id = string_member(first.expect_ok("the first import"), "ImportId", "the first");

    // A second new import in the same Region has to wait for the first.
    let blocked = start_into(&emulator, HOME_REGION, ACTIVE_STORE).await;
    blocked.expect_error(
        "a second import while the first is in progress",
        400,
        "AccountHasOngoingImportException",
    );
    assert!(
        blocked.message().contains(&first_id),
        "the refusal must name the import still in progress ({first_id}), got {blocked:?}"
    );
    assert_eq!(
        import_count(&emulator),
        1,
        "a refused StartImport must persist nothing"
    );

    // An import belongs to the Region it was created in, so another Region's
    // registry is free.
    let elsewhere = start_into(&emulator, OTHER_REGION, OTHER_REGION_STORE).await;
    let elsewhere_id = string_member(
        elsewhere.expect_ok("an import in another Region"),
        "ImportId",
        "another Region",
    );
    assert_ne!(
        elsewhere_id, first_id,
        "each Region's import is its own job, got the same id twice"
    );

    // Once the first import is stopped it is no longer in progress, so a new one
    // starts.
    stop_in(&emulator, HOME_REGION, &first_id)
        .await
        .expect_ok("stopping the first import");
    let third = start_into(&emulator, HOME_REGION, ACTIVE_STORE).await;
    let third_id = string_member(
        third.expect_ok("a new import after the first was stopped"),
        "ImportId",
        "after the stop",
    );
    assert_ne!(
        third_id, first_id,
        "a new import gets a new id, not the stopped one's"
    );
    assert_eq!(
        import_count(&emulator),
        3,
        "the registry must hold the three imports that started"
    );

    // A retry that would RESTART a stopped job is refused for the same reason: the
    // third import now holds the Region.
    let blocked_retry = start_in(&emulator, HOME_REGION, json!({ "ImportId": first_id })).await;
    blocked_retry.expect_error(
        "retrying a stopped import while another is in progress",
        400,
        "AccountHasOngoingImportException",
    );
    assert!(
        blocked_retry.message().contains(&third_id),
        "the refusal must name the import still in progress ({third_id}), got {blocked_retry:?}"
    );
    let still_stopped = get_in(&emulator, HOME_REGION, &first_id).await;
    assert_eq!(
        still_stopped
            .expect_ok("reading the stopped import after the refused retry")
            .get("ImportStatus"),
        Some(&json!(IMPORT_STATUS_STOPPED)),
        "a refused retry must not restart the import, got {still_stopped:?}"
    );
}

/// RULE: an `ImportId` retries the stored job under the same id — a job that
/// stopped runs again from the request's parameters, while a retry of a job that is
/// still running is refused, so no restated parameter is accepted and dropped.
#[tokio::test]
async fn an_import_id_retries_the_stored_import() {
    let emulator = seeded_emulator();

    let started = start_into(&emulator, HOME_REGION, ACTIVE_STORE).await;
    let started = started.expect_ok("the import to retry").clone();
    let import_id = string_member(&started, "ImportId", "the import to retry");

    // A retry of a RUNNING import has nothing to retry: it is refused, and the
    // parameters it restates are not taken.
    let running_retry = start_in(
        &emulator,
        HOME_REGION,
        json!({ "ImportId": import_id, "ImportSource": import_source("s3://ignored-archive") }),
    )
    .await;
    running_retry.expect_error(
        "retrying a running import",
        400,
        "AccountHasOngoingImportException",
    );
    assert!(
        running_retry.message().contains(&import_id),
        "the refusal must name the import still in progress ({import_id}), got \
         {running_retry:?}"
    );
    let unchanged = get_in(&emulator, HOME_REGION, &import_id).await;
    assert_eq!(
        unchanged.expect_ok("the import after the refused retry"),
        &started,
        "a refused retry must change nothing, got {unchanged:?}"
    );

    // Past the span the job is `IN_PROGRESS` rather than `INITIALIZING`, and still
    // running — so a retry is still refused, and the status a read reports is the
    // one the clock owes it.
    sleep(past_ingestion_start());
    let settled_retry = start_in(&emulator, HOME_REGION, json!({ "ImportId": import_id })).await;
    settled_retry.expect_error(
        "retrying a running import past the ingestion start",
        400,
        "AccountHasOngoingImportException",
    );
    let read_back = get_in(&emulator, HOME_REGION, &import_id).await;
    assert_eq!(
        read_back
            .expect_ok("reading the running import")
            .get("ImportStatus"),
        Some(&json!(IMPORT_STATUS_IN_PROGRESS)),
        "the running import reports {IMPORT_STATUS_IN_PROGRESS} once it is due, got \
         {read_back:?}"
    );

    stop_in(&emulator, HOME_REGION, &import_id)
        .await
        .expect_ok("stopping the import");
    let stopped = get_in(&emulator, HOME_REGION, &import_id).await;
    let stopped = stopped.expect_ok("the stopped import").clone();
    assert_eq!(
        stopped.get("ImportStatus"),
        Some(&json!(IMPORT_STATUS_STOPPED)),
        "the import must be stopped before the retry, got {stopped:?}"
    );

    // A retry of a STOPPED import runs it again, under the same id and with the
    // parameters this request states.
    let retried_source = import_source("s3://another-archive/AWSLogs");
    let retried = start_in(
        &emulator,
        HOME_REGION,
        json!({
            "ImportId": import_id,
            "ImportSource": retried_source.clone(),
            "StartEventTime": 1_600_000_000,
            "EndEventTime": 1_600_086_400,
        }),
    )
    .await;
    let retried = retried.expect_ok("retrying a stopped import").clone();
    assert_eq!(
        string_member(&retried, "ImportId", "the retry"),
        import_id,
        "a retry keeps the import's id, got {retried:?}"
    );
    assert_eq!(
        retried.get("ImportStatus"),
        Some(&json!(IMPORT_STATUS_INITIALIZING)),
        "a retried import runs again from {IMPORT_STATUS_INITIALIZING}, got {retried:?}"
    );
    assert_eq!(
        retried.get("ImportSource"),
        Some(&retried_source),
        "a retry uses the ImportSource it states, got {retried:?}"
    );
    assert_eq!(
        retried.get("Destinations"),
        started.get("Destinations"),
        "a retry that restates no Destinations keeps the stored ones, got {retried:?}"
    );
    assert_eq!(
        seconds_member(&retried, "CreatedTimestamp", "the retry"),
        seconds_member(&started, "CreatedTimestamp", "the retry"),
        "a retry does not re-create the import, got {retried:?}"
    );
    assert!(
        seconds_member(&retried, "UpdatedTimestamp", "the retry")
            > seconds_member(&started, "UpdatedTimestamp", "the retry"),
        "a retry that restarts the import stamps UpdatedTimestamp, got {retried:?}"
    );
    assert_eq!(
        import_count(&emulator),
        1,
        "a retry runs the SAME job: it must not add a record"
    );

    // What the retry wrote is what the registry holds.
    let read = get_in(&emulator, HOME_REGION, &import_id).await;
    let fetched = read.expect_ok("reading the retried import").clone();
    for member in ["ImportStatus", "ImportSource", "Destinations", "StartEventTime"] {
        assert_eq!(
            fetched.get(member),
            retried.get(member),
            "GetImport must report the retried {member}, got {fetched:?}"
        );
    }
}

/// RULE: a new import reads `INITIALIZING` until the clock owes it `IN_PROGRESS`;
/// reading it is not what moves it, and it moves whether or not anybody read it.
#[tokio::test]
async fn a_new_import_is_initializing_until_the_clock_starts_it() {
    for case in [
        "read before the clock starts it",
        "never read before it starts",
    ] {
        let emulator = seeded_emulator();
        let started = start_into(&emulator, HOME_REGION, ACTIVE_STORE).await;
        let started = started.expect_ok(case).clone();
        let import_id = string_member(&started, "ImportId", case);
        let created = seconds_member(&started, "CreatedTimestamp", case);

        if case.starts_with("read") {
            // Two reads inside the span, to pin that a read does not start the
            // import early.
            for attempt in 0..2 {
                let fresh = get_in(&emulator, HOME_REGION, &import_id).await;
                assert_eq!(
                    fresh.expect_ok(case).get("ImportStatus"),
                    Some(&json!(IMPORT_STATUS_INITIALIZING)),
                    "case {case:?} (read {attempt}): a new import reads \
                     {IMPORT_STATUS_INITIALIZING} until the clock starts it, got {fresh:?}"
                );
            }
        }

        sleep(past_ingestion_start());
        let running = get_in(&emulator, HOME_REGION, &import_id).await;
        let running = running.expect_ok(case).clone();
        assert_eq!(
            running.get("ImportStatus"),
            Some(&json!(IMPORT_STATUS_IN_PROGRESS)),
            "case {case:?}: the import must move to {IMPORT_STATUS_IN_PROGRESS}, got {running:?}"
        );
        // The stamped instant is when the move was DUE, not when it was observed, so
        // it is the same for a watched and an unwatched import and a second read
        // reports the same record.
        let updated = seconds_member(&running, "UpdatedTimestamp", case);
        let due = created + IMPORT_INGESTION_STARTS_AFTER.as_secs_f64();
        assert!(
            (updated - due).abs() < 1e-3,
            "case {case:?}: expected UpdatedTimestamp at the due instant {due}, got {updated}"
        );
        let again = get_in(&emulator, HOME_REGION, &import_id).await;
        assert_eq!(
            again.expect_ok(case),
            &running,
            "case {case:?}: a second read must report the same import, got {again:?}"
        );
    }
}

/// RULE: a started import is ONE stored record, so every read path reports it in the
/// same status — the one the clock owes it — and the `ImportStatus` filter selects on
/// that same status.
///
/// The rows differ along where the read falls relative to the instant the job's
/// `INITIALIZING` -> `IN_PROGRESS` move is due.
#[tokio::test]
async fn every_read_path_reports_the_started_import_in_one_status() {
    let emulator = seeded_emulator();
    let started = start_into(&emulator, HOME_REGION, ACTIVE_STORE).await;
    let started = started.expect_ok("start for the read paths").clone();
    let import_id = string_member(&started, "ImportId", "start for the read paths");

    // (case, the status every read path must report, the status none may report it in)
    let rows = [
        (
            "inside the initializing span",
            IMPORT_STATUS_INITIALIZING,
            IMPORT_STATUS_IN_PROGRESS,
        ),
        (
            "past the instant ingestion is due",
            IMPORT_STATUS_IN_PROGRESS,
            IMPORT_STATUS_INITIALIZING,
        ),
    ];
    for (case, expected, excluded) in rows {
        if expected == IMPORT_STATUS_IN_PROGRESS {
            sleep(past_ingestion_start());
        }

        // ListImports FIRST, so the listing is never reading a status some earlier
        // GetImport wrote: both paths owe the same transition.
        let listed = list_in(&emulator, HOME_REGION, None).await;
        let listed = listed.expect_ok(case).clone();
        assert_eq!(
            listed_statuses(&listed, &import_id, case),
            vec![expected.to_string()],
            "case {case:?}: ListImports must report the started import once, in \
             {expected}, got {listed:?}"
        );

        let fetched = get_in(&emulator, HOME_REGION, &import_id).await;
        let fetched = fetched.expect_ok(case).clone();
        assert_eq!(
            fetched.get("ImportStatus"),
            Some(&json!(expected)),
            "case {case:?}: GetImport must report {expected}, got {fetched:?}"
        );

        let selected = list_in(&emulator, HOME_REGION, Some(expected)).await;
        let selected = selected.expect_ok(case).clone();
        assert_eq!(
            listed_statuses(&selected, &import_id, case),
            vec![expected.to_string()],
            "case {case:?}: the {expected} filter must select the started import, \
             got {selected:?}"
        );

        let rejected = list_in(&emulator, HOME_REGION, Some(excluded)).await;
        let rejected = rejected.expect_ok(case).clone();
        assert!(
            listed_statuses(&rejected, &import_id, case).is_empty(),
            "case {case:?}: the {excluded} filter must not select the started import, \
             got {rejected:?}"
        );
    }
}

/// Seed `count` STOPPED imports into the registry, so a case can put the store at a
/// chosen size without making that many calls.
///
/// They are STOPPED on purpose: a stopped job holds no Region, so what a later
/// `StartImport` is refused for is the store's size and nothing else.
fn seed_stopped_imports(emulator: &Emulator, region: &str, count: usize) {
    let state = emulator.state();
    let mut store = state.lock();
    let at = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    for n in 0..count {
        let record = ImportRecord {
            // A distinct id in the form the modelled `UUID` pins.
            import_id: format!("{n:08x}-0000-0000-0000-000000000000"),
            destinations: vec![event_data_store_arn(region, ACTIVE_STORE)],
            import_source: None,
            start_event_time: None,
            end_event_time: None,
            status: IMPORT_STATUS_STOPPED.to_string(),
            created_timestamp: at,
            updated_timestamp: at,
            ingests_at: at,
            statistics: None,
        };
        store
            .imports
            .insert((record.import_id.clone(), region.to_string()), record);
    }
}

/// RULE: the registry is bounded — the emulator keeps at most `IMPORT_QUOTA` import
/// jobs, so a start that would exceed that is refused with the modelled
/// `OperationNotPermittedException` and persists nothing, while one that still fits
/// is served.
///
/// The rows sit on the bound: one import short of it, and at it.
#[tokio::test]
async fn an_import_past_the_store_ceiling_is_refused() {
    // (case, how many jobs the registry already holds, is the start served?)
    let rows = [
        ("one short of the ceiling", IMPORT_QUOTA - 1, true),
        ("at the ceiling", IMPORT_QUOTA, false),
    ];

    for (case, seeded, served) in rows {
        let emulator = seeded_emulator();
        seed_stopped_imports(&emulator, HOME_REGION, seeded);
        assert_eq!(
            import_count(&emulator),
            seeded,
            "case {case:?}: the registry must start at {seeded}, got {}",
            import_count(&emulator)
        );

        let reply = start_into(&emulator, HOME_REGION, ACTIVE_STORE).await;
        if served {
            let started = reply.expect_ok(case).clone();
            assert_minted_id(case, &string_member(&started, "ImportId", case));
            assert_eq!(
                import_count(&emulator),
                seeded + 1,
                "case {case:?}: a served start must persist its import, got {}",
                import_count(&emulator)
            );
        } else {
            reply.expect_error(case, 400, "OperationNotPermittedException");
            assert!(
                reply.message().contains(&IMPORT_QUOTA.to_string()),
                "case {case:?}: the refusal must state the ceiling {IMPORT_QUOTA}, got {reply:?}"
            );
            assert_eq!(
                import_count(&emulator),
                seeded,
                "case {case:?}: a refused start must persist nothing, got {}",
                import_count(&emulator)
            );
        }
    }
}

/// RULE: an import needs a source wherever it comes from — a retry of a stored job
/// that carries none, and that restates none, is refused rather than run with
/// nothing to import from.
///
/// The rows differ in whether the request restates the source the stored job lacks.
#[tokio::test]
async fn a_retry_with_no_source_anywhere_is_refused() {
    // The seeded jobs carry no `ImportSource` (`seed_stopped_imports`), which a job
    // started over the wire always would.
    let seeded_id = format!("{:08x}-0000-0000-0000-000000000000", 0);

    // (case, the retry request, is it served?)
    let rows = [
        ("a retry restating no source", json!({}), false),
        (
            "a retry restating a source",
            json!({ "ImportSource": default_source() }),
            true,
        ),
    ];

    for (case, extra, served) in rows {
        let emulator = seeded_emulator();
        seed_stopped_imports(&emulator, HOME_REGION, 1);
        let mut body = json!({ "ImportId": seeded_id });
        for (member, value) in extra.as_object().expect("the row's members") {
            body[member] = value.clone();
        }

        let reply = start_in(&emulator, HOME_REGION, body).await;
        if served {
            let retried = reply.expect_ok(case).clone();
            assert_eq!(
                retried.get("ImportSource"),
                Some(&default_source()),
                "case {case:?}: the retry must run with the source it restated, got {retried:?}"
            );
        } else {
            reply.expect_error(case, 400, "InvalidParameterException");
            assert!(
                reply.message().contains("ImportSource"),
                "case {case:?}: the refusal must name ImportSource, got {reply:?}"
            );
            let unchanged = get_in(&emulator, HOME_REGION, &seeded_id).await;
            assert_eq!(
                unchanged
                    .expect_ok(case)
                    .get("ImportStatus")
                    .and_then(Value::as_str),
                Some(IMPORT_STATUS_STOPPED),
                "case {case:?}: a refused retry must leave the stored job alone, got \
                 {unchanged:?}"
            );
        }
    }
}
