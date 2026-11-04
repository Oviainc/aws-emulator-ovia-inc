// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the bound ONE engine run is held to.
//!
//! A query execution runs its SQL on this service's in-process engine under a wall-clock
//! deadline and a step budget that are armed when THAT run starts and released when it
//! returns. The engine consults the bound from its progress handler every few thousand
//! instructions, so a statement that does real work consults it hundreds of times over
//! one run, and the budget is spent a callback at a time.
//!
//! Two rules are asserted here, each over a SET of inputs rather than one request:
//!
//! * work that fits inside the bound SUCCEEDS and reports the rows the engine computed,
//!   however many times the bound was consulted while it ran — the rows differ along how
//!   much engine work the statement does and how much of its result is retained;
//! * the bound belongs to ONE run: what a run spends is not charged to the next, so a run
//!   stopped at the bound is followed by a run that gets a whole bound of its own.
//!
//! Driven over the wire (awsJson1_1, `x-amz-target: AmazonAthena.<Operation>`) through
//! the same router the emulator boots, so every assertion is about what an SDK client
//! observes.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_athena_amazonathena::state::{
    query_engine, QUERY_STATE_FAILED, QUERY_STATE_QUEUED, QUERY_STATE_RUNNING,
    QUERY_STATE_SUCCEEDED,
};
use service_com_amazonaws_athena_amazonathena::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The `x-amz-target` prefix of this service: the local name of the Smithy service shape
/// (`com.amazonaws.athena#AmazonAthena`), which is how the awsJson protocols build that
/// header.
const TARGET_PREFIX: &str = "AmazonAthena";

/// The router picks a service by the SigV4 credential scope's service field, so every
/// request carries one naming `athena` (this service's `sigv4Name`). Neither the key id
/// nor the signature is a credential: the harness turns request authentication off below,
/// so nothing here is verified.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/athena/aws4_request, \
     SignedHeaders=host;x-amz-target, Signature=not-verified";

/// Where the queries in these tests write their results. Any location satisfies the
/// documented rule that one must be given; nothing is written to it.
const OUTPUT_LOCATION: &str = "s3://athena-results/engine-run-bound/";

/// The page size every read below asks for: the modelled maximum of
/// `GetQueryResults$MaxResults` (range 1..1000), so a result set of thousands of rows is
/// read in as few pages as the operation allows.
const PAGE_ROWS: usize = 1_000;

/// How many pages one result set may take before the paging loop gives up, so a reader
/// that never ends the pagination fails the test instead of spinning.
const MAX_PAGES: usize = 64;

/// How long a poll waits between two reads of a started query's state, and how many times
/// it reads: the product is well past the span a completion is owed after, so a busy host
/// does not fail the test.
const POLL_INTERVAL: Duration = Duration::from_millis(100);
/// How many times [`settled`] reads before giving up; see [`POLL_INTERVAL`].
const POLL_ATTEMPTS: usize = 80;

/// How many rows the aggregate below counts: far more engine instructions than one
/// progress-handler callback spends, so one run of it consults the bound many times while
/// retaining a single row of result.
const AGGREGATED_ROWS: u64 = 200_000;

/// How many rows the sort below orders, for the same reason as [`AGGREGATED_ROWS`] — with
/// the work in a sort rather than in an aggregate.
const SORTED_ROWS: u64 = 50_000;

/// How many of the sorted rows are reported.
const SORTED_REPORTED: u64 = 3;

/// Work with no end to it: a run of this can only finish by being stopped at the bound
/// the emulator holds it to.
const ENDLESS_SQL: &str = "WITH RECURSIVE forever(n) AS (SELECT 1 UNION ALL \
     SELECT n + 1 FROM forever) SELECT count(*) FROM forever";

/// One service mounted on its own engine, reached the way the production router reaches
/// it.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Pinned off so the suite does not depend on how the emulator process was started;
    // these tests assert Athena's behaviour, not the emulator's SigV4 check.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Invoke one operation and return `(status, body)`.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "athena.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header("content-type", "application/x-amz-json-1.1")
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// A `StartQueryExecution` body for `sql` with a results location, running in the
/// database `context` names — or in the default database when it names none.
fn query(sql: &str, context: Option<&Value>) -> Value {
    let mut body = json!({
        "QueryString": sql,
        "ResultConfiguration": { "OutputLocation": OUTPUT_LOCATION },
    });
    if let Some(context) = context {
        body["QueryExecutionContext"] = context.clone();
    }
    body
}

