// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListQueries`, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with, so every assertion is about the
//! wire behaviour a real client sees.
//!
//! The rules these suites pin:
//!
//! * a listing enumerates exactly the queries `StartQuery` recorded for the
//!   workspace the request names, and no query of another workspace;
//! * a workspace this service does not hold is the modelled not-found refusal, not
//!   an empty page, while a held workspace with no query is an empty page;
//! * every summary reports the status the CLOCK has made due for that query, and the
//!   completion instant only once the query is terminal — the same values a read of
//!   that one query reports, and a listing changes neither;
//! * `maxResults`/`nextToken` page one stable enumeration: the pages joined are the
//!   whole listing once, in one order, whatever page size the caller asks for;
//! * `filter` returns only the queries in the status it names, and a value outside
//!   the modelled status catalogue is refused.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::QUERY_STEP;

use crate::asset_lifecycle::{call, create_asset, create_model, error_code, hex_token, router};

/// The workspace every suite below starts its queries in.
const WORKSPACE: &str = "Plant1";

/// A second workspace, so a listing can be shown to report only the queries of the
/// one it names.
const OTHER_WORKSPACE: &str = "Plant2";

/// Every `QueryStatus` a query can report. Source: `smithy_query(query="shape",
/// shape="QueryStatus")` — the enum's own value list.
const QUERY_STATUSES: [&str; 6] = [
    "SUBMITTED",
    "RUNNING",
    "COMPLETED",
    "FAILED",
    "CANCELED",
    "CANCELING",
];

/// Long enough that the whole span the clock owes a query has elapsed, so every
/// query the suite started is in its terminal status.
fn settled() -> Duration {
    2 * QUERY_STEP + Duration::from_millis(200)
}

/// `/workspaces/{workspaceName}/queries`, as `OPERATIONS.md` reports the route of
/// both `StartQuery` (POST) and `ListQueries` (GET).
fn queries_uri(workspace: &str) -> String {
    format!("/workspaces/{workspace}/queries")
}

/// Create `name` through `CreateWorkspace`, so a query started in it names a
/// workspace this service really holds.
async fn create_workspace(router: &MultiServiceRouter, name: &str) {
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

/// One asset of one model, so the views the query engine exposes hold rows and a
/// started query has something to scan.
async fn fixture(router: &MultiServiceRouter) {
    for name in [WORKSPACE, OTHER_WORKSPACE] {
        create_workspace(router, name).await;
    }
    let model_id = create_model(
        router,
        "Boiler",
        json!([{"name": "Temperature", "dataType": "DOUBLE", "type": {"measurement": {}}}]),
    )
    .await;
    let _asset = create_asset(router, "Boiler1", &model_id).await;
}

/// `StartQuery` in `workspace`, asserting it was accepted, and the `queryId` it
/// minted.
async fn start(router: &MultiServiceRouter, workspace: &str, statement: &str) -> String {
    let (status, body) = call(
        router,
        "POST",
        &queries_uri(workspace),
        Some(json!({ "queryStatement": statement })),
    )
    .await;
    assert_eq!(
        status, 202,
        "start {statement:?} in {workspace}: expected 202, got {status} {body}"
    );
    body["queryId"]
        .as_str()
        .unwrap_or_else(|| panic!("start {statement:?} in {workspace}: no queryId in {body}"))
        .to_string()
}

/// The statements the suites start their queries with: rows spanning each view the
/// engine exposes, a predicate that keeps rows, one that keeps none, an ordering and
/// a limit — so what a listing reports cannot be an echo of one statement's shape.
fn statements() -> Vec<&'static str> {
    vec![
        "SELECT asset_name FROM asset",
        "SELECT * FROM asset WHERE asset_name = 'Boiler1'",
        "SELECT asset_id FROM asset WHERE asset_name = 'Chiller9'",
        "SELECT asset_id, property_name, data_type FROM asset_property",
        "SELECT * FROM raw_time_series",
        "SELECT event_timestamp FROM latest_value_time_series",
        "SELECT asset_name FROM asset ORDER BY asset_name DESC LIMIT 1",
    ]
}

/// `ListQueries` on `workspace` with the raw query string `query` (empty for none).
async fn list_raw(
    router: &MultiServiceRouter,
    workspace: &str,
    query: &str,
) -> (u16, Value) {
    let uri = if query.is_empty() {
        queries_uri(workspace)
    } else {
        format!("{}?{query}", queries_uri(workspace))
    };
    call(router, "GET", &uri, None).await
}

