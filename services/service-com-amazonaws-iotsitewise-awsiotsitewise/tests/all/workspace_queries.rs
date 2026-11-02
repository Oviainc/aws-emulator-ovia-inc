// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the asynchronous query slice of a workspace
//! (`StartQuery` -> `DescribeQuery`), driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with, so every assertion is about the
//! wire behaviour a real client sees.
//!
//! The rules these suites pin:
//!
//! * a started query is RUN against the shared store, and what a read reports about
//!   it — its terminal status and the statistics beside it — is what that run
//!   produced: the row count is the number of rows the same statement answers
//!   through the synchronous query path, and the bytes scanned are the stored cells
//!   the run read;
//! * a query is not terminal the instant it is started and is terminal once the
//!   span the clock owes has elapsed, with its completion instant and statistics
//!   appearing only then;
//! * a statement the query engine cannot read is the modelled invalid-request
//!   refusal and starts NO query, and so is a statement carrying a clause the query
//!   language this service answers does not carry;
//! * a query is addressed only inside the workspace it was started in, and a
//!   workspace this service does not hold is the modelled not-found refusal;
//! * a retry carrying the client token of a query that was already started — the
//!   SAME request under that token — reports that one query and starts no second,
//!   while a DIFFERENT request reusing the token repeats no call and is refused the
//!   modelled conflict naming the query that token's own call started;
//! * a run that outruns the engine's budget is a FAILED query carrying the message
//!   saying why — not a refused request, because an asynchronous query reports the
//!   fate of its run through its own status.
//!
//! The HTTP harness (`call`, `router`, `error_code`, …) is the one the asset suite
//! already defines; this suite reuses it rather than growing a second.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{
    service_state, AssetRecord, QUERY_STEP,
};

use crate::asset_lifecycle::{
    call, create_asset, create_model, error_code, property_id, router, router_with_engine,
};
use crate::support::now_seconds;

/// The workspace every suite below starts its queries in.
pub(crate) const WORKSPACE: &str = "Plant1";

/// The `QueryStatus` values that are TERMINAL: a query in one of them never moves
/// again. Source: `smithy_query(query="shape", shape="QueryStatus")` — SUBMITTED |
/// RUNNING | COMPLETED | FAILED | CANCELED | CANCELING, of which the first two and
/// `CANCELING` are the ones a query is still on its way through.
const TERMINAL_STATUSES: [&str; 3] = ["COMPLETED", "FAILED", "CANCELED"];

/// What a `StartQuery` request carrying a `clientToken` is answered with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answer {
    /// The request is served: it starts a query of its own.
    Started,
    /// The request REPEATS the call its token was spent on, so it reports that
    /// call's query and starts none.
    Replayed,
    /// The request differs from the call its token was spent on, so it repeats no
    /// call and is refused the modelled conflict.
    Refused,
}

/// Long enough that the whole span the clock owes a query has elapsed.
pub(crate) fn settled() -> Duration {
    2 * QUERY_STEP + Duration::from_millis(200)
}

/// `POST /workspaces/{workspaceName}/queries`, as `OPERATIONS.md` reports the
/// route.
fn queries_uri(workspace: &str) -> String {
    format!("/workspaces/{workspace}/queries")
}

/// `GET /workspaces/{workspaceName}/queries/{queryId}`.
pub(crate) fn query_uri(workspace: &str, query_id: &str) -> String {
    format!("/workspaces/{workspace}/queries/{query_id}")
}

/// `StartQuery` with `statement` and, when given, a `clientToken`.
pub(crate) async fn start_in(
    router: &MultiServiceRouter,
    workspace: &str,
    statement: &str,
    token: Option<&str>,
) -> (u16, Value) {
    let mut body = json!({ "queryStatement": statement });
    if let Some(token) = token {
        body.as_object_mut()
            .expect("an object body")
            .insert("clientToken".to_string(), json!(token));
    }
    call(router, "POST", &queries_uri(workspace), Some(body)).await
}

