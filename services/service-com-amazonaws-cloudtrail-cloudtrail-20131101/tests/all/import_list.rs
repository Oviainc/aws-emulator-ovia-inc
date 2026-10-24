// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListImports` over the CloudTrail import registry.
//!
//! Every case goes over the wire through the shared [`super::emulator`] harness — an
//! `awsJson1_1` `POST /` dispatched by the same `MultiServiceRouter` the production
//! router serves — so what is asserted here is what a client sees, including the HTTP
//! status and the error code.
//!
//! The rules under test:
//!
//! * the listing is computed from the persisted import registry: it reports the imports
//!   the caller's Region holds, id-ordered, each member (`ImportId`, `ImportStatus`,
//!   `Destinations`, `CreatedTimestamp`, `UpdatedTimestamp` — the whole
//!   `ImportsListItem` shape) the stored value;
//! * "Returns information on all imports, or a select set of imports by ImportStatus or
//!   Destination" (model doc for `ListImports`): a filter the call names narrows the
//!   listing to the imports that match it, and filters compose; `Destination` is "The
//!   ARN of the destination event data store", so every form of that identifier — the
//!   whole ARN, the bare id suffix, the id under its resource prefix — selects the same
//!   imports;
//! * `MaxResults` bounds one page and `NextToken` resumes the listing exactly where the
//!   previous page stopped, so walking the pages yields each import once, in the order
//!   an unpaged call reports them;
//! * a `NextToken` this listing did not issue — unreadable, resuming at a value no
//!   `ImportId` can take, or issued for another Region or other filters — is refused
//!   with `InvalidNextTokenException` ("a token that is not valid, or a token that was
//!   previously used in a request with different parameters");
//! * a `Destination` readable as neither an event data store ARN nor the id suffix of
//!   one is refused with `EventDataStoreARNInvalidException`;
//! * the listing is a READ: it reports what another operation wrote (a `StopImport`
//!   shows up as `STOPPED` with the stamped `UpdatedTimestamp`) and changes nothing
//!   itself.
//!
//! The registry is seeded through the service's public `state` module because
//! `StartImport` — the operation that creates an import — is still a scaffold stub with
//! its own issue (`20261022T230533Z-cloudtrail-startimport-stub.md`), so the records a
//! listing computes from cannot yet be made over the wire. A seeded record is exactly
//! what that handler will insert: the same registry, the same key.

use std::time::{Duration, UNIX_EPOCH};

use serde_json::{json, Value};

use service_com_amazonaws_cloudtrail_cloudtrail_20131101::ops::types::{
    ImportSource, S3ImportSource,
};
use service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::{
    event_data_store_arn, event_data_store_arn_invalid_message, ImportRecord,
    IMPORT_STATUS_COMPLETED, IMPORT_STATUS_FAILED, IMPORT_STATUS_INITIALIZING,
    IMPORT_STATUS_IN_PROGRESS, IMPORT_STATUS_STOPPED,
};

use super::emulator::{Emulator, HOME_REGION};

/// A Region other than [`HOME_REGION`], to pin that an import is reported only in the
/// Region it belongs to.
const OTHER_REGION: &str = "us-west-2";

/// The modelled bounds of `ListImports$MaxResults` (range 1..1000, from
/// `smithy_query(query="operation",operation="ListImports")`).
const MAX_RESULTS_MIN: i32 = 1;
/// See [`MAX_RESULTS_MIN`].
const MAX_RESULTS_MAX: i32 = 1000;

/// The modelled minimum length of `PaginationToken` (length 4..1000), the shape
/// `ListImports$NextToken` targets.
const NEXT_TOKEN_MIN_LEN: usize = 4;

/// The instant a seeded import was created at, so every timestamp assertion has a
/// fixed value to compare against (epoch seconds, as `awsJson1_1` carries a `Date`).
const SEEDED_AT_EPOCH_SECONDS: u64 = 1_700_000_000;

