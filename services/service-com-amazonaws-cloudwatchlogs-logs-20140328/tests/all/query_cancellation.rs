// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `StopQuery`: stopping a CloudWatch Logs Insights query and reading
//! the stopped query back through `DescribeQueries`.
//!
//! The rules under test:
//!
//! * a stop succeeds exactly when the query it names is still IN PROGRESS at that instant —
//!   whichever non-terminal status it is in — and then reports `success` true, leaves the
//!   query `Cancelled`, and cancels the completion the clock still owed it, so the query
//!   never reports a terminal status it did not reach;
//! * a stop naming a query that has already ended, in any terminal status, is refused
//!   `InvalidParameterException` and leaves that query's outcome exactly as it was;
//! * a stop naming an id this account holds no query under is refused
//!   `ResourceNotFoundException`, for every id the member admits.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response a handler does not actually compute cannot pass.
//!
//! The tables span what the rules generalise over rather than one request: how far into the
//! query's own lifecycle the stop arrives (so each status it passes through is a row), which
//! terminal status the already-ended query reached (completed on the clock, or cancelled by an
//! earlier stop of this same operation), and the shape and length of an id that names no query
//! (both published bounds of `StopQuery$queryId` and a well-formed id this store never
//! minted).

use std::time::Duration;

use serde_json::{json, Value};

use crate::harness::Logs;

/// The operations under test.
const START: &str = "StartQuery";
const STOP: &str = "StopQuery";
const DESCRIBE: &str = "DescribeQueries";

/// The modelled error codes the rules name.
const INVALID_PARAMETER: &str = "InvalidParameterException";
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The statuses a query passes through, as the model's `QueryStatus` enum spells them.
const SCHEDULED: &str = "Scheduled";
const RUNNING: &str = "Running";
const COMPLETE: &str = "Complete";
const CANCELLED: &str = "Cancelled";

/// A query string that compiles, used by every case whose subject is not the query string.
const VALID_QUERY: &str = "fields @timestamp, @message | filter @message like /ERROR/";

/// How long a case waits before treating a started query as finished: past both spans a start
/// arms (`QUERY_RUNNING_DELAY_MS` + `QUERY_COMPLETION_DELAY_MS`), with room for the test's own
/// scheduling.
fn past_completion() -> Duration {
    Duration::from_millis(900)
}

/// A log group plus the window a query of it is started with, in the unit `StartQuery`
/// documents for its range ("epoch time, the number of seconds since January 1, 1970").
async fn group_with_window(logs: &Logs, group: &str) -> (i64, i64) {
    logs.ok("CreateLogGroup", &json!({"logGroupName": group}))
        .await;
    let now_secs = crate::harness::now_ms() / 1_000 + 1;
    (now_secs - 60 * 60, now_secs)
}

/// Start one query of `group` and return the `queryId` it answers.
async fn start(logs: &Logs, group: &str, window: (i64, i64), query_string: &str) -> String {
    let answer = logs
        .ok(
            START,
            &json!({
                "logGroupName": group,
                "startTime": window.0,
                "endTime": window.1,
                "queryString": query_string,
            }),
        )
        .await;
    answer["queryId"]
        .as_str()
        .unwrap_or_else(|| panic!("StartQuery answers a queryId, got {answer}"))
        .to_string()
}

/// The status `DescribeQueries` reports for the query carrying `id`.
async fn reported_status(logs: &Logs, id: &str) -> String {
    let page = logs.ok(DESCRIBE, &json!({})).await;
    let queries = page["queries"]
        .as_array()
        .unwrap_or_else(|| panic!("DescribeQueries reports a queries list, got {page}"))
        .clone();
    let query = queries
        .iter()
        .find(|query| query["queryId"].as_str() == Some(id))
        .unwrap_or_else(|| panic!("the started query {id} must be reported, got {page}"));
    query["status"]
        .as_str()
        .unwrap_or_else(|| panic!("a reported query carries a status, got {query}"))
        .to_string()
}

