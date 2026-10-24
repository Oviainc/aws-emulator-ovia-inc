// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListQueries` — listing CloudTrail Lake query runs.
//!
//! The rules under test, each grounded in this service's Smithy model:
//!
//! * `ListQueries` REPORTS the persisted runs of ONE event data store: every
//!   member of the modelled `Query` (`QueryId`, `QueryStatus`, `CreationTime`) is
//!   the value the stored run carries, for every value of the model's
//!   `QueryStatus` enum, and no other store's runs are listed;
//! * it REPORTS: its only write is to materialise the end the clock already owed
//!   a run ([`state::QueryRecord::settle`], the call `DescribeQuery` makes too),
//!   so a run inside its settle span keeps the status it had however often it is
//!   listed, and two calls in a row report the same thing;
//! * "You must specify an ARN value for `EventDataStore`": an unreadable
//!   identifier is `EventDataStoreARNInvalidException`, a readable one naming no
//!   store in the Region it addresses is `EventDataStoreNotFoundException` —
//!   naming the store it could not find — and a store awaiting deletion is
//!   `InactiveEventDataStoreException`;
//! * "Returns a list of queries and query statuses for the past seven days": a
//!   run older than the window is not listed, and a `StartTime` reaching further
//!   back does not widen it;
//! * "Optionally … you can specify a time range … and a `QueryStatus` value":
//!   `StartTime`/`EndTime` bound the run's creation time inclusively, and
//!   `QueryStatus` keeps only the runs in that state. "Be sure that the start time
//!   is chronologically before the end time" is `InvalidDateRangeException`;
//! * `MaxResults` bounds one page and `NextToken` resumes the next, so the pages
//!   of one traversal partition the full listing exactly once; a token this
//!   service did not issue, or one issued for a request with different parameters
//!   (another store, another filter, another page size), is
//!   `InvalidNextTokenException`.
//!
//! `EventDataStoreNotFoundException` answers HTTP 400 — the status CloudTrail
//! answers with, which is not the `@httpError(404)` its model annotates.
//!
//! The round-trip case mints its store through `CreateEventDataStore` and its
//! runs through `StartQuery`, and reads each one back through `DescribeQuery`, so
//! what the listing reports is what the API persisted. The other cases seed the
//! SAME registries the handler reads, through the shared engine, and then go over
//! the wire for every assertion: they need runs in statuses, Regions and ages no
//! `StartQuery` produces (a run that failed, timed out or was cancelled, a run of
//! another Region, a run older than the history window).

use serde_json::{json, Value};
use std::time::{Duration, SystemTime};

use service_com_amazonaws_cloudtrail_cloudtrail_20131101::ops::list_queries::QUERY_HISTORY_WINDOW;
use service_com_amazonaws_cloudtrail_cloudtrail_20131101::ops::types::QUERY_STATUS_VALUES;
use service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::{
    self, event_data_store_arn, EventDataStoreRecord, EventDataStoreState, QueryRecord, QueryState,
};

use super::emulator::{Emulator, HOME_REGION};

/// The id of the event data store most cases list: an event data store id is the
/// resource suffix of its ARN.
const STORE_ID: &str = "bbbbbbbb-1111-2222-3333-444444444444";

/// A second store, for the cases that pin that a listing holds only the runs of
/// the store it names.
const OTHER_STORE_ID: &str = "cccccccc-1111-2222-3333-444444444444";

/// The HTTP status this service answers a not-found refusal with: the model
/// annotates `EventDataStoreNotFoundException` `@httpError(404)`, and the recorded
/// CloudTrail response is 400.
const NOT_FOUND_STATUS: u16 = 400;

/// The operation under test, as the `X-Amz-Target` names it.
const LIST_QUERIES: &str = "ListQueries";

/// A `UUID` is exactly 36 characters of `[a-f0-9-]`, so a query id is built to that
/// shape — the one a query id carries on the wire.
fn query_id(nth: u8) -> String {
    format!("{nth:02x}bbbbbb-1111-2222-3333-444444444444")
}

/// `secs` seconds before now, truncated to a whole second so the reported
/// `CreationTime` compares exactly against what was seeded.
fn seconds_ago(secs: u64) -> SystemTime {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("now is after the epoch")
        .as_secs();
    SystemTime::UNIX_EPOCH + Duration::from_secs(now - secs)
}