/// The id of the event data store most seeded imports target.
const STORE_A: &str = "aaaaaaaa-1111-2222-3333-444444444444";
/// A second destination store, so a `Destination` filter has something to exclude.
const STORE_B: &str = "bbbbbbbb-1111-2222-3333-444444444444";
/// A readable store id no seeded import targets.
const STORE_C: &str = "cccccccc-1111-2222-3333-444444444444";

/// The id of the `n`-th seeded import: a distinct 36-character `UUID`
/// (`^[a-f0-9\-]+$`), ordered the way the registry iterates.
fn import_id(n: u32) -> String {
    format!("{n:08x}-0000-0000-0000-000000000000")
}

/// A seeded import in `status`, targeting the event data store `store_id` in `region`.
fn import_record(n: u32, status: &str, region: &str, store_id: &str) -> ImportRecord {
    let created = UNIX_EPOCH + Duration::from_secs(SEEDED_AT_EPOCH_SECONDS + u64::from(n));
    ImportRecord {
        import_id: import_id(n),
        destinations: vec![event_data_store_arn(region, store_id)],
        import_source: Some(ImportSource {
            s3: S3ImportSource {
                s3_location_uri: format!("s3://bucket-{store_id}/prefix"),
                s3_bucket_region: region.to_string(),
                s3_bucket_access_role_arn: "arn:aws:iam::000000000000:role/ImportRole".to_string(),
            },
        }),
        start_event_time: Some(created - Duration::from_secs(86_400)),
        end_event_time: Some(created),
        status: status.to_string(),
        created_timestamp: created,
        updated_timestamp: created,
        // Nothing is owed this record by the clock: a listing reports the status the
        // row seeded rather than materialising an `INITIALIZING` -> `IN_PROGRESS`
        // move, whose own cases live in `tests/all/import_start.rs`.
        ingests_at: UNIX_EPOCH + Duration::from_secs(4_000_000_000),
        statistics: None,
    }
}

/// Put `record` in `region`'s share of the import registry — what `StartImport` will do
/// once it is implemented.
fn seed(emulator: &Emulator, region: &str, record: &ImportRecord) {
    emulator
        .state()
        .lock()
        .imports
        .insert((record.import_id.clone(), region.to_string()), record.clone());
}

/// The imports every filter case below selects from: six in [`HOME_REGION`] spanning the
/// whole `ImportStatus` enum and two destinations, plus one in [`OTHER_REGION`].
fn seed_registry(emulator: &Emulator) -> Vec<ImportRecord> {
    // (n, status, destination store)
    let rows: [(u32, &str, &str); 6] = [
        (1, IMPORT_STATUS_INITIALIZING, STORE_A),
        (2, IMPORT_STATUS_IN_PROGRESS, STORE_A),
        (3, IMPORT_STATUS_IN_PROGRESS, STORE_B),
        (4, IMPORT_STATUS_COMPLETED, STORE_B),
        (5, IMPORT_STATUS_FAILED, STORE_A),
        (6, IMPORT_STATUS_STOPPED, STORE_B),
    ];
    let seeded: Vec<ImportRecord> = rows
        .into_iter()
        .map(|(n, status, store)| import_record(n, status, HOME_REGION, store))
        .collect();
    for record in &seeded {
        seed(emulator, HOME_REGION, record);
    }
    // An import of another Region, reported only there.
    seed(
        emulator,
        OTHER_REGION,
        &import_record(7, IMPORT_STATUS_IN_PROGRESS, OTHER_REGION, STORE_A),
    );
    seeded
}

/// One filter case: its name, the `Destination` and `ImportStatus` the call names, and
/// the seeded imports it must report, in order.
type FilterCase<'a> = (&'a str, Option<&'a str>, Option<&'a str>, Vec<u32>);

/// A string member of a JSON object.
fn string_member(value: &Value, member: &str, case: &str) -> String {
    value
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: expected a string {member} in {value:?}"))
        .to_string()
}