/// Start one query and return the `QueryExecutionId` it minted. The engine run happens
/// inside this call, so it returns once the statement is over — whether it ran to
/// completion or was stopped at its bound.
async fn start(router: &MultiServiceRouter, body: &Value) -> String {
    let (status, answer) = call(router, "StartQueryExecution", body).await;
    assert_eq!(
        status, 200,
        "StartQueryExecution({body}) must be accepted, got {status} with {answer:?}"
    );
    answer["QueryExecutionId"]
        .as_str()
        .unwrap_or_else(|| panic!("StartQueryExecution must answer with an id, got {answer:?}"))
        .to_string()
}

/// The stored `QueryExecution` of `id`.
async fn execution(router: &MultiServiceRouter, id: &str) -> Value {
    let (status, body) =
        call(router, "GetQueryExecution", &json!({ "QueryExecutionId": id })).await;
    assert_eq!(
        status, 200,
        "GetQueryExecution({id}) must answer 200, got {status} with {body:?}"
    );
    body["QueryExecution"].clone()
}

/// The execution of `id` once it is terminal.
///
/// A completion is owed a span after the submit, so this POLLS the stored record until it
/// is out of the states a query is reported in before then.
async fn settled(router: &MultiServiceRouter, id: &str) -> Value {
    for _ in 0..POLL_ATTEMPTS {
        let found = execution(router, id).await;
        let state = found["Status"]["State"].as_str().unwrap_or_default();
        if state != QUERY_STATE_QUEUED && state != QUERY_STATE_RUNNING {
            return found;
        }
        ::tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!("the execution {id} never reached a terminal state");
}

/// Every row `GetQueryResults` reports for `id`, a page at a time, following the
/// `NextToken` the previous page handed back: the column-label row a DML result set
/// starts with, and then the data rows.
async fn reported_rows(router: &MultiServiceRouter, id: &str) -> Vec<Vec<Option<String>>> {
    let mut rows: Vec<Vec<Option<String>>> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0;
    loop {
        let mut request = json!({ "QueryExecutionId": id, "MaxResults": PAGE_ROWS });
        if let Some(token) = &token {
            request["NextToken"] = json!(token);
        }
        let (status, body) = call(router, "GetQueryResults", &request).await;
        assert_eq!(
            status, 200,
            "GetQueryResults({request}) must answer 200, got {status} with {body:?}"
        );
        pages += 1;
        let empty = Vec::new();
        for row in body["ResultSet"]["Rows"].as_array().unwrap_or(&empty) {
            rows.push(
                row["Data"]
                    .as_array()
                    .unwrap_or(&empty)
                    .iter()
                    .map(|datum| datum["VarCharValue"].as_str().map(str::to_string))
                    .collect(),
            );
        }
        token = body["NextToken"].as_str().map(str::to_string);
        if token.is_none() {
            break;
        }
        assert!(
            pages < MAX_PAGES,
            "the results of {id} did not end after {pages} pages"
        );
    }
    rows
}

/// `rows` rows, each computed from the one before: the cheapest statement that makes the
/// engine loop, so one run of it consults the bound once every few thousand instructions
/// it spends.
fn counting_sql(rows: u64) -> String {
    format!(
        "WITH RECURSIVE counted(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM counted \
         WHERE n < {rows}) SELECT n FROM counted"
    )
}

/// One row of the inside-the-bound table.
struct RunCase {
    /// What this row is about.
    case: &'static str,
    /// The statement submitted.
    sql: String,
    /// How many DATA rows it must produce, without the column-label row.
    rows: usize,
    /// The first data row's values, as `GetQueryResults` renders them.
    first: Vec<String>,
    /// The last data row's values.
    last: Vec<String>,
}

/// The rows: each statement does more engine work than the one before, or retains a
/// different amount of what it computed, so the bound is consulted a different number of
/// times per run and a different share of the result is held.
fn run_cases() -> Vec<RunCase> {
    // The most rows one result set retains is the engine's own published bound
    // (`query_engine::MAX_RESULT_ROWS`), so the row AT that bound is read from it rather
    // than written out here; one row PAST it is a failure, which the lifecycle suite
    // asserts.
    let retained = u64::try_from(query_engine::MAX_RESULT_ROWS).unwrap_or(1);
    // The sum of 1..=AGGREGATED_ROWS, so the expected total is arithmetic rather than a
    // number copied from a run.
    let aggregated_total = AGGREGATED_ROWS * (AGGREGATED_ROWS + 1) / 2;
    vec![
        RunCase {
            case: "one row, with no loop for the engine to run",
            sql: "SELECT 1 AS n".to_string(),
            rows: 1,
            first: vec!["1".to_string()],
            last: vec!["1".to_string()],
        },
        RunCase {
            case: "a thousand rows, each built from the one before",
            sql: counting_sql(1_000),
            rows: 1_000,
            first: vec!["1".to_string()],
            last: vec!["1000".to_string()],
        },
        RunCase {
            case: "as many rows as one result set retains",
            sql: counting_sql(retained),
            rows: query_engine::MAX_RESULT_ROWS,
            first: vec!["1".to_string()],
            last: vec![retained.to_string()],
        },
        RunCase {
            case: "many instructions and one row out of all of them",
            sql: format!(
                "WITH RECURSIVE counted(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM \
                 counted WHERE n < {AGGREGATED_ROWS}) SELECT count(*) AS seen, \
                 sum(n) AS total FROM counted"
            ),
            rows: 1,
            first: vec![AGGREGATED_ROWS.to_string(), aggregated_total.to_string()],
            last: vec![AGGREGATED_ROWS.to_string(), aggregated_total.to_string()],
        },
        RunCase {
            case: "a sort over many rows, of which only the top few are reported",
            sql: format!(
                "{} ORDER BY n DESC LIMIT {SORTED_REPORTED}",
                counting_sql(SORTED_ROWS)
            ),
            rows: usize::try_from(SORTED_REPORTED).unwrap_or(1),
            first: vec![SORTED_ROWS.to_string()],
            last: vec![(SORTED_ROWS - SORTED_REPORTED + 1).to_string()],
        },
    ]
}

/// The `EngineExecutionTimeInMillis` the execution reports, which is what the engine
/// really spent on the run.
fn engine_millis(found: &Value) -> i64 {
    found["Statistics"]["EngineExecutionTimeInMillis"]
        .as_i64()
        .unwrap_or(-1)
}

/// The deadline one run is held to, in whole milliseconds, with room for the request walk
/// around the engine: the engine time a terminal execution reports must be inside it.
fn deadline_millis() -> i64 {
    i64::try_from(query_engine::MAX_QUERY_DEADLINE.as_millis()).unwrap_or(i64::MAX)
        + i64::try_from(POLL_INTERVAL.as_millis()).unwrap_or(0)
}

/// Work that fits inside the bound SUCCEEDS and reports the rows the engine computed,
/// however many times the bound was consulted while the statement ran.
#[tokio::test]
async fn work_inside_the_run_bound_succeeds_however_often_the_bound_is_consulted() {
    let router = router();
    let cases = run_cases();

    // Started first, all of them, so the one span a completion is owed after is waited
    // out once rather than per row.
    let mut started: Vec<String> = Vec::new();
    for case in &cases {
        started.push(start(&router, &query(&case.sql, None)).await);
    }

    for (case, id) in cases.iter().zip(started.iter()) {
        let name = case.case;
        let found = settled(&router, id).await;
        assert_eq!(
            found["Status"]["State"].as_str(),
            Some(QUERY_STATE_SUCCEEDED),
            "case {name:?}: work inside the bound must succeed, got {found:?}"
        );
        let spent = engine_millis(&found);
        assert!(
            (0..=deadline_millis()).contains(&spent),
            "case {name:?}: the engine time must be inside the run's deadline of \
             {}ms, got {spent}ms",
            deadline_millis()
        );

        let rows = reported_rows(&router, id).await;
        // The first row of a DML result set is the column labels, and the data rows
        // follow it.
        assert_eq!(
            rows.len(),
            case.rows + 1,
            "case {name:?}: expected {} data rows and one label row, got {} rows in total",
            case.rows,
            rows.len()
        );
        let data = &rows[1..];
        let expected_first: Vec<Option<String>> =
            case.first.iter().map(|value| Some(value.clone())).collect();
        let expected_last: Vec<Option<String>> =
            case.last.iter().map(|value| Some(value.clone())).collect();
        assert_eq!(
            data.first(),
            Some(&expected_first),
            "case {name:?}: expected {expected_first:?} as the first data row, got {:?}",
            data.first()
        );
        assert_eq!(
            data.last(),
            Some(&expected_last),
            "case {name:?}: expected {expected_last:?} as the last data row, got {:?}",
            data.last()
        );
        // Nothing the engine produced is reported as a short row: every data row carries
        // one value per projected column.
        for (index, row) in data.iter().enumerate() {
            assert_eq!(
                row.len(),
                expected_first.len(),
                "case {name:?}: data row {index} must carry {} values, got {row:?}",
                expected_first.len()
            );
        }
    }
}

/// The bound belongs to ONE run: what a run spends is not charged to the next, so a run
/// stopped at the bound is followed by a run that gets a whole bound of its own and
/// answers exactly what the same statement answered before it.
#[tokio::test]
async fn the_run_bound_is_armed_per_run_so_a_stopped_run_does_not_shorten_the_next() {
    let router = router();
    // One database for all three queries, so every run goes through the SAME engine and
    // its progress handler: a bound left armed after the stopped run, or a budget carried
    // over from it, would show up on the run that follows.
    let context = json!({ "Database": "engine_run_bound", "Catalog": "AwsDataCatalog" });
    let heavy = run_cases()
        .into_iter()
        .find(|case| case.rows == 1 && case.first.len() == 2)
        .expect("the aggregate case does the most engine work of the table");

    // (case, the statement, whether the run must finish inside the bound), in the order
    // they run: a long run that fits, a run that cannot, and the long run again.
    let sequence: Vec<(&str, &str, bool)> = vec![
        ("before the stopped run", heavy.sql.as_str(), true),
        ("the run stopped at the bound", ENDLESS_SQL, false),
        ("after the stopped run", heavy.sql.as_str(), true),
    ];

    let mut started: Vec<(&str, String, bool)> = Vec::new();
    for (case, sql, succeeds) in &sequence {
        started.push((case, start(&router, &query(sql, Some(&context))).await, *succeeds));
    }

    let mut answers: Vec<(&str, Vec<Vec<Option<String>>>)> = Vec::new();
    for (case, id, succeeds) in &started {
        let found = settled(&router, id).await;
        let expected_state = if *succeeds {
            QUERY_STATE_SUCCEEDED
        } else {
            QUERY_STATE_FAILED
        };
        assert_eq!(
            found["Status"]["State"].as_str(),
            Some(expected_state),
            "case {case:?}: expected {expected_state}, got {found:?}"
        );
        let spent = engine_millis(&found);
        assert!(
            (0..=deadline_millis()).contains(&spent),
            "case {case:?}: the engine time must be inside the run's deadline of {}ms, \
             got {spent}ms",
            deadline_millis()
        );
        if *succeeds {
            answers.push((case, reported_rows(&router, id).await));
        } else {
            // A run the emulator stopped reports why, and has no result to read.
            assert!(
                found["Status"]["StateChangeReason"]
                    .as_str()
                    .is_some_and(|reason| !reason.is_empty()),
                "case {case:?}: a stopped run must report why, got {found:?}"
            );
        }
    }

    let (first_case, first_answer) = answers
        .first()
        .expect("the sequence runs the long statement before the stopped run");
    for (case, answer) in &answers {
        assert_eq!(
            answer, first_answer,
            "case {case:?}: the same statement must answer what it answered for case \
             {first_case:?}, got {answer:?}"
        );
    }
    assert_eq!(
        answers.len(),
        2,
        "both runs of the long statement must have results to compare, got {}",
        answers.len()
    );
}