/// A stop succeeds exactly when the query is still in progress, and the query it stopped is
/// `Cancelled` from then on.
#[tokio::test]
async fn a_stop_succeeds_while_the_query_is_in_progress_and_leaves_it_cancelled() {
    let logs = Logs::new();
    const GROUP: &str = "/stop-query/lifecycle";
    let window = group_with_window(&logs, GROUP).await;

    // (case, how long after the start the stop arrives) — one row per point of the query's
    // own lifecycle, so the stop meets it in each status a start leads it through.
    let cases: [(&str, Duration); 3] = [
        ("straight after the start", Duration::from_millis(0)),
        ("once the query is running", Duration::from_millis(350)),
        ("past the spans the start armed", past_completion()),
    ];
    for (case, delay) in cases {
        let id = start(&logs, GROUP, window, VALID_QUERY).await;
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        // The status the query is in when the stop arrives, read over the wire like any
        // client's: it is what decides whether there is an execution left to stop.
        let before = reported_status(&logs, &id).await;
        let (status, body) = logs.call(STOP, &json!({"queryId": id})).await;
        if before == COMPLETE {
            assert_eq!(
                status, 400,
                "case {case:?}: a query that reached {COMPLETE} has ended, so the stop is \
                 refused, got {status} with body {body}"
            );
            let code = crate::harness::error_code(&body);
            assert_eq!(
                code, INVALID_PARAMETER,
                "case {case:?}: expected {INVALID_PARAMETER}, got {code} with body {body}"
            );
            continue;
        }
        assert!(
            before == SCHEDULED || before == RUNNING,
            "case {case:?}: a query that has not ended is {SCHEDULED} or {RUNNING}, got \
             {before:?}"
        );
        assert_eq!(
            status, 200,
            "case {case:?}: a query in progress ({before}) is stopped, got {status} with body \
             {body}"
        );
        assert_eq!(
            body["success"].as_bool(),
            Some(true),
            "case {case:?}: the stop that stopped the query reports success, got {body}"
        );
        assert_eq!(
            reported_status(&logs, &id).await,
            CANCELLED,
            "case {case:?}: the stopped query is reported {CANCELLED}"
        );
        // The completion the clock still owed the query is cancelled with it: waiting past
        // the instant it was due must not report a run that was stopped.
        tokio::time::sleep(past_completion()).await;
        assert_eq!(
            reported_status(&logs, &id).await,
            CANCELLED,
            "case {case:?}: a stopped query stays {CANCELLED} past the completion it was \
             armed for"
        );
        // … and it is reported under that status as a filter, which is the store's own copy
        // of it rather than the answer to one read.
        let page = logs.ok(DESCRIBE, &json!({"status": CANCELLED})).await;
        let reported: Vec<String> = page["queries"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter_map(|query| query["queryId"].as_str().map(ToString::to_string))
            .collect();
        assert!(
            reported.contains(&id),
            "case {case:?}: the {CANCELLED} filter reports the stopped query, got {page}"
        );
    }
}

/// A stop of a query that has already ended is refused, and the ended query keeps the outcome
/// it reached.
#[tokio::test]
async fn a_stop_of_a_query_that_has_already_ended_is_refused() {
    let logs = Logs::new();
    const GROUP: &str = "/stop-query/ended";
    let window = group_with_window(&logs, GROUP).await;

    // A query the clock completed, and a query an earlier stop cancelled: the two terminal
    // statuses this service's queries reach.
    let completed = start(&logs, GROUP, window, VALID_QUERY).await;
    let cancelled = start(&logs, GROUP, window, "fields @message").await;
    let success = logs.ok(STOP, &json!({"queryId": cancelled})).await;
    assert_eq!(
        success["success"].as_bool(),
        Some(true),
        "case the first stop: stopping a query in progress succeeds, got {success}"
    );
    tokio::time::sleep(past_completion()).await;

    // (case, the query, the terminal status it is in)
    let cases: [(&str, &String, &str); 2] = [
        ("a query the clock completed", &completed, COMPLETE),
        ("a query an earlier stop cancelled", &cancelled, CANCELLED),
    ];
    for (case, id, terminal) in cases {
        assert_eq!(
            reported_status(&logs, id).await,
            terminal,
            "case {case:?}: the query under test must be {terminal}"
        );
        let (code, message) = logs.refusal(STOP, &json!({"queryId": id})).await;
        assert_eq!(
            code, INVALID_PARAMETER,
            "case {case:?}: expected {INVALID_PARAMETER}, got {code} with message {message:?}"
        );
        assert!(
            message.contains(id.as_str()),
            "case {case:?}: the refusal names the query the caller asked to stop, got \
             {message:?}"
        );
        // The refusal changes nothing: the ended query keeps the outcome it reached.
        assert_eq!(
            reported_status(&logs, id).await,
            terminal,
            "case {case:?}: a refused stop leaves the query {terminal}"
        );
    }
}

/// A stop naming an id this account holds no query under is refused as the missing resource it
/// is, for every id the member admits.
#[tokio::test]
async fn a_stop_naming_no_stored_query_is_refused_as_a_missing_resource() {
    let logs = Logs::new();
    const GROUP: &str = "/stop-query/unknown";
    let window = group_with_window(&logs, GROUP).await;
    // One query of this account, so an id that names no query is distinguished from a store
    // holding nothing at all.
    let started = start(&logs, GROUP, window, VALID_QUERY).await;

    // (case, the id) — `StopQuery$queryId` is modelled `len 1..256`, so both published bounds
    // reach the handler, as does a well-formed id this store never minted.
    let cases: [(&str, String); 5] = [
        (
            "an id at the published minimum length",
            "q".to_string(),
        ),
        (
            "an id at the published maximum length",
            "q".repeat(256),
        ),
        (
            "a canonical UUID this store never minted",
            "ffffffff-ffff-4fff-bfff-ffffffffffff".to_string(),
        ),
        (
            "the started query's id with one character appended",
            format!("{started}0"),
        ),
        (
            "a prefix of the started query's id",
            started[..started.len() - 1].to_string(),
        ),
    ];
    for (case, id) in cases {
        let code = logs.err(STOP, &json!({"queryId": id})).await;
        assert_eq!(
            code, NOT_FOUND,
            "case {case:?}: expected {NOT_FOUND} for {id:?}, got {code}"
        );
    }

    // The query this account does hold is untouched by every refused stop, and is still
    // stoppable.
    let answer = logs.ok(STOP, &json!({"queryId": &started})).await;
    assert_eq!(
        answer["success"].as_bool(),
        Some(true),
        "case the stored query: it is still the query the caller can stop, got {answer}"
    );
    assert_eq!(
        reported_status(&logs, &started).await,
        CANCELLED,
        "case the stored query: the stop that succeeded left it {CANCELLED}"
    );
}

/// Stopping one query stops ONLY that query: another query started beside it keeps the
/// lifecycle its own start armed.
#[tokio::test]
async fn a_stop_touches_only_the_query_it_names() {
    let logs = Logs::new();
    const GROUP: &str = "/stop-query/neighbours";
    let window = group_with_window(&logs, GROUP).await;

    let stopped = start(&logs, GROUP, window, VALID_QUERY).await;
    let untouched = start(&logs, GROUP, window, "fields @message | limit 1").await;
    logs.ok(STOP, &json!({"queryId": &stopped})).await;

    tokio::time::sleep(past_completion()).await;
    assert_eq!(
        reported_status(&logs, &stopped).await,
        CANCELLED,
        "case the stopped query: it stays {CANCELLED}"
    );
    assert_eq!(
        reported_status(&logs, &untouched).await,
        COMPLETE,
        "case the query beside it: it reaches {COMPLETE} on its own clock"
    );
    // Reporting is a read: the statuses above are the store's, so reading twice reports the
    // same thing.
    let first: Value = logs.ok(DESCRIBE, &json!({})).await;
    let second: Value = logs.ok(DESCRIBE, &json!({})).await;
    assert_eq!(
        first, second,
        "case twice: reporting the queries changes nothing a later caller sees"
    );
}