/// A timestamp member, in epoch seconds.
fn seconds_member(value: &Value, member: &str, case: &str) -> f64 {
    value
        .get(member)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case}: expected a numeric {member} in {value:?}"))
}

/// The `Imports` list of a reply.
fn items(body: &Value, case: &str) -> Vec<Value> {
    body.get("Imports")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case}: expected an Imports list in {body:?}"))
        .clone()
}

/// The `ImportId`s of a reply's `Imports`, in the order they were reported.
fn reported_ids(body: &Value, case: &str) -> Vec<String> {
    items(body, case)
        .iter()
        .map(|item| string_member(item, "ImportId", case))
        .collect()
}

/// Assert `item` reports every member of `record` as the registry holds it.
fn assert_item_matches(item: &Value, record: &ImportRecord, case: &str) {
    let seconds = record
        .created_timestamp
        .duration_since(UNIX_EPOCH)
        .expect("a seeded instant after the epoch")
        .as_secs() as f64;
    assert_eq!(
        string_member(item, "ImportStatus", case),
        record.status,
        "case {case}: expected the stored ImportStatus, got {item:?}"
    );
    assert_eq!(
        item.get("Destinations"),
        Some(&json!(record.destinations)),
        "case {case}: expected the stored Destinations, got {item:?}"
    );
    assert_eq!(
        seconds_member(item, "CreatedTimestamp", case),
        seconds,
        "case {case}: expected the stored CreatedTimestamp, got {item:?}"
    );
    assert_eq!(
        seconds_member(item, "UpdatedTimestamp", case),
        seconds,
        "case {case}: expected the stored UpdatedTimestamp, got {item:?}"
    );
}

/// Walk every page of the listing `filters` selects, `max_results` at a time, and hand
/// back the ids in the order they were reported.
async fn walk_pages(
    emulator: &Emulator,
    case: &str,
    filters: &Value,
    max_results: i32,
) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0;
    loop {
        pages += 1;
        assert!(
            pages <= 64,
            "case {case}: the listing did not terminate after {pages} pages"
        );
        let mut body = filters.clone();
        body["MaxResults"] = json!(max_results);
        if let Some(resume) = &token {
            body["NextToken"] = json!(resume);
        }
        let reply = emulator.call("ListImports", &body).await;
        let page = reply.expect_ok(case).clone();
        let reported = reported_ids(&page, case);
        assert!(
            reported.len() <= max_results as usize,
            "case {case}: a page must hold at most MaxResults ({max_results}) imports, \
             got {reported:?}"
        );
        ids.extend(reported.iter().cloned());
        token = page
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        match &token {
            None => break,
            Some(resume) => {
                assert!(
                    resume.len() >= NEXT_TOKEN_MIN_LEN,
                    "case {case}: a NextToken must satisfy the modelled length \
                     {NEXT_TOKEN_MIN_LEN}.., got {resume:?}"
                );
                assert_eq!(
                    reported.len(),
                    max_results as usize,
                    "case {case}: only a FULL page may be followed by a NextToken, \
                     got {reported:?}"
                );
            }
        }
    }
    ids
}