/// An instant as the whole epoch seconds a modelled timestamp carries on the wire.
fn epoch_secs(at: SystemTime) -> u64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .expect("an instant after the epoch")
        .as_secs()
}

/// Seed one event data store, the way `CreateEventDataStore` will once it is
/// implemented.
fn seed_store(emulator: &Emulator, id: &str, region: &str, status: EventDataStoreState) {
    emulator.state().lock().event_data_stores.insert(
        id.to_string(),
        EventDataStoreRecord {
            id: id.to_string(),
            name: format!("store-{id}"),
            region: region.to_string(),
            status,
            owner_account_id: None,
            created_at: Some(seconds_ago(3_600)),
            // The members only a read of the store itself reports are not what
            // these cases vary.
            ..EventDataStoreRecord::default()
        },
    );
}

/// A margin on top of [`state::QUERY_SETTLE_AFTER`], so a case that waits for a
/// run to end waits past the instant it is due rather than exactly on it.
fn past_settle() -> Duration {
    state::QUERY_SETTLE_AFTER + Duration::from_millis(250)
}

/// Seed one query run, the way `StartQuery` mints one — used by the cases that
/// need a status, a Region or an age a started run cannot have.
fn seed_query(emulator: &Emulator, record: QueryRecord) {
    emulator
        .state()
        .lock()
        .queries
        .insert(record.query_id.clone(), record);
}

/// A run of [`STORE_ID`] in [`HOME_REGION`], created `age_secs` ago.
fn run(nth: u8, status: QueryState, age_secs: u64) -> QueryRecord {
    QueryRecord {
        query_id: query_id(nth),
        event_data_store_id: STORE_ID.to_string(),
        region: HOME_REGION.to_string(),
        query_string: format!("SELECT eventName FROM {STORE_ID} LIMIT {nth}"),
        status,
        created_at: Some(seconds_ago(age_secs)),
        ..QueryRecord::default()
    }
}

/// The `QueryId`s one reply lists, in the order it listed them.
fn listed_ids(case: &str, body: &Value) -> Vec<String> {
    body.get("Queries")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case}: expected a Queries list, got {body:?}"))
        .iter()
        .map(|query| {
            query
                .get("QueryId")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case}: expected a QueryId, got {query:?}"))
                .to_string()
        })
        .collect()
}

/// List every run of `store` with no filter at all.
async fn list_all(emulator: &Emulator, store: &str) -> Value {
    emulator
        .call(LIST_QUERIES, &json!({ "EventDataStore": store }))
        .await
        .expect_ok("list every run")
        .clone()
}

/// Every member of the modelled `Query` is the value the stored run carries, for
/// every value of the model's `QueryStatus` enum.
#[tokio::test]
async fn list_queries_reports_every_persisted_run() {
    let emulator = Emulator::new();
    seed_store(&emulator, STORE_ID, HOME_REGION, EventDataStoreState::Enabled);

    // One row per `QueryStatus` value the model accepts — the catalog comes from
    // the model itself (`ops::types::QUERY_STATUS_VALUES`), so a value this crate
    // cannot store would fail the pairing below rather than go untested.
    let states = [
        QueryState::Queued,
        QueryState::Running,
        QueryState::Finished,
        QueryState::Failed,
        QueryState::Cancelled,
        QueryState::TimedOut,
    ];
    let mut covered: Vec<&str> = states.iter().map(|state| state.as_wire()).collect();
    covered.sort_unstable();
    assert_eq!(
        covered, QUERY_STATUS_VALUES,
        "case catalog: expected the model's values {QUERY_STATUS_VALUES:?}, got {covered:?}"
    );

    let records: Vec<QueryRecord> = states
        .iter()
        .enumerate()
        .map(|(nth, status)| run(nth as u8 + 1, *status, 60 * (nth as u64 + 1)))
        .collect();
    for record in &records {
        seed_query(&emulator, record.clone());
    }

    let body = list_all(&emulator, STORE_ID).await;
    let queries = body
        .get("Queries")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case all runs: expected a Queries list, got {body:?}"));
    assert_eq!(
        queries.len(),
        records.len(),
        "case all runs: expected {} queries, got {body:?}",
        records.len()
    );
    assert!(
        body.get("NextToken").is_none() || body["NextToken"].is_null(),
        "case all runs: expected no NextToken on a single page, got {body:?}"
    );

    // The registry is keyed by query id, so the listing reports id order.
    for record in &records {
        let listed = queries
            .iter()
            .find(|query| query.get("QueryId").and_then(Value::as_str) == Some(&record.query_id))
            .unwrap_or_else(|| {
                panic!(
                    "case {}: expected the run to be listed, got {body:?}",
                    record.query_id
                )
            });
        assert_eq!(
            listed.get("QueryStatus").and_then(Value::as_str),
            Some(record.status.as_wire()),
            "case {}: expected QueryStatus {:?}, got {listed:?}",
            record.query_id,
            record.status.as_wire()
        );
        let created_at = record.created_at.map(|at| epoch_secs(at) as f64);
        assert_eq!(
            listed.get("CreationTime").and_then(Value::as_f64),
            created_at,
            "case {}: expected CreationTime {created_at:?}, got {listed:?}",
            record.query_id
        );
    }

    let ids = listed_ids("all runs", &body);
    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(
        ids, sorted,
        "case all runs: expected the listing in query-id order, got {ids:?}"
    );
}

