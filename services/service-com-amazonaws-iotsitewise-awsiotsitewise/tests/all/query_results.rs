// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `GetQueryResults`, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with, so every assertion is about the
//! wire behaviour a real client sees.
//!
//! The rules these suites pin, over a SET of queries rather than one:
//!
//! * the results of a query are the rows its own statement answers over the store,
//!   so they are the rows the synchronous query path answers for that same
//!   statement, in the same order, and as many as the query's statistics report;
//! * each column is reported under the name the result projects it by and with its
//!   type named in this operation's own vocabulary;
//! * "Returns empty rows if the query is not yet complete": a query the clock has
//!   not taken to `COMPLETED` — one still going, and one that was cancelled —
//!   carries an empty page and no column metadata;
//! * one page carries at most `maxResults` rows and `nextToken` resumes at the row
//!   after it, so the pages of a result concatenate back into the whole result and
//!   the last page carries no token;
//! * a workspace or a query this service does not hold is the modelled not-found
//!   refusal, and a `nextToken` this operation never issued is the modelled invalid
//!   request;
//! * a page token names a position IN one enumeration of rows, so it is honoured
//!   only against the query it was issued for and refused when it was issued for
//!   another enumeration of rows;
//! * a read reports: reading a query's results neither moves the query on nor
//!   changes what the next read of it sees.
//!
//! The HTTP harness (`call`, `router`, `error_code`, `hex_token`) and the query
//! helpers (`fixture`, `start`, `described`, …) are the ones the asset and
//! workspace-query suites already define; this suite reuses them rather than
//! growing a second.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::asset_lifecycle::{call, error_code, hex_token, router};
use crate::workspace_queries::{create_workspace, described, fixture, settled, start, WORKSPACE};

/// A second workspace, so a read can be shown to reach a query only inside the
/// workspace it was started in.
const OTHER_WORKSPACE: &str = "Plant2";

/// The `ColumnDataType` a result column reports for each type the query engine
/// gives a column.
///
/// Source: `smithy_query(query="shape", shape="ScalarType")` — BOOLEAN | INT |
/// DOUBLE | TIMESTAMP | STRING — read against `ColumnDataType`, documented as "The
/// data type of a column. Valid values are STRING, DOUBLE, BOOLEAN, INTEGER,
/// TIMESTAMP, and VARIANT". Only the integer type is spelled differently in the
/// two.
const COLUMN_DATA_TYPES: [(&str, &str); 5] = [
    ("BOOLEAN", "BOOLEAN"),
    ("INT", "INTEGER"),
    ("DOUBLE", "DOUBLE"),
    ("TIMESTAMP", "TIMESTAMP"),
    ("STRING", "STRING"),
];

/// A statement whose projection covers EVERY type the engine gives a column, so the
/// catalogue above is exercised on each of its rows and not only on the string
/// columns most views carry.
const EVERY_COLUMN_TYPE: &str =
    "SELECT boolean_value, int_value, double_value, event_timestamp, string_value \
     FROM raw_time_series";

/// A statement the paging suite reads: the fixture's two assets carry two properties
/// each, so this view holds more rows than any page size the suite asks for.
const FOUR_ROWS: &str = "SELECT asset_id, property_name, data_type FROM asset_property";

/// A statement whose predicate keeps no row at all, so the query that runs it
/// completes with a result holding nothing.
const NO_ROWS: &str = "SELECT asset_id FROM asset WHERE asset_name = 'Chiller9'";

/// A statement over a DIFFERENT view from [`FOUR_ROWS`], whose result also holds
/// more than one row: paging it is a second enumeration of rows, so a token minted
/// in it names a position counted somewhere other than in [`FOUR_ROWS`]' result.
const TWO_ROWS: &str = "SELECT asset_id, asset_name FROM asset";