/// The listing reports the caller's Region id-ordered, and every filter the call names
/// narrows it — across the whole `ImportStatus` enum, every form of a `Destination`
/// identifier, and the two filters together.
#[tokio::test]
async fn list_imports_reports_the_registry_and_narrows_by_every_filter() {
    let emulator = Emulator::new();
    let seeded = seed_registry(&emulator);
    let destination_a = event_data_store_arn(HOME_REGION, STORE_A);
    let destination_b = event_data_store_arn(HOME_REGION, STORE_B);
    // A store of another Region: readable, and the destination of no import HOME_REGION
    // holds.
    let destination_elsewhere = event_data_store_arn(OTHER_REGION, STORE_A);

    // (case, Destination, ImportStatus, the ids expected, in order)
    let rows: [FilterCase<'_>; 14] = [
        ("no filter", None, None, vec![1, 2, 3, 4, 5, 6]),
        (
            "ImportStatus INITIALIZING",
            None,
            Some(IMPORT_STATUS_INITIALIZING),
            vec![1],
        ),
        (
            "ImportStatus IN_PROGRESS",
            None,
            Some(IMPORT_STATUS_IN_PROGRESS),
            vec![2, 3],
        ),
        (
            "ImportStatus COMPLETED",
            None,
            Some(IMPORT_STATUS_COMPLETED),
            vec![4],
        ),
        (
            "ImportStatus FAILED",
            None,
            Some(IMPORT_STATUS_FAILED),
            vec![5],
        ),
        (
            "ImportStatus STOPPED",
            None,
            Some(IMPORT_STATUS_STOPPED),
            vec![6],
        ),
        (
            "Destination as the whole ARN",
            Some(destination_a.as_str()),
            None,
            vec![1, 2, 5],
        ),
        ("Destination as a bare id", Some(STORE_A), None, vec![1, 2, 5]),
        (
            "Destination as the id under its resource prefix",
            Some("eventdatastore/aaaaaaaa-1111-2222-3333-444444444444"),
            None,
            vec![1, 2, 5],
        ),
        (
            "the other Destination",
            Some(destination_b.as_str()),
            None,
            vec![3, 4, 6],
        ),
        (
            "both filters",
            Some(destination_a.as_str()),
            Some(IMPORT_STATUS_IN_PROGRESS),
            vec![2],
        ),
        (
            "both filters, selecting nothing",
            Some(destination_b.as_str()),
            Some(IMPORT_STATUS_INITIALIZING),
            vec![],
        ),
        (
            "a readable Destination no import targets",
            Some(STORE_C),
            None,
            vec![],
        ),
        (
            "a Destination in another Region",
            Some(destination_elsewhere.as_str()),
            None,
            vec![],
        ),
    ];

    for (case, destination, status, expected) in rows {
        let mut body = json!({});
        if let Some(destination) = destination {
            body["Destination"] = json!(destination);
        }
        if let Some(status) = status {
            body["ImportStatus"] = json!(status);
        }
        let reply = emulator.call("ListImports", &body).await;
        let listed = reply.expect_ok(case).clone();

        let expected_ids: Vec<String> = expected.iter().map(|n| import_id(*n)).collect();
        assert_eq!(
            reported_ids(&listed, case),
            expected_ids,
            "case {case}: expected exactly {expected_ids:?}, got {listed:?}"
        );
        assert_eq!(
            listed.get("NextToken"),
            None,
            "case {case}: a listing that fits one page carries no NextToken, got {listed:?}"
        );
        // Every member of every reported item is the stored one.
        for (item, n) in items(&listed, case).iter().zip(expected.iter()) {
            let record = seeded
                .iter()
                .find(|record| record.import_id == import_id(*n))
                .expect("a seeded import");
            assert_item_matches(item, record, case);
        }
    }

    // The other Region reports its own import, and only that one.
    let elsewhere = emulator
        .call_in("ListImports", OTHER_REGION, &json!({}))
        .await;
    let body = elsewhere.expect_ok("the other Region").clone();
    assert_eq!(
        reported_ids(&body, "the other Region"),
        vec![import_id(7)],
        "an import is reported only in the Region it belongs to, got {body:?}"
    );

    // Listing changed nothing: the registry still holds every seeded record as it was.
    let store = emulator.state();
    let held = store.lock();
    assert_eq!(
        held.imports.len(),
        seeded.len() + 1,
        "a listing must not add or drop records"
    );
    for record in &seeded {
        let stored = held
            .import(&record.import_id, HOME_REGION)
            .unwrap_or_else(|| panic!("the seeded import {}", record.import_id));
        assert_eq!(
            stored.status, record.status,
            "a listing must not move the import {}, got {:?}",
            record.import_id, stored.status
        );
        assert_eq!(
            stored.updated_timestamp, record.updated_timestamp,
            "a listing must not re-stamp the import {}",
            record.import_id
        );
    }
}