/// A listing holds the runs of the store it names, in the Region that store lives
/// in, and nothing else.
#[tokio::test]
async fn list_queries_lists_only_the_named_stores_runs() {
    let emulator = Emulator::new();
    seed_store(&emulator, STORE_ID, HOME_REGION, EventDataStoreState::Enabled);
    seed_store(
        &emulator,
        OTHER_STORE_ID,
        HOME_REGION,
        EventDataStoreState::Enabled,
    );

    seed_query(&emulator, run(1, QueryState::Finished, 60));
    seed_query(&emulator, run(2, QueryState::Running, 120));
    // A run of the OTHER store, and one of this store recorded in another Region:
    // neither belongs to this store's listing.
    seed_query(
        &emulator,
        QueryRecord {
            event_data_store_id: OTHER_STORE_ID.to_string(),
            ..run(3, QueryState::Finished, 60)
        },
    );
    seed_query(
        &emulator,
        QueryRecord {
            region: "eu-west-1".to_string(),
            ..run(4, QueryState::Finished, 60)
        },
    );

    for (case, store, expected) in [
        (
            "the named store",
            STORE_ID,
            vec![query_id(1), query_id(2)],
        ),
        ("the other store", OTHER_STORE_ID, vec![query_id(3)]),
    ] {
        let body = list_all(&emulator, store).await;
        assert_eq!(
            listed_ids(case, &body),
            expected,
            "case {case}: expected {expected:?}, got {body:?}"
        );
    }

    // The same store named by its ARN is the same store.
    let by_arn = emulator
        .call(
            LIST_QUERIES,
            &json!({ "EventDataStore": event_data_store_arn(HOME_REGION, STORE_ID) }),
        )
        .await;
    assert_eq!(
        listed_ids("by ARN", by_arn.expect_ok("by ARN")),
        vec![query_id(1), query_id(2)],
        "case by ARN: expected the same listing as by id, got {by_arn:?}"
    );
}

/// "Returns a list of queries and query statuses for the past seven days": the
/// window bounds the listing, and a `StartTime` reaching further back does not
/// widen it.
#[tokio::test]
async fn list_queries_reports_only_the_history_window() {
    let emulator = Emulator::new();
    seed_store(&emulator, STORE_ID, HOME_REGION, EventDataStoreState::Enabled);

    let window = QUERY_HISTORY_WINDOW.as_secs();
    // Rows span the window: just now, well inside, just inside, just outside and
    // far outside.
    let rows: [(u8, u64, bool); 5] = [
        (1, 1, true),
        (2, window / 2, true),
        (3, window - 60, true),
        (4, window + 60, false),
        (5, window * 4, false),
    ];
    for (nth, age_secs, _) in rows {
        seed_query(&emulator, run(nth, QueryState::Finished, age_secs));
    }

    let body = list_all(&emulator, STORE_ID).await;
    let listed = listed_ids("history window", &body);
    for (nth, age_secs, expected) in rows {
        assert_eq!(
            listed.contains(&query_id(nth)),
            expected,
            "case {age_secs}s old: expected listed={expected}, got {body:?}"
        );
    }

    // A StartTime a month back cannot reach past the window.
    let widened = emulator
        .call(
            LIST_QUERIES,
            &json!({
                "EventDataStore": STORE_ID,
                "StartTime": epoch_secs(seconds_ago(window * 4)),
            }),
        )
        .await;
    assert_eq!(
        listed_ids("widened", widened.expect_ok("widened")),
        listed,
        "case widened: a StartTime past the window must not widen it, got {widened:?}"
    );
}

