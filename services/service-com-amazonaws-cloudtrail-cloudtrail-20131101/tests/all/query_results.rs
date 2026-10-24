// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetQueryResults` — the rows and status of a Lake query.
//!
//! The rules under test, each grounded in this service's Smithy model:
//!
//! * `GetQueryResults` REPORTS a persisted query: its status (every value of the
//!   model's `QueryStatus` enum), the rows the run produced and the scan statistics of
//!   that run, and `ErrorMessage` exactly when the run recorded one;
//! * `QueryResultRows` is paged by `MaxQueryResults`, and `NextToken` resumes exactly
//!   where the previous page stopped, in row order, with no row dropped or repeated;
//!   `QueryStatistics.ResultsCount` counts THIS page and `TotalResultsCount` every row
//!   the query produced;
//! * a `NextToken` this query did not hand out is refused with
//!   `InvalidNextTokenException` (HTTP 400);
//! * a query is addressed by `QueryId` and is reachable only from the Region it ran in
//!   and through the store it ran on, which `EventDataStore` (deprecated here) and
//!   `EventDataStoreOwnerAccountId` only narrow; an unaddressed id is refused with
//!   `QueryIdNotFoundException`, naming the member and the value the request supplied,
//!   with the HTTP 400 CloudTrail answers that refusal with (its model annotates
//!   `@httpError(404)`);
//! * `EventDataStore` is "the ARN (or the ID suffix of the ARN) of an event data
//!   store": one that can be read as neither is `EventDataStoreARNInvalidException`
//!   (HTTP 400);
//! * reading writes nothing a later caller would not have seen anyway: two calls in a
//!   row report the same thing, and the only write is the transition the CLOCK already
//!   made due — a run past its settle instant had ended before the call was made.
//!
//! The round-trip case mints its query through the implemented `StartQuery` on a store
//! minted through `CreateEventDataStore`, and reads it back through `GetQueryResults`.
//! The other cases need runs no call produces — a run in each status the model names,
//! and runs carrying result rows, which this emulator ingests no events to produce — so
//! they seed the SAME registry the handler reads, through the shared engine, and then go
//! over the wire for every assertion.

use serde_json::{json, Value};
use std::time::Duration;

use service_com_amazonaws_cloudtrail_cloudtrail_20131101::ops::types::QUERY_STATUS_VALUES;
use service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::{
    event_data_store_arn, QueryRecord, QueryState, QueryStatisticsRecord,
    GET_QUERY_RESULTS_MAX_PAGE_SIZE, QUERY_SETTLE_AFTER,
};

use super::emulator::{Emulator, HOME_REGION};

/// The id of the event data store the seeded queries ran on — the resource suffix of
/// its ARN.
const STORE_ID: &str = "bbbbbbbb-1111-2222-3333-444444444444";

/// The HTTP status this service answers a not-found refusal with: the model annotates
/// `QueryIdNotFoundException` `@httpError(404)`, and the recorded CloudTrail response
/// is 400.
const NOT_FOUND_STATUS: u16 = 400;

/// The bytes one seeded run scans per row, so a reply's `BytesScanned` is checkable.
const BYTES_PER_ROW: i64 = 4_096;

/// A `UUID` is exactly 36 characters of `[a-f0-9-]`, so a query id is built to that
/// shape: the boundary validator refuses anything else before the handler runs.
fn query_id(nth: u8) -> String {
    format!("{nth:02x}bbbbbb-1111-2222-3333-444444444444")
}

/// `rows` rows of `columns` columns each, with every cell naming its position so a
/// paging walk can prove nothing was dropped, repeated or reordered.
fn rows_of(rows: usize, columns: usize) -> Vec<Vec<(String, String)>> {
    (0..rows)
        .map(|row| {
            (0..columns)
                .map(|column| (format!("column{column}"), format!("r{row}c{column}")))
                .collect()
        })
        .collect()
}