/// `GET /workspaces/{workspaceName}/queries/{queryId}/results`, as `OPERATIONS.md`
/// reports the route, with the query string a paged request carries.
fn results_uri(workspace: &str, query_id: &str, params: &[(&str, String)]) -> String {
    let mut uri = format!("/workspaces/{workspace}/queries/{query_id}/results");
    if !params.is_empty() {
        let query: Vec<String> = params
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect();
        uri.push('?');
        uri.push_str(&query.join("&"));
    }
    uri
}

/// `GetQueryResults` on a query of `workspace`.
async fn results(
    router: &MultiServiceRouter,
    workspace: &str,
    query_id: &str,
    params: &[(&str, String)],
) -> (u16, Value) {
    call(router, "GET", &results_uri(workspace, query_id, params), None).await
}

/// `GetQueryResults` in [`WORKSPACE`], asserting it answered, and the body.
async fn read(router: &MultiServiceRouter, query_id: &str, params: &[(&str, String)]) -> Value {
    let (status, body) = results(router, WORKSPACE, query_id, params).await;
    assert_eq!(
        status, 200,
        "results of {query_id} with {params:?}: expected 200, got {status} {body}"
    );
    body
}

/// The rows a `GetQueryResults` body reports, each cell as the string it carries.
fn page_rows(body: &Value) -> Vec<Vec<String>> {
    body["rows"]
        .as_array()
        .unwrap_or_else(|| panic!("no rows in {body}"))
        .iter()
        .map(|row| {
            row.as_array()
                .unwrap_or_else(|| panic!("a row is a list of column values, got {row}"))
                .iter()
                .map(|cell| cell.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .collect()
}

/// The column metadata a `GetQueryResults` body reports, as `(name, type)` pairs in
/// the order the result projects them.
fn page_columns(body: &Value) -> Vec<(String, String)> {
    body["columnInfo"]
        .as_array()
        .unwrap_or_else(|| panic!("no columnInfo in {body}"))
        .iter()
        .map(|column| {
            (
                column["name"].as_str().unwrap_or_default().to_string(),
                column["type"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// What the SYNCHRONOUS query path answers for `statement`, read in one page — the
/// independent oracle for a query's results, since a query's rows are what its
/// statement answers over the same store.
async fn executed(router: &MultiServiceRouter, statement: &str) -> Value {
    let (status, body) = call(
        router,
        "POST",
        "/queries/execution",
        Some(json!({ "queryStatement": statement })),
    )
    .await;
    assert_eq!(status, 200, "ExecuteQuery {statement:?}: got {status} {body}");
    assert!(
        body["nextToken"].is_null(),
        "ExecuteQuery {statement:?}: the oracle reads one page, got {body}"
    );
    body
}

/// The oracle's rows, each cell as the text a query's results must report for it:
/// `ExecuteQuery` carries a `Datum` per cell whose `scalarValue` is the value and
/// whose absence is a null cell, and a result row carries the column values
/// positionally, so a null cell holds no text there.
fn oracle_rows(body: &Value) -> Vec<Vec<String>> {
    body["rows"]
        .as_array()
        .unwrap_or_else(|| panic!("no rows in {body}"))
        .iter()
        .map(|row| {
            row["data"]
                .as_array()
                .unwrap_or_else(|| panic!("no data in {row}"))
                .iter()
                .map(|datum| {
                    datum["scalarValue"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .collect()
}

/// The oracle's columns as `(name, type)` pairs, with the type translated into the
/// vocabulary a query's results report through [`COLUMN_DATA_TYPES`].
fn oracle_columns(body: &Value) -> Vec<(String, String)> {
    body["columns"]
        .as_array()
        .unwrap_or_else(|| panic!("no columns in {body}"))
        .iter()
        .map(|column| {
            let scalar_type = column["type"]["scalarType"].as_str().unwrap_or_default();
            let reported = COLUMN_DATA_TYPES
                .iter()
                .find(|(engine_type, _)| *engine_type == scalar_type)
                .unwrap_or_else(|| panic!("{scalar_type:?} is no ScalarType the engine gives"))
                .1;
            (
                column["name"].as_str().unwrap_or_default().to_string(),
                reported.to_string(),
            )
        })
        .collect()
}

/// `maxResults` as a request parameter.
fn max_results(rows: usize) -> Vec<(&'static str, String)> {
    vec![("maxResults", rows.to_string())]
}

/// `nextToken` as a request parameter.
fn next_token(token: &str) -> Vec<(&'static str, String)> {
    vec![("nextToken", token.to_string())]
}

/// One row of the results table.
#[derive(Debug)]
struct ResultCase {
    label: &'static str,
    statement: &'static str,
}

/// The rows: statements spanning what the rule generalises over — every view the
/// engine exposes, a predicate that keeps every row, one that keeps one, one that
/// keeps none, an ordering, a `LIMIT` that cuts the answer, and a projection
/// covering every column type — so the rows reported can only be right if they came
/// from the statement's run over the store.
fn result_cases() -> Vec<ResultCase> {
    vec![
        ResultCase {
            label: "every asset",
            statement: "SELECT asset_id, asset_name FROM asset",
        },
        ResultCase {
            label: "one asset by name",
            statement: "SELECT * FROM asset WHERE asset_name = 'Boiler1'",
        },
        ResultCase {
            label: "a predicate no asset satisfies",
            statement: NO_ROWS,
        },
        ResultCase {
            label: "every asset property",
            statement: FOUR_ROWS,
        },
        ResultCase {
            label: "every stored sample",
            statement: "SELECT * FROM raw_time_series",
        },
        ResultCase {
            label: "the latest sample of each series",
            statement: "SELECT event_timestamp, double_value FROM latest_value_time_series",
        },
        ResultCase {
            label: "an ordering with a limit",
            statement: "SELECT asset_name FROM asset ORDER BY asset_name DESC LIMIT 1",
        },
        ResultCase {
            label: "every column type",
            statement: EVERY_COLUMN_TYPE,
        },
    ]
}

#[tokio::test]
async fn the_results_of_a_query_are_the_rows_its_statement_answers() {
    let router = router();
    fixture(&router).await;

    // Every row's query is started first and read straight away: a query the clock
    // still owes a transition has not completed, so it answers an empty page and no
    // column metadata whatever its statement would produce.
    let mut started: Vec<(&ResultCase, String, Value)> = Vec::new();
    let cases = result_cases();
    for case in &cases {
        let oracle = executed(&router, case.statement).await;
        let query_id = start(&router, case.statement).await;
        let body = read(&router, &query_id, &[]).await;
        assert!(
            page_rows(&body).is_empty(),
            "case {:?}: a query that is not yet complete answers empty rows, got {body}",
            case.label
        );
        assert!(
            body["columnInfo"].is_null(),
            "case {:?}: column metadata is a COMPLETED query's, got {body}",
            case.label
        );
        assert!(
            body["nextToken"].is_null(),
            "case {:?}: an empty page resumes nowhere, got {body}",
            case.label
        );
        started.push((case, query_id, oracle));
    }

    tokio::time::sleep(settled()).await;

    for (case, query_id, oracle) in &started {
        let body = read(&router, query_id, &[]).await;
        let expected_rows = oracle_rows(oracle);
        assert_eq!(
            page_rows(&body),
            expected_rows,
            "case {:?}: the results are the rows the statement answers, got {body}",
            case.label
        );
        assert_eq!(
            page_columns(&body),
            oracle_columns(oracle),
            "case {:?}: the columns are the ones the statement projects, got {body}",
            case.label
        );
        assert!(
            body["nextToken"].is_null(),
            "case {:?}: a result read in one page resumes nowhere, got {body}",
            case.label
        );
        // The query's own statistics count the rows of the same run, so the two
        // readers of one query cannot disagree about how many rows it answered.
        let read_back = described(&router, query_id).await;
        assert_eq!(
            read_back["statistics"]["rowCount"].as_i64(),
            Some(expected_rows.len() as i64),
            "case {:?}: the row count is the result's own, got {read_back}",
            case.label
        );
        // A read reports; it does not move the query on or change what the next
        // reader sees.
        let again = read(&router, query_id, &[]).await;
        assert_eq!(
            again, body,
            "case {:?}: two reads of one result must agree",
            case.label
        );
        assert_eq!(
            read_back,
            described(&router, query_id).await,
            "case {:?}: reading a result leaves the query where it was",
            case.label
        );
    }
}

#[tokio::test]
async fn every_column_type_the_engine_gives_is_reported_in_the_results_vocabulary() {
    let router = router();
    fixture(&router).await;

    let query_id = start(&router, EVERY_COLUMN_TYPE).await;
    tokio::time::sleep(settled()).await;
    let body = read(&router, &query_id, &[]).await;

    // The projection names one column of each type the engine gives, in the order
    // the catalogue lists them, so every entry of it is exercised here — including
    // the integer type, the one type the two vocabularies spell differently.
    let reported: Vec<String> = page_columns(&body)
        .into_iter()
        .map(|(_, data_type)| data_type)
        .collect();
    let expected: Vec<String> = COLUMN_DATA_TYPES
        .iter()
        .map(|(_, reported)| (*reported).to_string())
        .collect();
    assert_eq!(
        reported, expected,
        "a result reports each column's type in its own vocabulary, got {body}"
    );

    // Each sample the fixture wrote carries ONE typed value, so every row holds
    // text in exactly one of the three value columns and none in the other two: a
    // null cell keeps its position and carries no text.
    for row in page_rows(&body) {
        assert_eq!(
            row.len(),
            COLUMN_DATA_TYPES.len(),
            "a row carries one value per column, positional to columnInfo, got {row:?}"
        );
        // The value columns are the four typed ones; the fifth column of the
        // projection is the timestamp every sample carries.
        let valued = [0, 1, 2, 4]
            .iter()
            .filter(|at| !row[**at].is_empty())
            .count();
        assert_eq!(
            valued, 1,
            "a sample carries one typed value, so one value column is filled: {row:?}"
        );
        assert!(
            !row[3].is_empty(),
            "every sample carries the timestamp it was written at: {row:?}"
        );
    }
}

#[tokio::test]
async fn a_result_pages_by_max_results_and_resumes_at_the_next_row() {
    let router = router();
    fixture(&router).await;

    let whole = oracle_rows(&executed(&router, FOUR_ROWS).await);
    assert!(
        whole.len() >= 4,
        "the fixture must hold enough rows to page over, got {whole:?}"
    );
    let query_id = start(&router, FOUR_ROWS).await;
    tokio::time::sleep(settled()).await;

    // Rows: page sizes smaller than, equal to and larger than the result, and the
    // size the operation chooses when the caller asks for none. Whatever the size,
    // the pages concatenate back into the whole result exactly once.
    let sizes: Vec<(&str, Option<usize>)> = vec![
        ("one row at a time", Some(1)),
        ("two rows at a time", Some(2)),
        ("a page that does not divide the result", Some(3)),
        ("a page exactly as long as the result", Some(whole.len())),
        ("a page longer than the result", Some(whole.len() + 7)),
        ("the size the operation chooses", None),
    ];

    for (case, size) in &sizes {
        let mut paged: Vec<Vec<String>> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut params = match size {
                Some(size) => max_results(*size),
                None => Vec::new(),
            };
            if let Some(token) = &token {
                params.extend(next_token(token));
            }
            let body = read(&router, &query_id, &params).await;
            // Every page carries the whole result's column metadata: the rows of a
            // page are positional to it, so a page that answered rows without it
            // could not be read.
            assert!(
                !page_columns(&body).is_empty(),
                "case {case:?}: every page of a completed query carries its columns, got {body}"
            );
            let rows = page_rows(&body);
            if let Some(size) = size {
                assert!(
                    rows.len() <= *size,
                    "case {case:?}: a page carries at most {size} rows, got {body}"
                );
            }
            // The token is OPAQUE: it names the row the next page resumes at inside
            // the enumeration that row is counted in, so it is not the bare row
            // index a client could write down for itself — `hex_token` is the
            // harness's encoder of exactly that bare index. WHERE it resumes is
            // pinned by the walk below, which must concatenate back into the whole
            // result.
            let resumed = paged.len() + rows.len();
            match body["nextToken"].as_str() {
                Some(issued) => {
                    assert_ne!(
                        issued,
                        hex_token(&resumed.to_string()),
                        "case {case:?}: a token carries more than the row index {resumed}, \
                         got {body}"
                    );
                    token = Some(issued.to_string());
                }
                None => token = None,
            }
            paged.extend(rows);
            pages += 1;
            assert!(
                pages <= whole.len() + 2,
                "case {case:?}: paging must end, got {pages} pages for {} rows",
                whole.len()
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            paged, whole,
            "case {case:?}: the pages of a result concatenate back into it"
        );
        let expected_pages = match size {
            Some(size) => whole.len().div_ceil(*size).max(1),
            None => 1,
        };
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: a result of {} rows is {expected_pages} page(s)",
            whole.len()
        );
    }
}

/// `StartQuery` in `workspace`, asserting it was accepted, and the `queryId` it
/// minted — the workspace-query suite's own `start` always starts in [`WORKSPACE`],
/// and this suite needs a query of a second workspace too.
async fn start_in(router: &MultiServiceRouter, workspace: &str, statement: &str) -> String {
    let (status, body) = call(
        router,
        "POST",
        &format!("/workspaces/{workspace}/queries"),
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

/// The `nextToken` a one-row page of `query_id`'s results carries — a token minted
/// in THAT query's enumeration of rows.
async fn token_of(router: &MultiServiceRouter, workspace: &str, query_id: &str) -> String {
    let (status, body) = results(router, workspace, query_id, &max_results(1)).await;
    assert_eq!(
        status, 200,
        "a page of {query_id} in {workspace}: got {status} {body}"
    );
    body["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a bounded page of {query_id} carries a token, got {body}"))
        .to_string()
}

/// The `nextToken` a one-row page of the SYNCHRONOUS query path carries for
/// `statement` — a token minted in the other enumeration of rows this service pages
/// through the same token form.
async fn execute_query_token(router: &MultiServiceRouter, statement: &str) -> String {
    let (status, body) = call(
        router,
        "POST",
        "/queries/execution",
        Some(json!({ "queryStatement": statement, "maxResults": 1 })),
    )
    .await;
    assert_eq!(
        status, 200,
        "ExecuteQuery {statement:?} bounded to one row: got {status} {body}"
    );
    body["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a bounded ExecuteQuery page carries a token, got {body}"))
        .to_string()
}

#[tokio::test]
async fn a_page_token_is_honoured_only_against_the_enumeration_that_issued_it() {
    let router = router();
    fixture(&router).await;
    create_workspace(&router, OTHER_WORKSPACE).await;

    // The enumeration under test, and the enumerations a token can be minted in
    // INSTEAD: another query of the same workspace running another statement, a
    // second query running the SAME statement (so the statement cannot be what
    // identifies the enumeration), the same statement started as a query of another
    // workspace, and the synchronous query path, which pages the same engine's rows
    // through the same token form.
    let paged = start(&router, FOUR_ROWS).await;
    let other_statement = start(&router, TWO_ROWS).await;
    let same_statement = start(&router, FOUR_ROWS).await;
    let other_workspace_query = start_in(&router, OTHER_WORKSPACE, FOUR_ROWS).await;
    tokio::time::sleep(settled()).await;

    let whole = oracle_rows(&executed(&router, FOUR_ROWS).await);
    let other_whole = oracle_rows(&executed(&router, TWO_ROWS).await);
    assert!(
        whole.len() >= 4 && other_whole.len() >= 2,
        "the fixture must hold enough rows in both views to page over, \
         got {whole:?} and {other_whole:?}"
    );

    // Tokens minted at SEVERAL positions of the enumeration under test: the rule is
    // about which enumeration a position is counted in, so it is exercised at more
    // than one position.
    let mut minted: Vec<(usize, String)> = Vec::new();
    for at in [1usize, 2] {
        let body = read(&router, &paged, &max_results(at)).await;
        let token = body["nextToken"]
            .as_str()
            .unwrap_or_else(|| panic!("a page of {at} row(s) carries a token, got {body}"))
            .to_string();
        minted.push((at, token));
    }

    // The control: each of those tokens IS honoured against the query that issued
    // it, and resumes the result at the row it names.
    for (at, token) in &minted {
        let mut params = max_results(whole.len());
        params.extend(next_token(token));
        let body = read(&router, &paged, &params).await;
        assert_eq!(
            page_rows(&body),
            whole[*at..].to_vec(),
            "case {at}: the issuing query's own token resumes its result at row {at}, \
             got {body}"
        );
    }

    // Replayed against the query under test, a token minted in any other
    // enumeration names no page of it.
    let foreign: Vec<(&str, String)> = vec![
        (
            "a token of another query of this workspace",
            token_of(&router, WORKSPACE, &other_statement).await,
        ),
        (
            "a token of another query running the same statement",
            token_of(&router, WORKSPACE, &same_statement).await,
        ),
        (
            "a token of a query of another workspace",
            token_of(&router, OTHER_WORKSPACE, &other_workspace_query).await,
        ),
        (
            "a token of the synchronous query path",
            execute_query_token(&router, FOUR_ROWS).await,
        ),
    ];
    for (case, token) in &foreign {
        let (status, body) = results(&router, WORKSPACE, &paged, &next_token(token)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for {token:?}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {case:?}: unexpected error code in {body}"
        );
    }

    // And the other direction, the one that hides rows: this query's own token
    // replayed against ANOTHER query is refused rather than re-paging that query's
    // result from the position — so no leading row of it is reported in no page of
    // its own enumeration.
    for (at, token) in &minted {
        let (status, body) = results(&router, WORKSPACE, &other_statement, &next_token(token)).await;
        assert_eq!(
            status, 400,
            "case {at}: a token of another query names no page here, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {at}: unexpected error code in {body}"
        );
    }
    let body = read(&router, &other_statement, &[]).await;
    assert_eq!(
        page_rows(&body),
        other_whole,
        "the refused replay left every row of the other query's result readable, got {body}"
    );
}

#[tokio::test]
async fn a_query_that_has_not_completed_has_no_results() {
    let router = router();
    fixture(&router).await;

    // A query that is still going, and one a cancel took to its terminal status
    // without a result: neither has rows to page, whatever its statement would have
    // answered. The cancelled one is set up FIRST, so the one left going is read as
    // soon as it has been started.
    let cancelled = start(&router, FOUR_ROWS).await;
    let (status, body) = call(
        &router,
        "POST",
        &format!("/workspaces/{WORKSPACE}/queries/{cancelled}/cancel"),
        None,
    )
    .await;
    assert_eq!(status, 202, "cancel {cancelled}: got {status} {body}");

    let going = start(&router, FOUR_ROWS).await;
    let body = read(&router, &going, &[]).await;
    assert!(
        page_rows(&body).is_empty() && body["columnInfo"].is_null(),
        "a query that is still going answers an empty page, got {body}"
    );

    tokio::time::sleep(settled()).await;
    let read_back = described(&router, &cancelled).await;
    assert_eq!(
        read_back["status"].as_str(),
        Some("CANCELED"),
        "the cancel must have fallen due, got {read_back}"
    );
    let body = read(&router, &cancelled, &[]).await;
    assert!(
        page_rows(&body).is_empty() && body["columnInfo"].is_null(),
        "a cancelled query carries no results, got {body}"
    );
    assert!(
        body["nextToken"].is_null(),
        "an empty page resumes nowhere, got {body}"
    );

    // The query that was left going completes on its own, and reading its results
    // while it was going changed nothing about the rows it then answers.
    let body = read(&router, &going, &[]).await;
    assert_eq!(
        page_rows(&body),
        oracle_rows(&executed(&router, FOUR_ROWS).await),
        "a query read before it completed still answers its own rows, got {body}"
    );
}

#[tokio::test]
async fn a_result_read_refuses_what_it_cannot_resolve() {
    let router = router();
    fixture(&router).await;
    create_workspace(&router, OTHER_WORKSPACE).await;
    let query_id = start(&router, FOUR_ROWS).await;
    // A query whose predicate keeps no row: its result holds nothing, so this
    // operation issues no token for it at all.
    let empty = start(&router, NO_ROWS).await;
    tokio::time::sleep(settled()).await;
    let held = oracle_rows(&executed(&router, FOUR_ROWS).await).len();

    // A workspace this service does not hold, a query id it does not hold, and the
    // id of a query of ANOTHER workspace: each addresses no query of the workspace
    // the read names. A name differing from a held one only in case is included,
    // since both are named exactly.
    let unknown_query = format!("{query_id}-other");
    let not_found: Vec<(&str, &str, &str)> = vec![
        ("a workspace no create minted", "Plant9", query_id.as_str()),
        ("a workspace that differs in case", "plant1", query_id.as_str()),
        (
            "a query id no query carries",
            WORKSPACE,
            unknown_query.as_str(),
        ),
        (
            "a query of another workspace",
            OTHER_WORKSPACE,
            query_id.as_str(),
        ),
    ];
    for (case, workspace, id) in &not_found {
        let (status, body) = results(&router, workspace, id, &[]).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error code in {body}"
        );
    }

    // A `nextToken` this operation never issued: text that is not a token at all,
    // one that decodes to no row index, a bare row index of the result (a position
    // on its own is not one of this operation's tokens), one at the end of the
    // result (a token is issued only for a row the result HAS), and one past the
    // end.
    let at_the_end = hex_token(&held.to_string());
    let past_the_end = hex_token("9999");
    let tokens: Vec<(&str, String)> = vec![
        ("not hexadecimal at all", "zzz".to_string()),
        ("hexadecimal of no number", hex_token("page-two")),
        ("a negative offset", hex_token("-1")),
        ("a bare row index of the result", hex_token("1")),
        ("an offset at the end of the result", at_the_end),
        ("an offset past the result", past_the_end),
    ];
    for (case, token) in &tokens {
        let (status, body) = results(&router, WORKSPACE, &query_id, &next_token(token)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for {token:?}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {case:?}: unexpected error code in {body}"
        );
    }

    // A result that holds no rows: it is answered, as its own empty page, and every
    // offset into it is a token this operation never issued.
    let body = read(&router, &empty, &[]).await;
    assert!(
        page_rows(&body).is_empty() && body["nextToken"].is_null(),
        "a result of no rows is one empty page that resumes nowhere, got {body}"
    );
    for (case, token) in [
        ("the first row of an empty result", hex_token("0")),
        ("any row of an empty result", hex_token("3")),
    ] {
        let (status, body) = results(&router, WORKSPACE, &empty, &next_token(&token)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for {token:?}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {case:?}: unexpected error code in {body}"
        );
    }

    // `maxResults` carries the modelled range 1..10000, so a value outside it is
    // refused before the handler runs rather than quietly clamped.
    for (case, value) in [("below the range", "0"), ("above the range", "10001")] {
        let (status, body) = results(
            &router,
            WORKSPACE,
            &query_id,
            &[("maxResults", value.to_string())],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for maxResults={value}, got {status} {body}"
        );
    }
}