/// `StartTime` and `EndTime` bound the run's creation time inclusively.
#[tokio::test]
async fn list_queries_bounds_the_time_range() {
    let emulator = Emulator::new();
    seed_store(&emulator, STORE_ID, HOME_REGION, EventDataStoreState::Enabled);

    // Four runs, 100 seconds apart, youngest first.
    let ages = [100_u64, 200, 300, 400];
    for (nth, age) in ages.iter().enumerate() {
        seed_query(&emulator, run(nth as u8 + 1, QueryState::Finished, *age));
    }
    let at = |age: u64| epoch_secs(seconds_ago(age));

    // Each row is a bound pair and the runs it keeps: the bounds are inclusive, so
    // a bound landing exactly on a creation time keeps that run.
    let rows: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "no bounds",
            json!({ "EventDataStore": STORE_ID }),
            vec![query_id(1), query_id(2), query_id(3), query_id(4)],
        ),
        (
            "start only",
            json!({ "EventDataStore": STORE_ID, "StartTime": at(250) }),
            vec![query_id(1), query_id(2)],
        ),
        (
            "end only",
            json!({ "EventDataStore": STORE_ID, "EndTime": at(250) }),
            vec![query_id(3), query_id(4)],
        ),
        (
            "both bounds",
            json!({ "EventDataStore": STORE_ID, "StartTime": at(350), "EndTime": at(150) }),
            vec![query_id(2), query_id(3)],
        ),
        (
            "bounds on the creation times",
            json!({ "EventDataStore": STORE_ID, "StartTime": at(300), "EndTime": at(200) }),
            vec![query_id(2), query_id(3)],
        ),
        (
            "a range holding nothing",
            json!({ "EventDataStore": STORE_ID, "StartTime": at(199), "EndTime": at(101) }),
            Vec::new(),
        ),
    ];

    for (case, request, expected) in rows {
        let reply = emulator.call(LIST_QUERIES, &request).await;
        assert_eq!(
            listed_ids(case, reply.expect_ok(case)),
            expected,
            "case {case}: expected {expected:?}, got {reply:?}"
        );
    }
}

/// "Be sure that the start time is chronologically before the end time": a range
/// that is not is refused with `InvalidDateRangeException`.
#[tokio::test]
async fn list_queries_refuses_a_range_that_is_not_chronological() {
    let emulator = Emulator::new();
    seed_store(&emulator, STORE_ID, HOME_REGION, EventDataStoreState::Enabled);
    seed_query(&emulator, run(1, QueryState::Finished, 60));

    let at = |age: u64| epoch_secs(seconds_ago(age));
    for (case, start, end) in [
        ("start after end", at(100), at(200)),
        ("start equals end", at(100), at(100)),
        ("start one second after end", at(100), at(101)),
    ] {
        let reply = emulator
            .call(
                LIST_QUERIES,
                &json!({ "EventDataStore": STORE_ID, "StartTime": start, "EndTime": end }),
            )
            .await;
        reply.expect_error(case, 400, "InvalidDateRangeException");
    }

    // A range that IS chronological is served.
    let ok = emulator
        .call(
            LIST_QUERIES,
            &json!({ "EventDataStore": STORE_ID, "StartTime": at(200), "EndTime": at(1) }),
        )
        .await;
    assert_eq!(
        listed_ids("chronological", ok.expect_ok("chronological")),
        vec![query_id(1)],
        "case chronological: expected the run to be listed, got {ok:?}"
    );
}