/// `StartQuery` in [`WORKSPACE`], asserting it was accepted, and the `queryId` it
/// minted.
pub(crate) async fn start(router: &MultiServiceRouter, statement: &str) -> String {
    let (status, body) = start_in(router, WORKSPACE, statement, None).await;
    assert_eq!(
        status, 202,
        "start {statement:?}: expected 202, got {status} {body}"
    );
    assert_eq!(
        body["status"].as_str(),
        Some("SUBMITTED"),
        "start {statement:?}: a started query reports SUBMITTED, got {body}"
    );
    let query_id = body["queryId"]
        .as_str()
        .unwrap_or_else(|| panic!("start {statement:?}: no queryId in {body}"))
        .to_string();
    // `QueryId` is modelled len 1..64, pattern ^[a-zA-Z0-9-]+$.
    assert!(
        (1..=64).contains(&query_id.len())
            && query_id
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '-'),
        "start {statement:?}: {query_id:?} is not a modelled queryId"
    );
    query_id
}

/// `DescribeQuery` on a query of [`WORKSPACE`].
pub(crate) async fn describe(router: &MultiServiceRouter, query_id: &str) -> (u16, Value) {
    call(router, "GET", &query_uri(WORKSPACE, query_id), None).await
}

/// `DescribeQuery`, asserting it answered, and the body it answered with.
pub(crate) async fn described(router: &MultiServiceRouter, query_id: &str) -> Value {
    let (status, body) = describe(router, query_id).await;
    assert_eq!(status, 200, "describe {query_id}: got {status} {body}");
    body
}

/// How many rows the SYNCHRONOUS query path answers for `statement` — the
/// independent oracle for the row count a started query's statistics report, since
/// both run the caller's statement over the same store.
async fn rows_through_execute_query(router: &MultiServiceRouter, statement: &str) -> usize {
    let (status, body) = call(
        router,
        "POST",
        "/queries/execution",
        Some(json!({ "queryStatement": statement })),
    )
    .await;
    assert_eq!(
        status, 200,
        "ExecuteQuery {statement:?}: got {status} {body}"
    );
    assert!(
        body["nextToken"].is_null(),
        "ExecuteQuery {statement:?}: the oracle reads one page, got {body}"
    );
    body["rows"]
        .as_array()
        .unwrap_or_else(|| panic!("ExecuteQuery {statement:?}: no rows in {body}"))
        .len()
}

/// How many queries the store holds, across every workspace — the only way to
/// observe that a refused request started nothing, since no reader enumerates a
/// workspace's queries yet.
pub(crate) fn queries_held(engine: &engine::InMemoryEngine) -> usize {
    let state = service_state(engine).expect("the IoT SiteWise store");
    let store = state.lock();
    store.queries.len()
}

/// Create `name` through `CreateWorkspace`, so a query started in it names a
/// workspace this service really holds.
pub(crate) async fn create_workspace(router: &MultiServiceRouter, name: &str) {
    let (status, body) = call(
        router,
        "POST",
        "/workspaces",
        Some(json!({
            "workspaceName": name,
            "encryptionConfiguration": { "encryptionType": "SITEWISE_DEFAULT_ENCRYPTION" },
        })),
    )
    .await;
    assert_eq!(status, 200, "create workspace {name}: got {status} {body}");
}

/// Two assets of one model, the first carrying two samples on its DOUBLE property
/// and one on its STRING property, inside a created [`WORKSPACE`]. Every view the
/// query engine exposes therefore holds rows.
pub(crate) async fn fixture(router: &MultiServiceRouter) {
    create_workspace(router, WORKSPACE).await;
    let model_id = create_model(
        router,
        "Boiler",
        json!([
            {"name": "Temperature", "dataType": "DOUBLE", "type": {"measurement": {}}},
            {"name": "Status", "dataType": "STRING", "type": {"measurement": {}}},
        ]),
    )
    .await;
    let asset_id = create_asset(router, "Boiler1", &model_id).await;
    let _second = create_asset(router, "Boiler2", &model_id).await;
    let (_, asset) = call(router, "GET", &format!("/assets/{asset_id}"), None).await;
    let temperature = property_id(&asset["assetProperties"], "Temperature");
    let status = property_id(&asset["assetProperties"], "Status");
    let samples = [
        (temperature.clone(), 240, json!({ "doubleValue": 10.5 })),
        (temperature, 180, json!({ "doubleValue": 11.5 })),
        (status, 120, json!({ "stringValue": "RUNNING" })),
    ];
    for (at, (property, age, value)) in samples.into_iter().enumerate() {
        let (code, body) = call(
            router,
            "POST",
            "/properties",
            Some(json!({"entries": [{
                "entryId": format!("entry-{at}"),
                "assetId": asset_id,
                "propertyId": property,
                "propertyValues": [{
                    "value": value,
                    "timestamp": {"timeInSeconds": now_seconds() - age, "offsetInNanos": 0},
                }],
            }]})),
        )
        .await;
        assert_eq!(code, 200, "seeding a sample: got {code} {body}");
        assert_eq!(
            body["errorEntries"].as_array().map(Vec::len),
            Some(0),
            "seeding a sample: every entry must be accepted, got {body}"
        );
    }
}