/// `ListQueries`, asserting it answered and that the required `queries` member is a
/// list, and the body it answered with.
async fn listed(router: &MultiServiceRouter, workspace: &str, query: &str) -> Value {
    let (status, body) = list_raw(router, workspace, query).await;
    assert_eq!(
        status, 200,
        "list {workspace} ?{query}: expected 200, got {status} {body}"
    );
    assert!(
        body["queries"].is_array(),
        "list {workspace} ?{query}: queries is a required list, got {body}"
    );
    body
}

/// The `queryId`s a listing body reports, in the order it reported them.
fn ids(body: &Value) -> Vec<String> {
    body["queries"]
        .as_array()
        .unwrap_or_else(|| panic!("a listing holds a queries list, got {body}"))
        .iter()
        .map(|summary| {
            summary["queryId"]
                .as_str()
                .unwrap_or_else(|| panic!("a summary carries a queryId, got {summary}"))
                .to_string()
        })
        .collect()
}

/// Every page of a listing of `workspace`, walked with `page_size` when given and
/// with no `maxResults` otherwise: the ids in order, and how many pages it took.
async fn walk(
    router: &MultiServiceRouter,
    workspace: &str,
    page_size: Option<i64>,
    expected_total: usize,
) -> (Vec<String>, usize) {
    let case = match page_size {
        Some(size) => format!("maxResults={size}"),
        None => "the default page size".to_string(),
    };
    let mut collected: Vec<String> = Vec::new();
    let mut pages = 0usize;
    let mut token: Option<String> = None;
    loop {
        let mut query: Vec<String> = Vec::new();
        if let Some(size) = page_size {
            query.push(format!("maxResults={size}"));
        }
        if let Some(token) = &token {
            query.push(format!("nextToken={token}"));
        }
        let body = listed(router, workspace, &query.join("&")).await;
        let page = ids(&body);
        pages += 1;
        if let Some(size) = page_size {
            assert!(
                page.len() as i64 <= size,
                "case {case:?}: page {pages} holds {} summaries, more than asked: {body}",
                page.len()
            );
        }
        collected.extend(page);
        match body["nextToken"].as_str() {
            Some(next) => {
                // A page followed by a token has to leave something to resume from.
                assert!(
                    collected.len() < expected_total,
                    "case {case:?}: page {pages} ends the listing yet carries {next}"
                );
                token = Some(next.to_string());
            }
            None => break,
        }
        assert!(
            pages <= expected_total + 2,
            "case {case:?}: paging did not terminate after {pages} pages"
        );
    }
    (collected, pages)
}