/// `QueryStatus` keeps only the runs in that state — one row per value the model's
/// `QueryStatus` enum accepts.
#[tokio::test]
async fn list_queries_filters_by_query_status() {
    let emulator = Emulator::new();
    seed_store(&emulator, STORE_ID, HOME_REGION, EventDataStoreState::Enabled);

    let states = [
        QueryState::Queued,
        QueryState::Running,
        QueryState::Finished,
        QueryState::Failed,
        QueryState::Cancelled,
        QueryState::TimedOut,
    ];
    // Two runs per state, so a filter that ignored the state could not pass.
    for (nth, status) in states.iter().enumerate() {
        seed_query(&emulator, run(nth as u8 + 1, *status, 60));
        seed_query(&emulator, run(nth as u8 + 101, *status, 120));
    }

    // The catalog of filter values is the model's own.
    for status in QUERY_STATUS_VALUES {
        let nth = states
            .iter()
            .position(|state| state.as_wire() == *status)
            .unwrap_or_else(|| panic!("case {status}: no stored state carries this wire value"));
        let reply = emulator
            .call(
                LIST_QUERIES,
                &json!({ "EventDataStore": STORE_ID, "QueryStatus": status }),
            )
            .await;
        let expected = vec![query_id(nth as u8 + 1), query_id(nth as u8 + 101)];
        let mut listed = listed_ids(status, reply.expect_ok(status));
        listed.sort();
        let mut expected_sorted = expected.clone();
        expected_sorted.sort();
        assert_eq!(
            listed, expected_sorted,
            "case {status}: expected {expected_sorted:?}, got {reply:?}"
        );
    }
}

/// "You must specify an ARN value for `EventDataStore`": how each identifier form
/// resolves, and which refusal each failure answers with.
#[tokio::test]
async fn list_queries_resolves_the_event_data_store() {
    let emulator = Emulator::new();
    seed_store(&emulator, STORE_ID, HOME_REGION, EventDataStoreState::Enabled);
    seed_query(&emulator, run(1, QueryState::Finished, 60));
    // A store awaiting deletion holds no queryable lake any more.
    let pending = "dddddddd-1111-2222-3333-444444444444";
    seed_store(
        &emulator,
        pending,
        HOME_REGION,
        EventDataStoreState::PendingDeletion,
    );
    // A store in another Region: reachable by its ARN, not by a bare id from here.
    let elsewhere = "eeeeeeee-1111-2222-3333-444444444444";
    seed_store(&emulator, elsewhere, "eu-west-1", EventDataStoreState::Enabled);

    // Each row is an identifier and the refusal it earns.
    let unknown = "ffffffff-1111-2222-3333-444444444444";
    let rows: Vec<(&str, String, &str)> = vec![
        // Shaped like an ARN but not an event data store ARN.
        (
            "an ARN of another service",
            "arn:aws:s3:us-east-1:000000000000:eventdatastore/x".to_string(),
            "EventDataStoreARNInvalidException",
        ),
        (
            "an ARN of another resource type",
            format!("arn:aws:cloudtrail:{HOME_REGION}:000000000000:trail/not-a-store"),
            "EventDataStoreARNInvalidException",
        ),
        (
            "an ARN missing its account",
            format!("arn:aws:cloudtrail:{HOME_REGION}::eventdatastore/{STORE_ID}"),
            "EventDataStoreARNInvalidException",
        ),
        // Readable, but naming no store in the Region it addresses.
        (
            "an unknown store id",
            unknown.to_string(),
            "EventDataStoreNotFoundException",
        ),
        (
            "an unknown store ARN",
            event_data_store_arn(HOME_REGION, unknown),
            "EventDataStoreNotFoundException",
        ),
        (
            "a store in another Region, by bare id",
            elsewhere.to_string(),
            "EventDataStoreNotFoundException",
        ),
        (
            "a store ARN in the wrong Region",
            event_data_store_arn("eu-west-1", STORE_ID),
            "EventDataStoreNotFoundException",
        ),
        // Found, but not queryable.
        (
            "a store awaiting deletion",
            pending.to_string(),
            "InactiveEventDataStoreException",
        ),
    ];

    for (case, identifier, code) in rows {
        let reply = emulator
            .call(LIST_QUERIES, &json!({ "EventDataStore": identifier }))
            .await;
        let status = if code == "EventDataStoreNotFoundException" {
            NOT_FOUND_STATUS
        } else {
            400
        };
        reply.expect_error(case, status, code);
    }

    // The refusal NAMES the store that could not be found, as an ARN built from
    // the request: the identifier itself when it was an ARN, and the ARN of the
    // store id in the addressed Region when it was a bare id.
    for (case, identifier, expected_arn) in [
        (
            "bare id",
            unknown.to_string(),
            event_data_store_arn(HOME_REGION, unknown),
        ),
        (
            "full ARN",
            event_data_store_arn(HOME_REGION, unknown),
            event_data_store_arn(HOME_REGION, unknown),
        ),
    ] {
        let reply = emulator
            .call(LIST_QUERIES, &json!({ "EventDataStore": identifier }))
            .await;
        let expected = format!("The event data store {expected_arn} was not found.");
        assert_eq!(
            reply.body.get("Message").and_then(Value::as_str),
            Some(expected.as_str()),
            "case {case}: expected Message {expected:?}, got {reply:?}"
        );
    }

    // The store that IS queryable still serves its runs.
    let ok = emulator
        .call(LIST_QUERIES, &json!({ "EventDataStore": STORE_ID }))
        .await;
    assert_eq!(
        listed_ids("the live store", ok.expect_ok("the live store")),
        vec![query_id(1)],
        "case the live store: expected the run to be listed, got {ok:?}"
    );
}