/// One FINISHED query of `rows` rows on [`STORE_ID`] in [`HOME_REGION`].
fn finished_query(nth: u8, rows: usize, columns: usize) -> QueryRecord {
    QueryRecord {
        query_id: query_id(nth),
        event_data_store_id: STORE_ID.to_string(),
        region: HOME_REGION.to_string(),
        query_string: "SELECT eventName FROM store".to_string(),
        status: QueryState::Finished,
        statistics: QueryStatisticsRecord {
            events_matched: rows as i64,
            events_scanned: rows as i64,
            bytes_scanned: BYTES_PER_ROW * rows as i64,
            execution_time_in_millis: Some(12),
        },
        result_rows: rows_of(rows, columns),
        ..QueryRecord::default()
    }
}

/// Seed one query run, the way `StartQuery` will once it is implemented.
fn seed(emulator: &Emulator, record: &QueryRecord) {
    emulator
        .state()
        .lock()
        .queries
        .insert(record.query_id.clone(), record.clone());
}

/// The `QueryResultRows` of a reply, flattened to `(column, value)` pairs per row so a
/// row's column order is asserted alongside its values.
fn result_rows(body: &Value, case: &str) -> Vec<Vec<(String, String)>> {
    body.get("QueryResultRows")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case}: no QueryResultRows in {body:?}"))
        .iter()
        .map(|row| {
            row.as_array()
                .unwrap_or_else(|| panic!("case {case}: a row is not a list in {body:?}"))
                .iter()
                .flat_map(|column| {
                    column
                        .as_object()
                        .unwrap_or_else(|| panic!("case {case}: a column is not a map"))
                        .iter()
                        .map(|(name, value)| {
                            (
                                name.clone(),
                                value.as_str().unwrap_or_default().to_string(),
                            )
                        })
                        .collect::<Vec<_>>()
                })
                .collect()
        })
        .collect()
}

/// A run started through `StartQuery` is reported by `GetQueryResults`: the round trip
/// over the wire, on a store minted through `CreateEventDataStore`.
///
/// A new run is `QUEUED` and the clock owes it its terminal status after
/// [`QUERY_SETTLE_AFTER`], so the read before that instant reports the transient status
/// and the read after it the one the run had already reached. This emulator ingests no
/// CloudTrail events, so the run matched and scanned nothing: no row, and statistics of
/// zero.
#[tokio::test]
async fn a_run_started_through_start_query_is_reported_back() {
    let emulator = Emulator::new();

    let created = emulator
        .call("CreateEventDataStore", &json!({"Name": "lake-store-for-results"}))
        .await;
    let arn = created
        .expect_ok("CreateEventDataStore")
        .get("EventDataStoreArn")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    // An event data store id is the resource suffix of its ARN, which is how every
    // operation of this service reads one.
    let store_id = arn
        .rsplit('/')
        .next()
        .expect("an event data store ARN carries its id")
        .to_string();

    let started = emulator
        .call(
            "StartQuery",
            &json!({"QueryStatement": format!("SELECT eventName FROM {store_id}")}),
        )
        .await;
    let query_id = started
        .expect_ok("StartQuery")
        .get("QueryId")
        .and_then(Value::as_str)
        .expect("StartQuery mints a QueryId")
        .to_string();

    // Each row is a read of the same run: the status it is in at that point, and the
    // rows and statistics of a run over a store holding no event.
    for (case, expected_status) in [("queued", "QUEUED"), ("settled", "FINISHED")] {
        if case == "settled" {
            // The clock owed the transition at the run's settle instant; nothing sleeps
            // in the handler, so the test waits past it instead.
            tokio::time::sleep(QUERY_SETTLE_AFTER + Duration::from_millis(250)).await;
        }
        let reply = emulator
            .call("GetQueryResults", &json!({"QueryId": query_id}))
            .await;
        let body = reply.expect_ok(case).clone();
        assert_eq!(
            body["QueryStatus"],
            json!(expected_status),
            "case {case}: expected QueryStatus {expected_status}, got {body:?}"
        );
        assert_eq!(
            result_rows(&body, case),
            Vec::<Vec<(String, String)>>::new(),
            "case {case}: a run over a store holding no event produced no row, got {body:?}"
        );
        for (member, expected) in [
            ("ResultsCount", 0),
            ("TotalResultsCount", 0),
            ("BytesScanned", 0),
        ] {
            assert_eq!(
                body["QueryStatistics"][member],
                json!(expected),
                "case {case}: expected QueryStatistics.{member} {expected}, got {body:?}"
            );
        }
        assert_eq!(
            body.get("NextToken").cloned().unwrap_or(Value::Null),
            Value::Null,
            "case {case}: a single page of no rows continues nowhere, got {body:?}"
        );
        assert_eq!(
            body.get("ErrorMessage").cloned().unwrap_or(Value::Null),
            Value::Null,
            "case {case}: a run that did not fail carries no ErrorMessage, got {body:?}"
        );
    }
}