#[tokio::test]
async fn a_listing_reports_the_queries_of_the_workspace_it_names() {
    let router = router();
    fixture(&router).await;
    create_workspace(&router, "Plant3").await;

    let mut here: Vec<String> = Vec::new();
    for statement in statements() {
        here.push(start(&router, WORKSPACE, statement).await);
    }
    let mut there: Vec<String> = Vec::new();
    for statement in statements().into_iter().take(2) {
        there.push(start(&router, OTHER_WORKSPACE, statement).await);
    }

    for (case, workspace, expected) in [
        ("the workspace the queries were started in", WORKSPACE, &here),
        ("another workspace with queries of its own", OTHER_WORKSPACE, &there),
    ] {
        let body = listed(&router, workspace, "maxResults=10000").await;
        let reported: BTreeSet<String> = ids(&body).into_iter().collect();
        let wanted: BTreeSet<String> = expected.iter().cloned().collect();
        assert_eq!(
            reported, wanted,
            "case {case:?}: a listing reports exactly that workspace's queries, got {body}"
        );
        assert!(
            body["nextToken"].is_null(),
            "case {case:?}: a listing that reported everything carries no nextToken, got {body}"
        );
    }

    // A held workspace nobody started a query in is an EMPTY page — absence of
    // queries is not absence of the workspace.
    let empty = listed(&router, "Plant3", "").await;
    assert!(
        ids(&empty).is_empty() && empty["nextToken"].is_null(),
        "a workspace with no query answers an empty page, got {empty}"
    );

    // A workspace this service does not hold, in every form of a name no
    // `CreateWorkspace` minted — including ones differing from a held name only in
    // case, since a workspace is named exactly. The workspace the PATH names is
    // resolved before the filters are read, so a request that is wrong in both ways
    // still reports the resource that is absent.
    for name in ["Plant9", "plant1", "PLANT1", "Plant1-copy"] {
        for query in ["", "filter=COMPLETED", "filter=nonsense", "nextToken=not-hex"] {
            let (status, body) = list_raw(&router, name, query).await;
            assert_eq!(
                status, 404,
                "workspace {name:?} ?{query}: expected 404, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "workspace {name:?} ?{query}: unexpected error code in {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_summary_reports_the_status_the_clock_has_made_due() {
    let router = router();
    fixture(&router).await;

    let started: Vec<String> = {
        let mut ids = Vec::new();
        for statement in statements() {
            ids.push(start(&router, WORKSPACE, statement).await);
        }
        ids
    };

    // Read straight after being started, no query is terminal yet, so none reports a
    // completion instant; and what the listing says about a query is what a read of
    // that one query says.
    let body = listed(&router, WORKSPACE, "maxResults=10000").await;
    for summary in body["queries"].as_array().expect("a queries list") {
        let query_id = summary["queryId"].as_str().unwrap_or_default();
        let status = summary["status"].as_str().unwrap_or_default();
        assert!(
            ["SUBMITTED", "RUNNING"].contains(&status),
            "case {query_id}: a query read straight after being started is not terminal, \
             got {summary}"
        );
        assert!(
            summary["completedAt"].is_null(),
            "case {query_id}: a query that has not finished reports no completedAt, got {summary}"
        );
        assert!(
            summary["submittedAt"].as_f64().unwrap_or(-1.0) > 0.0,
            "case {query_id}: submittedAt is the instant the query was submitted, got {summary}"
        );
    }

    // A listing REPORTS: two listings of one unchanged store agree, and neither
    // moves a query on.
    let again = listed(&router, WORKSPACE, "maxResults=10000").await;
    assert_eq!(
        ids(&again),
        ids(&body),
        "two listings of one store must report the same queries in the same order"
    );

    tokio::time::sleep(settled()).await;

    let settled_body = listed(&router, WORKSPACE, "maxResults=10000").await;
    for summary in settled_body["queries"].as_array().expect("a queries list") {
        let query_id = summary["queryId"].as_str().unwrap_or_default().to_string();
        assert_eq!(
            summary["status"].as_str(),
            Some("COMPLETED"),
            "case {query_id}: a run that answered rows completes, got {summary}"
        );
        let submitted_at = summary["submittedAt"].as_f64().unwrap_or(-1.0);
        let completed_at = summary["completedAt"].as_f64().unwrap_or(-1.0);
        assert!(
            completed_at >= submitted_at && submitted_at > 0.0,
            "case {query_id}: completedAt {completed_at} precedes submittedAt {submitted_at}"
        );
        // The same query, read on its own: a listing and a read of one stored query
        // cannot disagree about its status or its instants.
        let (status, read) = call(
            &router,
            "GET",
            &format!("{}/{query_id}", queries_uri(WORKSPACE)),
            None,
        )
        .await;
        assert_eq!(status, 200, "case {query_id}: describe got {status} {read}");
        assert_eq!(
            (
                read["status"].as_str(),
                read["submittedAt"].as_f64(),
                read["completedAt"].as_f64()
            ),
            (
                summary["status"].as_str(),
                summary["submittedAt"].as_f64(),
                summary["completedAt"].as_f64()
            ),
            "case {query_id}: the summary {summary} disagrees with the read {read}"
        );
    }
    assert_eq!(
        ids(&settled_body).len(),
        started.len(),
        "every started query is still listed once, got {settled_body}"
    );
}

#[tokio::test]
async fn pagination_walks_one_stable_enumeration_whatever_the_page_size() {
    let router = router();
    fixture(&router).await;

    let mut started: Vec<String> = Vec::new();
    for statement in statements() {
        started.push(start(&router, WORKSPACE, statement).await);
    }
    let total = started.len();

    // The whole listing in one page is the order every paged walk has to reproduce.
    let baseline = ids(&listed(&router, WORKSPACE, "maxResults=10000").await);
    assert_eq!(
        baseline.len(),
        total,
        "the unpaged listing holds every started query"
    );

    // Rows: a page smaller than the listing, sizes that divide it and sizes that do
    // not, exactly the listing, more than it, and no `maxResults` at all.
    let cases: [Option<i64>; 7] = [
        Some(1),
        Some(2),
        Some(3),
        Some(total as i64),
        Some(total as i64 + 1),
        Some(10000),
        None,
    ];
    for page_size in cases {
        let case = format!("maxResults={page_size:?}");
        let (walked, pages) = walk(&router, WORKSPACE, page_size, total).await;
        assert_eq!(
            walked, baseline,
            "case {case:?}: the pages joined must be the whole listing once, in one order, \
             got {walked:?}"
        );
        let expected_pages = match page_size {
            Some(size) => total.div_ceil(size as usize),
            None => 1,
        };
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} pages, walked {pages}"
        );
    }

    // The token a page carries names the query the NEXT page starts at, in the form
    // this listing issues: the hex of that query's id, built by an oracle that does
    // not share the handler's encoder.
    let first = listed(&router, WORKSPACE, "maxResults=1").await;
    assert_eq!(
        first["nextToken"].as_str(),
        Some(hex_token(&baseline[1]).as_str()),
        "the first page's token resumes at the second query, got {first}"
    );

    // A token this listing never issued is refused rather than answered with an
    // empty page: unhexable text, hex of an id no query of this workspace carries,
    // and hex of the id of a query of ANOTHER workspace.
    let elsewhere = start(&router, OTHER_WORKSPACE, "SELECT asset_name FROM asset").await;
    for (case, token) in [
        ("not a token this service issues", "not-hex".to_string()),
        ("hex of an unknown id", hex_token("00000000-0000-0000-0000-000000000000")),
        ("hex of another workspace's query", hex_token(&elsewhere)),
    ] {
        let (status, body) = list_raw(&router, WORKSPACE, &format!("nextToken={token}")).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for {token}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {case:?}: unexpected error code in {body}"
        );
    }

    // `maxResults` carries @range 1..10000 (`smithy_query(query="operation",
    // operation="ListQueries")`): each published bound is served and one past it on
    // either side is refused.
    for (case, max_results, served) in [
        ("the range minimum", 1_i64, true),
        ("the range maximum", 10000, true),
        ("one under the minimum", 0, false),
        ("one past the maximum", 10001, false),
    ] {
        let (status, body) =
            list_raw(&router, WORKSPACE, &format!("maxResults={max_results}")).await;
        if served {
            assert_eq!(
                status, 200,
                "case {case:?} (maxResults={max_results}): an in-range page size must be served, \
                 got {status} {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?} (maxResults={max_results}): an out-of-range page size must be \
                 refused, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidRequestException",
                "case {case:?} (maxResults={max_results}): unexpected error code in {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_filter_reports_only_the_queries_in_the_status_it_names() {
    let router = router();
    fixture(&router).await;

    let mut started: Vec<String> = Vec::new();
    for statement in statements() {
        started.push(start(&router, WORKSPACE, statement).await);
    }
    let all: BTreeSet<String> = started.iter().cloned().collect();

    // While the clock still owes these queries their transitions, the only invariant
    // that cannot depend on when the test runs: a filtered page reports queries in
    // THAT status and nothing else, and never a query this workspace does not hold.
    for status in QUERY_STATUSES {
        let body = listed(&router, WORKSPACE, &format!("filter={status}")).await;
        for summary in body["queries"].as_array().expect("a queries list") {
            assert_eq!(
                summary["status"].as_str(),
                Some(status),
                "case filter={status}: a filtered page reports only that status, got {summary}"
            );
            assert!(
                all.contains(summary["queryId"].as_str().unwrap_or_default()),
                "case filter={status}: a filtered page reports only this workspace's queries, \
                 got {summary}"
            );
        }
    }

    tokio::time::sleep(settled()).await;

    // Terminal statuses never move again, so the partition is now stable: every
    // query this workspace holds is COMPLETED, and no other status holds one.
    for status in QUERY_STATUSES {
        let body = listed(&router, WORKSPACE, &format!("filter={status}&maxResults=10000")).await;
        let reported: BTreeSet<String> = ids(&body).into_iter().collect();
        let expected: BTreeSet<String> = if status == "COMPLETED" {
            all.clone()
        } else {
            BTreeSet::new()
        };
        assert_eq!(
            reported, expected,
            "case filter={status}: expected {} queries, got {body}",
            expected.len()
        );
    }

    // A value outside the modelled status catalogue names no status, so it is the
    // bad request it is rather than an empty page: a status in the wrong case, a
    // near-miss, a word that is not a status, and one past the member's length.
    for (case, filter) in [
        ("the wrong case", "completed".to_string()),
        ("a near-miss", "RUNNIN".to_string()),
        ("not a status at all", "ALL".to_string()),
        ("a status of another API", "QUEUED".to_string()),
        ("one past the modelled length", "C".repeat(65)),
    ] {
        let (status, body) = list_raw(&router, WORKSPACE, &format!("filter={filter}")).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for filter={filter}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {case:?}: unexpected error code in {body}"
        );
    }
}