/// `MaxResults` bounds one page and `NextToken` resumes the next: whatever the page
/// size, the pages of one traversal partition the full listing exactly once.
#[tokio::test]
async fn list_queries_pages_the_listing() {
    let emulator = Emulator::new();
    seed_store(&emulator, STORE_ID, HOME_REGION, EventDataStoreState::Enabled);

    let total = 7_u8;
    for nth in 1..=total {
        seed_query(
            &emulator,
            run(nth, QueryState::Finished, 60 * u64::from(nth)),
        );
    }
    let everything = listed_ids("unpaged", &list_all(&emulator, STORE_ID).await);
    assert_eq!(
        everything.len(),
        usize::from(total),
        "case unpaged: expected {total} runs, got {everything:?}"
    );

    // Rows span the page sizes: one at a time, an uneven split, exactly the total,
    // and more than the total.
    for page_size in [1_usize, 2, 3, usize::from(total), usize::from(total) + 5] {
        let case = format!("MaxResults={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({ "EventDataStore": STORE_ID, "MaxResults": page_size });
            if let Some(token) = &token {
                request["NextToken"] = json!(token);
            }
            let reply = emulator.call(LIST_QUERIES, &request).await;
            let body = reply.expect_ok(&case);
            let page = listed_ids(&case, body);
            assert!(
                page.len() <= page_size,
                "case {case}: a page carried {} runs, got {body:?}",
                page.len()
            );
            seen.extend(page);
            pages += 1;
            assert!(
                pages <= usize::from(total) + 1,
                "case {case}: the traversal did not end, got {seen:?}"
            );
            token = body
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, everything,
            "case {case}: the pages must partition the listing exactly once, got {seen:?}"
        );
    }
}