/// `GetQueryResults` reports the status the registry holds, for every value the model's
/// `QueryStatus` enum names — and a failed run's `ErrorMessage` only where the record
/// carries one.
#[tokio::test]
async fn every_modelled_query_status_is_reported_from_state() {
    let emulator = Emulator::new();

    // The catalog is the model's own enum, as the generated
    // `ops::types::QUERY_STATUS_VALUES` lists it.
    for (nth, wire) in QUERY_STATUS_VALUES.iter().enumerate() {
        let case = format!("status {wire}");
        let status = [
            QueryState::Queued,
            QueryState::Running,
            QueryState::Finished,
            QueryState::Failed,
            QueryState::Cancelled,
            QueryState::TimedOut,
        ]
        .into_iter()
        .find(|state| state.as_wire() == *wire)
        .unwrap_or_else(|| panic!("case {case}: the model names a status the state has no variant for"));

        let mut record = finished_query(nth as u8 + 1, 2, 2);
        record.status = status;
        if status == QueryState::Failed {
            record.error_message = Some("the query failed to parse".to_string());
        }
        let query_id = record.query_id.clone();
        seed(&emulator, &record);

        let reply = emulator
            .call("GetQueryResults", &json!({"QueryId": query_id}))
            .await;
        let body = reply.expect_ok(&case).clone();
        assert_eq!(
            body["QueryStatus"],
            json!(wire),
            "case {case}: expected QueryStatus {wire}, got {body:?}"
        );
        let expected_error = if status == QueryState::Failed {
            json!("the query failed to parse")
        } else {
            Value::Null
        };
        assert_eq!(
            body.get("ErrorMessage").cloned().unwrap_or(Value::Null),
            expected_error,
            "case {case}: expected ErrorMessage {expected_error:?}, got {body:?}"
        );
    }
}

/// A table spanning row counts, column counts and page sizes: paging through
/// `MaxQueryResults`-sized pages with `NextToken` yields every row exactly once, in
/// order, and each page's statistics describe that page and the whole result.
#[tokio::test]
async fn result_rows_page_through_next_token_in_order() {
    let emulator = Emulator::new();

    // rows, columns, MaxQueryResults (None = the service's own page size)
    let cases: [(usize, usize, Option<usize>); 7] = [
        (0, 1, None),
        (1, 1, None),
        (3, 2, Some(1)),
        (7, 3, Some(2)),
        (7, 1, Some(7)),
        (10, 2, Some(1000)),
        (250, 4, Some(100)),
    ];

    for (nth, (rows, columns, max)) in cases.into_iter().enumerate() {
        let case = format!("{rows} rows x {columns} columns, MaxQueryResults {max:?}");
        let record = finished_query(nth as u8 + 10, rows, columns);
        let query_id = record.query_id.clone();
        seed(&emulator, &record);

        let mut collected: Vec<Vec<(String, String)>> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            let mut request = json!({"QueryId": query_id});
            if let Some(max) = max {
                request["MaxQueryResults"] = json!(max);
            }
            if let Some(token) = &next_token {
                request["NextToken"] = json!(token);
            }
            let reply = emulator.call("GetQueryResults", &request).await;
            let body = reply.expect_ok(&case).clone();
            pages += 1;

            let page = result_rows(&body, &case);
            let expected_page_size = max
                .unwrap_or(GET_QUERY_RESULTS_MAX_PAGE_SIZE)
                .min(rows - collected.len());
            assert_eq!(
                page.len(),
                expected_page_size,
                "case {case}: page {pages} should carry {expected_page_size} rows, got {body:?}"
            );
            assert_eq!(
                body["QueryStatistics"]["ResultsCount"],
                json!(page.len()),
                "case {case}: page {pages} ResultsCount should count this page, got {body:?}"
            );
            assert_eq!(
                body["QueryStatistics"]["TotalResultsCount"],
                json!(rows),
                "case {case}: page {pages} TotalResultsCount should be {rows}, got {body:?}"
            );
            assert_eq!(
                body["QueryStatistics"]["BytesScanned"],
                json!(BYTES_PER_ROW * rows as i64),
                "case {case}: page {pages} BytesScanned should be the run's, got {body:?}"
            );
            collected.extend(page);

            next_token = body
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if next_token.is_none() {
                break;
            }
            assert!(
                pages <= rows + 1,
                "case {case}: paging did not terminate after {pages} pages"
            );
        }

        assert_eq!(
            collected,
            rows_of(rows, columns),
            "case {case}: the pages together must be every row, in order"
        );
        let page_size = max.unwrap_or(GET_QUERY_RESULTS_MAX_PAGE_SIZE);
        let expected_pages = rows.div_ceil(page_size).max(1);
        assert_eq!(
            pages, expected_pages,
            "case {case}: expected {expected_pages} page(s), walked {pages}"
        );
    }
}