/// `MaxResults` bounds one page and `NextToken` resumes where the page stopped: walking
/// the pages reports each selected import exactly once, in the unpaged order — at every
/// page size from one to one past the whole set, at the modelled bounds, and under a
/// filter.
#[tokio::test]
async fn list_imports_pages_at_max_results_and_resumes_where_the_page_stopped() {
    let emulator = Emulator::new();
    seed_registry(&emulator);
    let destination_a = event_data_store_arn(HOME_REGION, STORE_A);

    let all: Vec<String> = (1..=6).map(import_id).collect();
    // (case, MaxResults, the filters, the ids expected across every page)
    let rows: [(&str, i32, Value, Vec<String>); 9] = [
        (
            "MaxResults at the modelled minimum",
            MAX_RESULTS_MIN,
            json!({}),
            all.clone(),
        ),
        ("MaxResults 2", 2, json!({}), all.clone()),
        ("MaxResults 3", 3, json!({}), all.clone()),
        ("MaxResults 4", 4, json!({}), all.clone()),
        (
            "MaxResults exactly the whole set",
            6,
            json!({}),
            all.clone(),
        ),
        ("MaxResults one past the whole set", 7, json!({}), all.clone()),
        (
            "MaxResults at the modelled maximum",
            MAX_RESULTS_MAX,
            json!({}),
            all.clone(),
        ),
        (
            "paging a Destination-filtered listing",
            1,
            json!({ "Destination": destination_a }),
            vec![import_id(1), import_id(2), import_id(5)],
        ),
        (
            "paging an ImportStatus-filtered listing",
            1,
            json!({ "ImportStatus": IMPORT_STATUS_IN_PROGRESS }),
            vec![import_id(2), import_id(3)],
        ),
    ];

    for (case, max_results, filters, expected) in rows {
        let walked = walk_pages(&emulator, case, &filters, max_results).await;
        assert_eq!(
            walked, expected,
            "case {case}: walking the pages must report {expected:?} once each, got {walked:?}"
        );
    }

    // A `MaxResults` outside the modelled range is refused at the boundary, so no page
    // size the handler sees can be out of range.
    for (case, max_results) in [
        ("MaxResults one under the modelled minimum", MAX_RESULTS_MIN - 1),
        ("MaxResults one over the modelled maximum", MAX_RESULTS_MAX + 1),
    ] {
        let reply = emulator
            .call("ListImports", &json!({ "MaxResults": max_results }))
            .await;
        reply.expect_error(case, 400, "ValidationException");
    }
}