/// "A token that is not valid, or a token that was previously used in a request
/// with different parameters": both are `InvalidNextTokenException`.
#[tokio::test]
async fn list_queries_refuses_a_token_it_did_not_issue() {
    let emulator = Emulator::new();
    seed_store(&emulator, STORE_ID, HOME_REGION, EventDataStoreState::Enabled);
    seed_store(
        &emulator,
        OTHER_STORE_ID,
        HOME_REGION,
        EventDataStoreState::Enabled,
    );
    for nth in 1..=4_u8 {
        seed_query(
            &emulator,
            run(nth, QueryState::Finished, 60 * u64::from(nth)),
        );
        seed_query(
            &emulator,
            QueryRecord {
                event_data_store_id: OTHER_STORE_ID.to_string(),
                ..run(nth + 100, QueryState::Finished, 60 * u64::from(nth))
            },
        );
    }

    // A token issued for a one-at-a-time traversal of this store.
    let first = emulator
        .call(
            LIST_QUERIES,
            &json!({ "EventDataStore": STORE_ID, "MaxResults": 1 }),
        )
        .await;
    let token = first
        .expect_ok("first page")
        .get("NextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case first page: expected a NextToken, got {first:?}"))
        .to_string();

    // Each row is a request carrying a token that is not valid for it.
    let rows: Vec<(&str, Value)> = vec![
        (
            "a token this service never issued",
            json!({ "EventDataStore": STORE_ID, "NextToken": "not-a-token" }),
        ),
        (
            "a value shaped like a token but with the wrong version",
            json!({ "EventDataStore": STORE_ID, "NextToken": "v9:0123456789abcdef:somewhere"}),
        ),
        (
            "the same token against another store",
            json!({ "EventDataStore": OTHER_STORE_ID, "MaxResults": 1, "NextToken": token }),
        ),
        (
            "the same token with a status filter added",
            json!({
                "EventDataStore": STORE_ID,
                "MaxResults": 1,
                "QueryStatus": "FINISHED",
                "NextToken": token,
            }),
        ),
        (
            "the same token with a time bound added",
            json!({
                "EventDataStore": STORE_ID,
                "MaxResults": 1,
                "StartTime": epoch_secs(seconds_ago(300)),
                "NextToken": token,
            }),
        ),
        (
            "the same token with a different page size",
            json!({ "EventDataStore": STORE_ID, "MaxResults": 2, "NextToken": token }),
        ),
        (
            "the same token with no page size",
            json!({ "EventDataStore": STORE_ID, "NextToken": token }),
        ),
    ];
    for (case, request) in rows {
        let reply = emulator.call(LIST_QUERIES, &request).await;
        reply.expect_error(case, 400, "InvalidNextTokenException");
    }

    // The token IS valid for the request it was issued for, page size included.
    let resumed = emulator
        .call(
            LIST_QUERIES,
            &json!({ "EventDataStore": STORE_ID, "MaxResults": 1, "NextToken": token }),
        )
        .await;
    assert_eq!(
        listed_ids("resumed", resumed.expect_ok("resumed")),
        vec![query_id(2)],
        "case resumed: expected the second run, got {resumed:?}"
    );
}