/// One row of the start-and-read-back table.
#[derive(Debug)]
struct QueryCase {
    label: &'static str,
    statement: &'static str,
}

/// The rows: statements spanning what the rule generalises over — each view the
/// engine exposes, a predicate that keeps every row, one that keeps one, one that
/// keeps none, an ordering, and a `LIMIT` that cuts the answer — so the reported
/// row count can only be right if it came from the run and not from the request.
fn query_cases() -> Vec<QueryCase> {
    vec![
        QueryCase {
            label: "every asset",
            statement: "SELECT asset_name FROM asset",
        },
        QueryCase {
            label: "one asset by name",
            statement: "SELECT * FROM asset WHERE asset_name = 'Boiler1'",
        },
        QueryCase {
            label: "a predicate no asset satisfies",
            statement: "SELECT asset_id FROM asset WHERE asset_name = 'Chiller9'",
        },
        QueryCase {
            label: "every asset property",
            statement: "SELECT asset_id, property_name, data_type FROM asset_property",
        },
        QueryCase {
            label: "every stored sample",
            statement: "SELECT * FROM raw_time_series",
        },
        QueryCase {
            label: "the latest sample of each series",
            statement: "SELECT event_timestamp, double_value FROM latest_value_time_series",
        },
        QueryCase {
            label: "an ordering with a limit",
            statement: "SELECT asset_name FROM asset ORDER BY asset_name DESC LIMIT 1",
        },
        QueryCase {
            label: "a sample selected by its quality",
            statement: "SELECT quality FROM raw_time_series WHERE quality = 'GOOD'",
        },
    ]
}

#[tokio::test]
async fn a_started_query_reports_what_its_run_over_the_store_produced() {
    let router = router();
    fixture(&router).await;

    // Every row's query is started first, and each is read straight away: a query
    // the clock still owes a transition is NOT terminal, and carries neither a
    // completion instant nor statistics yet.
    let mut started: Vec<(&QueryCase, String, usize)> = Vec::new();
    let cases = query_cases();
    for case in &cases {
        let expected_rows = rows_through_execute_query(&router, case.statement).await;
        let query_id = start(&router, case.statement).await;
        let body = described(&router, &query_id).await;
        let status = body["status"].as_str().unwrap_or_default().to_string();
        assert!(
            !TERMINAL_STATUSES.contains(&status.as_str()),
            "case {:?}: a query read straight after being started is not terminal, got {body}",
            case.label
        );
        assert!(
            body["completedAt"].is_null(),
            "case {:?}: a query that has not finished reports no completedAt, got {body}",
            case.label
        );
        assert!(
            body["statistics"].is_null(),
            "case {:?}: statistics are a COMPLETED query's, got {body}",
            case.label
        );
        started.push((case, query_id, expected_rows));
    }

    tokio::time::sleep(settled()).await;

    for (case, query_id, expected_rows) in &started {
        let body = described(&router, query_id).await;
        assert_eq!(
            body["status"].as_str(),
            Some("COMPLETED"),
            "case {:?}: a run that answered rows completes, got {body}",
            case.label
        );
        assert_eq!(
            body["queryId"].as_str(),
            Some(query_id.as_str()),
            "case {:?}: a read answers the query it addressed, got {body}",
            case.label
        );
        assert!(
            body["errorMessage"].is_null(),
            "case {:?}: a completed query carries no errorMessage, got {body}",
            case.label
        );
        let submitted_at = body["submittedAt"]
            .as_f64()
            .unwrap_or_else(|| panic!("case {:?}: no submittedAt in {body}", case.label));
        let completed_at = body["completedAt"]
            .as_f64()
            .unwrap_or_else(|| panic!("case {:?}: no completedAt in {body}", case.label));
        assert!(
            completed_at >= submitted_at,
            "case {:?}: completedAt {completed_at} precedes submittedAt {submitted_at}",
            case.label
        );
        let statistics = &body["statistics"];
        assert_eq!(
            statistics["rowCount"].as_i64(),
            Some(*expected_rows as i64),
            "case {:?}: the row count is the run's own, got {body}",
            case.label
        );
        // Every view this fixture queries holds rows, so a run over one scanned
        // stored cells even when its predicate kept none of them: the bytes are the
        // SCAN's and not the answer's.
        assert!(
            statistics["bytesScanned"].as_i64().unwrap_or(-1) > 0,
            "case {:?}: a run over a view that holds rows scanned bytes, got {body}",
            case.label
        );
        assert!(
            statistics["executionTimeInMillis"].as_i64().unwrap_or(-1) >= 0,
            "case {:?}: the execution time is a measured duration, got {body}",
            case.label
        );
        // A read reports; it does not move the query on.
        let again = described(&router, query_id).await;
        assert_eq!(
            again, body,
            "case {:?}: two reads of one query must agree",
            case.label
        );
    }
}