/// A `NextToken` this listing did not issue — unreadable, resuming at a value no import
/// id can take, or issued for another Region or other filters — is refused with
/// `InvalidNextTokenException`; the token it DID issue resumes the same listing.
#[tokio::test]
async fn list_imports_refuses_a_next_token_it_did_not_issue() {
    let emulator = Emulator::new();
    seed_registry(&emulator);
    let destination_a = event_data_store_arn(HOME_REGION, STORE_A);

    // The token a one-import page of the `IN_PROGRESS` listing hands back.
    let first = emulator
        .call(
            "ListImports",
            &json!({ "ImportStatus": IMPORT_STATUS_IN_PROGRESS, "MaxResults": 1 }),
        )
        .await;
    let body = first.expect_ok("the first page").clone();
    let issued = string_member(&body, "NextToken", "the first page");

    // The token resumes the listing it was issued for.
    let resumed = emulator
        .call(
            "ListImports",
            &json!({
                "ImportStatus": IMPORT_STATUS_IN_PROGRESS,
                "MaxResults": 1,
                "NextToken": issued,
            }),
        )
        .await;
    let next = resumed.expect_ok("resuming with the issued token").clone();
    assert_eq!(
        reported_ids(&next, "resuming with the issued token"),
        vec![import_id(3)],
        "the issued token must resume where the page stopped, got {next:?}"
    );

    // (case, the body, the error code expected)
    let rows: [(&str, Value, &str); 8] = [
        (
            "the token reused with no filter",
            json!({ "NextToken": issued }),
            "InvalidNextTokenException",
        ),
        (
            "the token reused with a different ImportStatus",
            json!({ "ImportStatus": IMPORT_STATUS_COMPLETED, "NextToken": issued }),
            "InvalidNextTokenException",
        ),
        (
            "the token reused with an added Destination",
            json!({
                "ImportStatus": IMPORT_STATUS_IN_PROGRESS,
                "Destination": destination_a,
                "NextToken": issued,
            }),
            "InvalidNextTokenException",
        ),
        (
            "a token this service never issued",
            json!({ "NextToken": "not-a-token-this-service-issued" }),
            "InvalidNextTokenException",
        ),
        (
            "a token naming no import to resume at",
            json!({ "NextToken": "||||" }),
            "InvalidNextTokenException",
        ),
        (
            // The resume field is an `ImportId`, which the model pins to the `UUID`
            // shape (length 36..36, pattern `^[a-f0-9\-]+$`); `g` is outside it, so
            // no import can be named by this token.
            "a token resuming at a value no import id can take",
            json!({
                "ImportStatus": IMPORT_STATUS_IN_PROGRESS,
                "NextToken": format!(
                    "{}g|{HOME_REGION}|{IMPORT_STATUS_IN_PROGRESS}|",
                    "0".repeat(35),
                ),
            }),
            "InvalidNextTokenException",
        ),
        (
            // An import belongs to one Region, so a token issued for one Region's
            // listing resumes no listing in another.
            "a token carrying another Region",
            json!({
                "ImportStatus": IMPORT_STATUS_IN_PROGRESS,
                "NextToken": format!("{}|{OTHER_REGION}|{IMPORT_STATUS_IN_PROGRESS}|", import_id(3)),
            }),
            "InvalidNextTokenException",
        ),
        (
            // `PaginationToken` is length 4..1000, so a shorter token never reaches
            // the handler at all.
            "a token under the modelled minimum length",
            json!({ "NextToken": "a".repeat(NEXT_TOKEN_MIN_LEN - 1) }),
            "ValidationException",
        ),
    ];

    for (case, body, expected) in rows {
        let reply = emulator.call("ListImports", &body).await;
        reply.expect_error(case, 400, expected);
    }

    // The same token, with the same filters, in the Region it was NOT issued for: an
    // import belongs to one Region, so the token resumes nothing there.
    let elsewhere = emulator
        .call_in(
            "ListImports",
            OTHER_REGION,
            &json!({ "ImportStatus": IMPORT_STATUS_IN_PROGRESS, "NextToken": issued }),
        )
        .await;
    elsewhere.expect_error(
        "the token reused in another Region",
        400,
        "InvalidNextTokenException",
    );
}