/// `ListQueries` REPORTS: its only write is the end the clock already owed a run,
/// so listing a run inside its settle span leaves it where it was however often it
/// is listed, and a run whose settle instant has passed is listed — and persisted
/// — in the state it had already reached.
///
/// The rows span what the clock owes a run: nothing (a terminal run), a transition
/// not yet due, and one already due.
#[tokio::test]
async fn list_queries_only_materialises_the_end_the_clock_owed() {
    let emulator = Emulator::new();
    seed_store(&emulator, STORE_ID, HOME_REGION, EventDataStoreState::Enabled);

    let now = SystemTime::now();
    // (case, seeded status, settle instant, the status a listing must report)
    let rows: [(&str, u8, QueryState, Option<SystemTime>, QueryState); 4] = [
        (
            "a terminal run the clock owes nothing",
            1,
            QueryState::Finished,
            None,
            QueryState::Finished,
        ),
        (
            "a cancelled run past its settle instant",
            2,
            QueryState::Cancelled,
            Some(now - Duration::from_secs(1)),
            QueryState::Cancelled,
        ),
        (
            "a queued run whose settle instant is still ahead",
            3,
            QueryState::Queued,
            Some(now + Duration::from_secs(3_600)),
            QueryState::Queued,
        ),
        (
            "a queued run whose settle instant has passed",
            4,
            QueryState::Queued,
            Some(now - Duration::from_secs(1)),
            QueryState::Finished,
        ),
    ];
    for (_, nth, status, settles_at, _) in rows {
        seed_query(
            &emulator,
            QueryRecord {
                settles_at,
                ..run(nth, status, 60 * u64::from(nth))
            },
        );
    }

    let first = list_all(&emulator, STORE_ID).await;
    for (case, nth, _, _, expected) in rows {
        let id = query_id(nth);
        let listed = first["Queries"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected a Queries list, got {first:?}"))
            .iter()
            .find(|query| query.get("QueryId").and_then(Value::as_str) == Some(id.as_str()))
            .unwrap_or_else(|| panic!("case {case}: expected the run listed, got {first:?}"));
        assert_eq!(
            listed.get("QueryStatus").and_then(Value::as_str),
            Some(expected.as_wire()),
            "case {case}: expected QueryStatus {:?}, got {listed:?}",
            expected.as_wire()
        );
        // And the registry holds that same status: the transition was the clock's,
        // so materialising it changed nothing a later caller would not have seen.
        let persisted = emulator
            .state()
            .lock()
            .queries
            .get(&id)
            .map(|record| record.status.as_wire().to_string());
        assert_eq!(
            persisted.as_deref(),
            Some(expected.as_wire()),
            "case {case}: expected the registry to hold {:?}, got {persisted:?}",
            expected.as_wire()
        );
    }

    // Listing again reports exactly the same thing: the read is not what moves a
    // run, so the run still inside its settle span is still queued.
    let second = list_all(&emulator, STORE_ID).await;
    assert_eq!(
        first, second,
        "case repeated read: expected the same listing twice, got {first:?} then {second:?}"
    );
}

/// RULE: a listing reports the runs the API persisted — so a store minted through
/// `CreateEventDataStore` and runs started through `StartQuery` are exactly what
/// `ListQueries` lists, by the id and by the ARN that create answered with, and
/// every run it lists carries the status `DescribeQuery` reports for it.
///
/// The rows differ in the statement the run carries; `StartQuery` mints one run
/// per call, so the ids are the API's own.
#[tokio::test]
async fn list_queries_lists_the_runs_the_api_minted() {
    let emulator = Emulator::new();
    let created = emulator
        .call(
            "CreateEventDataStore",
            &json!({ "Name": "lake-store-for-listing" }),
        )
        .await;
    let arn = created
        .expect_ok("CreateEventDataStore")
        .get("EventDataStoreArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case create: expected an EventDataStoreArn, got {created:?}"))
        .to_string();
    // An event data store id is the resource suffix of its ARN, which is how every
    // operation of this service reads one.
    let id = arn
        .rsplit('/')
        .next()
        .expect("an event data store ARN carries its id")
        .to_string();

    let statements = [
        format!("SELECT eventName FROM {id}"),
        format!("SELECT * FROM {arn} WHERE eventSource = 'cloudtrail.amazonaws.com'"),
        format!("select count(*) as total from {id}"),
    ];
    assert!(
        statements.len() <= state::MAX_CONCURRENT_QUERIES,
        "case table size: {} runs would pass the concurrency limit of {}",
        statements.len(),
        state::MAX_CONCURRENT_QUERIES
    );
    let mut minted: Vec<String> = Vec::new();
    for statement in &statements {
        let reply = emulator
            .call("StartQuery", &json!({ "QueryStatement": statement }))
            .await;
        let body = reply.expect_ok(statement);
        minted.push(
            body.get("QueryId")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {statement:?}: expected a QueryId, got {body:?}"))
                .to_string(),
        );
    }
    minted.sort();

    // The clock owes each new run its end; past that instant every run has ended,
    // which is the status both readers report.
    tokio::time::sleep(past_settle()).await;

    for (case, store) in [("by id", id.as_str()), ("by ARN", arn.as_str())] {
        let body = list_all(&emulator, store).await;
        let mut listed = listed_ids(case, &body);
        listed.sort();
        assert_eq!(
            listed, minted,
            "case {case}: expected the runs the API minted {minted:?}, got {body:?}"
        );

        for query in body["Queries"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected a Queries list, got {body:?}"))
        {
            let query_id = query
                .get("QueryId")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case}: expected a QueryId, got {query:?}"));
            assert_eq!(
                query.get("QueryStatus").and_then(Value::as_str),
                Some(QueryState::Finished.as_wire()),
                "case {case}: past its settle instant a run has ended, got {query:?}"
            );
            assert!(
                query.get("CreationTime").and_then(Value::as_f64).is_some(),
                "case {case}: every listed run carries its CreationTime, got {query:?}"
            );

            // The two readers of one registry agree: what the listing reports is
            // what a read of that one run reports.
            let described = emulator
                .call("DescribeQuery", &json!({ "QueryId": query_id }))
                .await;
            let described = described.expect_ok(case);
            assert_eq!(
                described.get("QueryStatus"),
                query.get("QueryStatus"),
                "case {case}: DescribeQuery and ListQueries must report one status, got \
                 {described:?} and {query:?}"
            );
        }
    }
}