/// A `NextToken` this query did not hand out is refused, whatever form it takes.
#[tokio::test]
async fn a_next_token_this_query_did_not_hand_out_is_refused() {
    let emulator = Emulator::new();
    let record = finished_query(20, 4, 2);
    let query_id = record.query_id.clone();
    let other = finished_query(21, 4, 2);
    let other_id = other.query_id.clone();
    seed(&emulator, &record);
    seed(&emulator, &other);

    // The token form is this operation's own, so each row is a way of not being one of
    // ITS tokens: another query's token, a non-numeric offset, no offset at all, and an
    // offset at or past the last row — a page is only continued while rows remain, so
    // no such token is ever minted.
    let cases = [
        ("another query's token", format!("{other_id}:2")),
        ("no offset separator", format!("{query_id}2")),
        ("a non-numeric offset", format!("{query_id}:two")),
        ("a negative offset", format!("{query_id}:-1")),
        ("an offset at the row count", format!("{query_id}:4")),
        ("an offset past the last row", format!("{query_id}:5")),
    ];
    for (case, token) in cases {
        emulator
            .call(
                "GetQueryResults",
                &json!({"QueryId": query_id, "NextToken": token}),
            )
            .await
            .expect_error(case, 400, "InvalidNextTokenException");
    }

    // ... and a token the query DID hand out still works.
    let first = emulator
        .call(
            "GetQueryResults",
            &json!({"QueryId": query_id, "MaxQueryResults": 2}),
        )
        .await;
    let token = first.expect_ok("first page")["NextToken"]
        .as_str()
        .expect("the first of two pages carries a NextToken")
        .to_string();
    let second = emulator
        .call(
            "GetQueryResults",
            &json!({"QueryId": query_id, "NextToken": token}),
        )
        .await;
    assert_eq!(
        result_rows(second.expect_ok("second page"), "second page"),
        rows_of(4, 2)[2..].to_vec(),
        "case second page: the token must resume where the first page stopped"
    );
}