#[tokio::test]
async fn a_statement_the_query_engine_cannot_read_starts_no_query() {
    let (router, engine) = router_with_engine();
    fixture(&router).await;

    // Rows span the ways a statement is not one this service can answer at all: a
    // syntax error, a statement that is not a query, more than one statement, and
    // names the catalog does not hold.
    let cases: Vec<(&str, &str)> = vec![
        ("a misspelled keyword", "SELEC asset_name FROM asset"),
        ("no projection", "SELECT FROM asset"),
        ("no table", "SELECT * FROM"),
        ("a dangling WHERE", "SELECT * FROM asset WHERE"),
        ("an unterminated string", "SELECT * FROM asset WHERE asset_name = 'x"),
        ("whitespace only", "   "),
        ("not a statement at all", "wind farm 3 temperature"),
        ("two statements", "SELECT * FROM asset; SELECT * FROM asset"),
        (
            "a statement that is not a query",
            "INSERT INTO asset (asset_id) VALUES ('a')",
        ),
        ("a view the catalog does not hold", "SELECT * FROM turbines"),
        ("a column the view does not hold", "SELECT asset_colour FROM asset"),
        (
            "a column the view does not hold, in the WHERE",
            "SELECT asset_id FROM asset WHERE asset_colour = 'red'",
        ),
        ("a LIMIT that is not a count", "SELECT asset_id FROM asset LIMIT -1"),
    ];

    for (case, statement) in &cases {
        let (status, body) = start_in(&router, WORKSPACE, statement, None).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for {statement:?}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {case:?}: unexpected error code in {body}"
        );
    }
    assert_eq!(
        queries_held(&engine),
        0,
        "a refused request starts no query at all"
    );
}

#[tokio::test]
async fn a_construct_outside_the_query_language_is_a_modelled_refusal_and_starts_no_query() {
    let (router, engine) = router_with_engine();
    fixture(&router).await;

    // These statements carry clauses the query language this service answers does
    // not: the request cannot be read, which is the error this operation models for
    // it, so no query is started at all. A refusal outside the operation's modelled
    // set would be one no generated client carries an exception class for.
    let cases: Vec<(&str, &str)> = vec![
        (
            "a QUALIFY clause",
            "SELECT asset_id FROM asset QUALIFY asset_name = 'Boiler1'",
        ),
        (
            "a CLUSTER BY clause",
            "SELECT asset_id FROM asset CLUSTER BY asset_name",
        ),
        (
            "a SORT BY clause",
            "SELECT asset_id FROM asset SORT BY asset_name",
        ),
        ("an ORDER BY ALL clause", "SELECT asset_id FROM asset ORDER BY ALL"),
        ("a GROUP BY ALL clause", "SELECT asset_id FROM asset GROUP BY ALL"),
        (
            "a recursive WITH clause",
            "WITH RECURSIVE held AS (SELECT asset_id FROM asset) SELECT asset_id FROM held",
        ),
        ("a window function", "SELECT COUNT(*) OVER () FROM asset"),
        ("a function the language does not carry", "SELECT SQRT(2) FROM asset"),
    ];

    for (case, statement) in &cases {
        let (status, body) = start_in(&router, WORKSPACE, statement, None).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for {statement:?}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {case:?}: unexpected error code in {body}"
        );
    }
    assert_eq!(
        queries_held(&engine),
        0,
        "a construct outside the query language starts no query"
    );

    // The constructs a real client writes are RUN, so each starts a query of its
    // own: the engine admits the same statements through this path as through the
    // synchronous one.
    let run: Vec<(&str, &str)> = vec![
        ("SELECT DISTINCT", "SELECT DISTINCT asset_model_id FROM asset"),
        (
            "a union",
            "SELECT asset_id FROM asset UNION SELECT asset_id FROM asset",
        ),
        (
            "a FETCH clause",
            "SELECT asset_id FROM asset FETCH FIRST 1 ROW ONLY",
        ),
        ("SELECT TOP", "SELECT TOP 1 asset_id FROM asset"),
        (
            "a join",
            "SELECT a.asset_name FROM asset a JOIN asset_property p ON a.asset_id = p.asset_id",
        ),
        (
            "an aggregate over a group",
            "SELECT asset_model_id, COUNT(*) FROM asset GROUP BY asset_model_id",
        ),
    ];
    for (case, statement) in &run {
        let (status, body) = start_in(&router, WORKSPACE, statement, None).await;
        assert_eq!(
            status, 202,
            "case {case:?}: {statement:?} must be run, got {status} {body}"
        );
    }
    assert_eq!(
        queries_held(&engine),
        run.len(),
        "every statement the engine runs starts a query of its own"
    );
}