/// A `Destination` readable as neither an event data store ARN nor the id suffix of one
/// is refused with `EventDataStoreARNInvalidException`, naming the ARN it addressed; a
/// readable one is served.
#[tokio::test]
async fn list_imports_refuses_an_unreadable_destination() {
    let emulator = Emulator::new();
    seed_registry(&emulator);

    // ARN-shaped values that are not event data store ARNs, each within the modelled
    // `EventDataStoreArn` pattern `^[a-zA-Z0-9._/\-:]+$` so the boundary lets them
    // through to the handler.
    let rows: [(&str, String); 4] = [
        (
            "a CloudTrail ARN of another resource type",
            format!("arn:aws:cloudtrail:{HOME_REGION}:000000000000:trail/my-trail"),
        ),
        (
            "an ARN of another service",
            format!("arn:aws:s3:{HOME_REGION}:000000000000:eventdatastore/{STORE_A}"),
        ),
        (
            "an ARN missing a field",
            format!("arn:aws:cloudtrail:{HOME_REGION}:eventdatastore/{STORE_A}"),
        ),
        (
            "an ARN naming no store id",
            format!("arn:aws:cloudtrail:{HOME_REGION}:000000000000:eventdatastore/"),
        ),
    ];

    for (case, destination) in rows {
        let reply = emulator
            .call("ListImports", &json!({ "Destination": destination }))
            .await;
        reply.expect_error(case, 400, "EventDataStoreARNInvalidException");
        assert_eq!(
            string_member(&reply.body, "Message", case),
            event_data_store_arn_invalid_message(&destination, HOME_REGION),
            "case {case}: expected a message naming the ARN addressed, got {:?}",
            reply.body
        );
    }

    // A `Destination` under the modelled length (3..256) never reaches the handler.
    let short = emulator
        .call("ListImports", &json!({ "Destination": "ab" }))
        .await;
    short.expect_error(
        "a Destination under the modelled minimum length",
        400,
        "ValidationException",
    );

    // A refusal changed nothing, and a readable Destination is still served.
    let served = emulator
        .call("ListImports", &json!({ "Destination": STORE_A }))
        .await;
    let body = served.expect_ok("a readable Destination").clone();
    assert_eq!(
        reported_ids(&body, "a readable Destination"),
        vec![import_id(1), import_id(2), import_id(5)],
        "a readable Destination must still select its imports, got {body:?}"
    );
}

/// The listing is computed from the registry, not from a snapshot: an import another
/// operation moved is reported in the status and with the timestamp that operation
/// wrote.
#[tokio::test]
async fn list_imports_reports_what_stop_import_wrote() {
    let emulator = Emulator::new();
    seed_registry(&emulator);
    let case = "a stopped import";

    let stopped = emulator
        .call("StopImport", &json!({ "ImportId": import_id(2) }))
        .await;
    let body = stopped.expect_ok(case).clone();
    let stamped = seconds_member(&body, "UpdatedTimestamp", case);

    // The import that was `IN_PROGRESS` is now reported by the `STOPPED` listing...
    let listed = emulator
        .call(
            "ListImports",
            &json!({ "ImportStatus": IMPORT_STATUS_STOPPED }),
        )
        .await;
    let listed = listed.expect_ok(case).clone();
    assert_eq!(
        reported_ids(&listed, case),
        vec![import_id(2), import_id(6)],
        "case {case}: the listing must report the stop, got {listed:?}"
    );
    let item = items(&listed, case)
        .into_iter()
        .find(|item| string_member(item, "ImportId", case) == import_id(2))
        .expect("the stopped import");
    assert_eq!(
        seconds_member(&item, "UpdatedTimestamp", case),
        stamped,
        "case {case}: the listing must report the UpdatedTimestamp the stop stamped, \
         got {item:?}"
    );
    assert!(
        stamped > SEEDED_AT_EPOCH_SECONDS as f64,
        "case {case}: the stop must have stamped an instant later than the seeded one, \
         got {stamped}"
    );

    // ...and no longer by the `IN_PROGRESS` one.
    let remaining = emulator
        .call(
            "ListImports",
            &json!({ "ImportStatus": IMPORT_STATUS_IN_PROGRESS }),
        )
        .await;
    let remaining = remaining.expect_ok(case).clone();
    assert_eq!(
        reported_ids(&remaining, case),
        vec![import_id(3)],
        "case {case}: a stopped import must leave the IN_PROGRESS listing, got {remaining:?}"
    );

    // The elapsed instant the stop stamped is the registry's, not the reply's.
    let store = emulator.state();
    let held = store.lock();
    let persisted = held
        .import(&import_id(2), HOME_REGION)
        .expect("the stopped import")
        .clone();
    assert_eq!(
        persisted.status, IMPORT_STATUS_STOPPED,
        "case {case}: the registry must hold STOPPED, got {:?}",
        persisted.status
    );
    assert!(
        persisted.updated_timestamp > UNIX_EPOCH + Duration::from_secs(SEEDED_AT_EPOCH_SECONDS),
        "case {case}: the registry must hold the stamped instant, got {:?}",
        persisted.updated_timestamp
    );
}