/// A `QueryId` the registry does not hold under the addressing the request used is
/// refused with `QueryIdNotFoundException`, naming the id.
#[tokio::test]
async fn a_query_not_addressed_by_this_request_is_not_found() {
    let emulator = Emulator::new();
    let unknown = query_id(99);
    let record = finished_query(30, 2, 2);
    let query_id = record.query_id.clone();
    seed(&emulator, &record);

    // Rows differ along the addressing that fails to reach the query.
    let other_store = "cccccccc-1111-2222-3333-444444444444";
    let cases = [
        (
            "no such query id",
            json!({"QueryId": unknown}),
            unknown.clone(),
        ),
        (
            "another event data store, by id",
            json!({"QueryId": query_id, "EventDataStore": other_store}),
            query_id.clone(),
        ),
        (
            "another event data store, by ARN",
            json!({
                "QueryId": query_id,
                "EventDataStore": event_data_store_arn(HOME_REGION, other_store),
            }),
            query_id.clone(),
        ),
        (
            "another owner account",
            json!({"QueryId": query_id, "EventDataStoreOwnerAccountId": "999999999999"}),
            query_id.clone(),
        ),
    ];
    for (case, request, named) in cases {
        let reply = emulator.call("GetQueryResults", &request).await;
        reply.expect_error(case, NOT_FOUND_STATUS, "QueryIdNotFoundException");
        assert_eq!(
            reply.body["Message"],
            json!(format!(
                "The QueryId value {named} does not exist or is not in the same region."
            )),
            "case {case}: the refusal must name the id the request used, got {reply:?}"
        );
    }

    // The same id from another Region is not the same query either.
    emulator
        .call_in("GetQueryResults", "us-west-2", &json!({"QueryId": query_id}))
        .await
        .expect_error("another Region", NOT_FOUND_STATUS, "QueryIdNotFoundException");

    // ... while the store the query DID run on reaches it, in either form, as does the
    // account that owns it.
    let reachable = [
        ("by store id", json!({"QueryId": query_id, "EventDataStore": STORE_ID})),
        (
            "by store ARN",
            json!({
                "QueryId": query_id,
                "EventDataStore": event_data_store_arn(HOME_REGION, STORE_ID),
            }),
        ),
        (
            "by the owning account",
            json!({
                "QueryId": query_id,
                "EventDataStoreOwnerAccountId": engine::emulator_account_id(),
            }),
        ),
    ];
    for (case, request) in reachable {
        let reply = emulator.call("GetQueryResults", &request).await;
        assert_eq!(
            reply.expect_ok(case)["QueryStatus"],
            json!("FINISHED"),
            "case {case}: this addressing must reach the query, got {reply:?}"
        );
    }
}

/// `EventDataStore` is an event data store ARN or the ID suffix of one; a value that
/// can be read as neither is refused as an invalid ARN.
#[tokio::test]
async fn an_event_data_store_identifier_that_cannot_be_read_is_refused() {
    let emulator = Emulator::new();
    let record = finished_query(40, 1, 1);
    let query_id = record.query_id.clone();
    seed(&emulator, &record);

    let cases = [
        ("another service's ARN", "arn:aws:s3:::my-bucket".to_string()),
        (
            "this service, another resource type",
            format!("arn:aws:cloudtrail:{HOME_REGION}:000000000000:trail/demo-trail"),
        ),
        (
            "an ARN with no store id",
            format!("arn:aws:cloudtrail:{HOME_REGION}:000000000000:eventdatastore/"),
        ),
        (
            "too few ARN fields",
            "arn:aws:cloudtrail:000000000000:eventdatastore/abc".to_string(),
        ),
        (
            "an empty Region",
            "arn:aws:cloudtrail::000000000000:eventdatastore/abc".to_string(),
        ),
        ("the resource prefix alone", "eventdatastore/".to_string()),
    ];
    for (case, value) in cases {
        let reply = emulator
            .call(
                "GetQueryResults",
                &json!({"QueryId": query_id, "EventDataStore": value}),
            )
            .await;
        reply.expect_error(case, 400, "EventDataStoreARNInvalidException");
        assert!(
            reply.body["Message"]
                .as_str()
                .unwrap_or_default()
                .contains(&value),
            "case {case}: the refusal must name the value given, got {reply:?}"
        );
    }
}

/// Reading reports state and changes nothing: two reads of one query, and a read
/// between two pages, answer the same.
#[tokio::test]
async fn reading_a_query_changes_nothing() {
    let emulator = Emulator::new();
    let record = finished_query(50, 3, 2);
    let query_id = record.query_id.clone();
    seed(&emulator, &record);

    let first = emulator
        .call("GetQueryResults", &json!({"QueryId": query_id}))
        .await;
    let second = emulator
        .call("GetQueryResults", &json!({"QueryId": query_id}))
        .await;
    assert_eq!(
        first.expect_ok("repeated read"),
        second.expect_ok("repeated read"),
        "case repeated read: two reads of one query must answer the same"
    );

    // A paged read does not consume rows either: the first page is the same page
    // however often it is asked for.
    let paged = json!({"QueryId": query_id, "MaxQueryResults": 2});
    let page_one = emulator.call("GetQueryResults", &paged).await;
    let page_one_again = emulator.call("GetQueryResults", &paged).await;
    assert_eq!(
        page_one.expect_ok("repeated first page"),
        page_one_again.expect_ok("repeated first page"),
        "case repeated first page: paging must not consume rows"
    );
}