#[tokio::test]
async fn a_query_is_reachable_only_inside_the_workspace_it_was_started_in() {
    let router = router();
    fixture(&router).await;
    create_workspace(&router, "Plant2").await;
    let query_id = start(&router, "SELECT asset_name FROM asset").await;

    // A workspace this service does not hold: every form of a name no
    // `CreateWorkspace` minted, including one that differs from a held name only in
    // case, since a workspace is named exactly.
    for name in ["Plant9", "plant1", "PLANT1", "Plant1-copy"] {
        let (status, body) = start_in(&router, name, "SELECT asset_name FROM asset", None).await;
        assert_eq!(
            status, 404,
            "workspace {name:?}: expected 404, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "workspace {name:?}: unexpected error code in {body}"
        );
    }

    // A query id the workspace does not hold, and the id of a query of ANOTHER
    // workspace: both address no query of the workspace the read names.
    let unknown = format!("{query_id}-other");
    for (case, uri) in [
        ("an id no query carries", query_uri(WORKSPACE, &unknown)),
        ("another workspace", query_uri("Plant2", &query_id)),
        (
            "a workspace this service does not hold",
            query_uri("Plant9", &query_id),
        ),
    ] {
        let (status, body) = call(&router, "GET", &uri, None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 for {uri}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error code in {body}"
        );
    }
}

#[tokio::test]
async fn a_retry_carrying_the_same_client_token_starts_no_second_query() {
    let (router, engine) = router_with_engine();
    fixture(&router).await;

    let first = "11111111-1111-4111-8111-111111111111-token";
    let second = "22222222-2222-4222-8222-222222222222-token";
    // Rows: the same token with the same statement, the same token with a DIFFERENT
    // statement, a second token, and two requests carrying no token at all. A token
    // is spent by ONE request, so only a REPEAT of that request may answer an id
    // already issued; a request differing in the statement repeats no call and is
    // the conflict this operation models.
    let cases: Vec<(&str, &str, Option<&str>, Answer)> = vec![
        (
            "the first request",
            "SELECT asset_name FROM asset",
            Some(first),
            Answer::Started,
        ),
        (
            "the same token and statement",
            "SELECT asset_name FROM asset",
            Some(first),
            Answer::Replayed,
        ),
        (
            "the same token, another statement",
            "SELECT asset_id FROM asset_property",
            Some(first),
            Answer::Refused,
        ),
        (
            "another token",
            "SELECT asset_name FROM asset",
            Some(second),
            Answer::Started,
        ),
        (
            "no token",
            "SELECT asset_name FROM asset",
            None,
            Answer::Started,
        ),
        (
            "no token again",
            "SELECT asset_name FROM asset",
            None,
            Answer::Started,
        ),
    ];

    let mut by_token: HashMap<&str, String> = HashMap::new();
    let mut ids: Vec<String> = Vec::new();
    for (case, statement, token, answer) in &cases {
        let (status, body) = start_in(&router, WORKSPACE, statement, *token).await;
        if *answer == Answer::Refused {
            assert_eq!(
                status, 409,
                "case {case:?}: a request differing from the call its token was spent on \
                 expected 409, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ConflictingOperationException",
                "case {case:?}: unexpected error code in {body}"
            );
            let spent_on = token.and_then(|token| by_token.get(token)).cloned();
            assert_eq!(
                body["resourceId"].as_str().map(str::to_string),
                spent_on,
                "case {case:?}: the conflict must name the query that token's own call \
                 started, got {body}"
            );
            continue;
        }
        assert_eq!(
            status, 202,
            "case {case:?}: expected 202, got {status} {body}"
        );
        let query_id = body["queryId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no queryId in {body}"))
            .to_string();
        match (token, answer) {
            (Some(token), Answer::Replayed) => assert_eq!(
                by_token.get(token),
                Some(&query_id),
                "case {case:?}: a retry reports the query its token already started, got {body}"
            ),
            _ => {
                assert!(
                    !ids.contains(&query_id),
                    "case {case:?}: a request that is not a retry starts a query of its own, got {body}"
                );
                if let Some(token) = token {
                    by_token.insert(token, query_id.clone());
                }
                ids.push(query_id.clone());
            }
        }
    }

    assert_eq!(
        queries_held(&engine),
        ids.len(),
        "the store holds one query per request that was served as a start of its own: \
         neither the retry nor the refused request started one"
    );

    // The replayed query is the one the FIRST request started, so what it reports is
    // that request's own run: the row count of `SELECT asset_name FROM asset`, not
    // of the statement any later request under that token carried.
    tokio::time::sleep(settled()).await;
    let replayed = described(&router, &by_token[first]).await;
    let expected = rows_through_execute_query(&router, "SELECT asset_name FROM asset").await;
    assert_eq!(
        replayed["statistics"]["rowCount"].as_i64(),
        Some(expected as i64),
        "a replayed query reports the run of the statement it was started with, got {replayed}"
    );
}

#[tokio::test]
async fn a_run_that_outruns_the_engines_budget_is_a_failed_query() {
    let (router, engine) = router_with_engine();
    fixture(&router).await;

    // More rows than the query engine's row-work budget admits (`MAX_SCANNED_ROWS`
    // in `src/ops/execute_query.rs`, 20 000 rows), seeded straight into the store:
    // no API call can place this many rows inside one test, and the point of the row
    // is what the engine does when a run cannot finish.
    const ROWS_PAST_THE_BUDGET: usize = 25_000;
    {
        let state = service_state(&*engine).expect("the IoT SiteWise store");
        let mut store = state.lock();
        let model_id = store
            .assets
            .values()
            .next()
            .map(|asset| asset.asset_model_id.clone())
            .expect("the fixture's asset model");
        let now = SystemTime::now();
        for at in 0..ROWS_PAST_THE_BUDGET {
            let id = format!("seeded-asset-{at}");
            store.assets.insert(
                id.clone(),
                AssetRecord {
                    arn: format!("arn:aws:iotsitewise:us-west-2:000000000000:asset/{id}"),
                    name: format!("Seeded{at}"),
                    asset_model_id: model_id.clone(),
                    id,
                    external_id: None,
                    description: None,
                    properties: Vec::new(),
                    hierarchies: Vec::new(),
                    creation_date: now,
                    last_update_date: now,
                    active_at: now,
                    settling_state: "CREATING".to_string(),
                    tags: std::collections::HashMap::new(),
                },
            );
        }
    }

    let statement = "SELECT asset_name FROM asset";
    // The SYNCHRONOUS path refuses such a statement with the timeout error it
    // models, which is the error `StartQuery` does NOT model …
    let (status, body) = call(
        &router,
        "POST",
        "/queries/execution",
        Some(json!({ "queryStatement": statement })),
    )
    .await;
    assert_eq!(
        status, 400,
        "ExecuteQuery over more rows than the budget admits: got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "QueryTimeoutException",
        "ExecuteQuery over more rows than the budget admits: unexpected code in {body}"
    );

    // … so the asynchronous path ACCEPTS the request and reports the failure of the
    // run through the query's own status, with the message saying why.
    let query_id = start(&router, statement).await;
    tokio::time::sleep(settled()).await;
    let described = described(&router, &query_id).await;
    assert_eq!(
        described["status"].as_str(),
        Some("FAILED"),
        "a run that did not finish leaves the query FAILED, got {described}"
    );
    let message = described["errorMessage"].as_str().unwrap_or_default();
    assert!(
        !message.is_empty() && message.len() <= 1024,
        "a failed query carries a modelled errorMessage, got {described}"
    );
    assert!(
        described["statistics"].is_null(),
        "statistics are a COMPLETED query's, got {described}"
    );
    assert!(
        !described["completedAt"].is_null(),
        "a FAILED query has reached a terminal state, got {described}"
    );
}
