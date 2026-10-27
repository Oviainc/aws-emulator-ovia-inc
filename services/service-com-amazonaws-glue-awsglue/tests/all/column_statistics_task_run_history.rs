// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetColumnStatisticsTaskRuns`: the run history of ONE stored
//! table.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: the listing reports the column statistics task runs of exactly the
//! catalog, database and table the request addressed — every one of them, newest first,
//! element for element the projection the single-run read serves — a page of at most
//! `MaxResults` at a time with a continuation token for the rest, and an empty page for a
//! table this account has no run of, which the operation models no miss for.
//!
//! The rows below differ along what the rule generalises over: how many runs the table has
//! (none, one, several), the page size (each published bound of `MaxResults` and one in
//! between), whether the request names a catalog id, the casing of the names, and which
//! table of the database the runs belong to.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The run lifecycle's clock helper lives with the suite that owns that lifecycle, so this
// suite moves the clock through the same one.
use crate::column_statistics_task_runs::{already_past, set_due_instant};
use crate::wire::{emulator_on, ok, ACCOUNT_ID};

/// The IAM role a start names.
const ROLE: &str = "arn:aws:iam::000000000000:role/glue-column-stats-role";

/// The published bounds of `MaxResults` ("The maximum size of the response", `PageSize`
/// range 1..1000, from `smithy_query operation=GetColumnStatisticsTaskRuns`).
const MIN_MAX_RESULTS: i64 = 1;
const MAX_MAX_RESULTS: i64 = 1000;

const DATABASE: &str = "history_db";
const TABLE: &str = "history_table";
const OTHER_TABLE: &str = "other_history_table";

/// An emulator plus the engine it runs on, so a test can hold the in-flight window open
/// instead of waiting on it.
fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    (emulator_on(engine.clone()), engine)
}

/// Store `database` holding `table` with one string column.
async fn store_table(router: &MultiServiceRouter, database: &str, table: &str) {
    let (status, parsed) = crate::wire::call(
        router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": database } }),
    )
    .await;
    assert!(
        status == 200 || crate::wire::error_code(&parsed).ends_with("AlreadyExistsException"),
        "CreateDatabase must answer 200 or report the database is already stored, got {status} \
         with {parsed}"
    );
    ok(
        router,
        "CreateTable",
        &json!({
            "DatabaseName": database,
            "TableInput": {
                "Name": table,
                "TableType": "EXTERNAL_TABLE",
                "StorageDescriptor": {
                    "Location": format!("s3://bucket/{}/", table.to_lowercase()),
                    "Columns": [{ "Name": "a", "Type": "string" }],
                },
            },
        }),
    )
    .await;
}

/// Start one run of `table` and answer the id it minted, then let the clock finish it:
/// one table has at most one run in flight, so the next start of the same table is served
/// only once this one has reached a terminal state. The due instant is moved THROUGH THE
/// STORE rather than slept for.
async fn start_run(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    database: &str,
    table: &str,
) -> String {
    let started = ok(
        router,
        "StartColumnStatisticsTaskRun",
        &json!({ "DatabaseName": database, "TableName": table, "Role": ROLE }),
    )
    .await;
    let id = started
        .get("ColumnStatisticsTaskRunId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("a start must mint a run id, got {started}"))
        .to_string();
    set_due_instant(engine, already_past());
    id
}

/// One page of the history of `table`, as (the ids it reports in order, its NextToken).
async fn history_page(
    router: &MultiServiceRouter,
    body: &Value,
) -> (Vec<String>, Option<String>) {
    let page = ok(router, "GetColumnStatisticsTaskRuns", body).await;
    let ids = page
        .get("ColumnStatisticsTaskRuns")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("the listing must report a ColumnStatisticsTaskRuns list, got {page}"))
        .iter()
        .map(|run| {
            run.get("ColumnStatisticsTaskRunId")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("each reported run carries its id, got {run}"))
                .to_string()
        })
        .collect();
    let next = page
        .get("NextToken")
        .and_then(Value::as_str)
        .map(str::to_string);
    (ids, next)
}

/// A listing body for one table, plus whatever `extra` adds.
fn history_body(database: &str, table: &str, extra: Value) -> Value {
    let mut body = json!({ "DatabaseName": database, "TableName": table });
    if let Some(members) = extra.as_object() {
        for (member, value) in members {
            body[member] = value.clone();
        }
    }
    body
}

// ---------------------------------------------------------------------------
// What the listing reports
// ---------------------------------------------------------------------------

/// The history reports every run of the addressed table, newest first, and no run of any
/// other table — whatever casing and catalog id the request names.
#[tokio::test]
async fn the_history_reports_the_runs_of_the_addressed_table_newest_first() {
    let (router, engine) = emulator_with_engine();
    store_table(&router, DATABASE, TABLE).await;
    store_table(&router, DATABASE, OTHER_TABLE).await;

    // A table with no run has an empty history: the operation models no miss for it.
    let (empty, next) = history_page(&router, &history_body(DATABASE, TABLE, json!({}))).await;
    assert!(
        empty.is_empty() && next.is_none(),
        "case {:?}: a table with no run reports an empty page and no token, got {empty:?} with \
         {next:?}",
        "no run yet"
    );

    // Three runs of the table, each started after the clock finished the last one, so the
    // history has an order to report.
    let mut started = Vec::new();
    for _ in 0..3 {
        started.push(start_run(&router, &engine, DATABASE, TABLE).await);
    }
    // One run of another table, which this history must not report.
    let other = start_run(&router, &engine, DATABASE, OTHER_TABLE).await;

    let newest_first: Vec<String> = started.iter().rev().cloned().collect();
    // (case, the request that must report exactly those runs)
    for (case, body) in [
        (
            "the table addressed by name",
            history_body(DATABASE, TABLE, json!({})),
        ),
        (
            "the table addressed in another casing",
            history_body(&DATABASE.to_uppercase(), &TABLE.to_uppercase(), json!({})),
        ),
        (
            "the table addressed with its catalog id",
            history_body(DATABASE, TABLE, json!({ "CatalogID": ACCOUNT_ID })),
        ),
        (
            "the largest page the model accepts",
            history_body(DATABASE, TABLE, json!({ "MaxResults": MAX_MAX_RESULTS })),
        ),
    ] {
        let (ids, next) = history_page(&router, &body).await;
        assert_eq!(
            ids, newest_first,
            "case {case:?}: the history reports every run of the table, newest first; got {ids:?}"
        );
        assert!(
            next.is_none(),
            "case {case:?}: a page that reached the end carries no token, got {next:?}"
        );
        assert!(
            !ids.contains(&other),
            "case {case:?}: another table's run is not part of this history, got {ids:?}"
        );
    }

    // The other table's history is its own.
    let (other_ids, _) = history_page(&router, &history_body(DATABASE, OTHER_TABLE, json!({}))).await;
    assert_eq!(
        other_ids,
        vec![other.clone()],
        "case {:?}: each table's history is its own, got {other_ids:?}",
        "the other table"
    );
}

/// A page is at most `MaxResults` runs, and its token resumes the listing where it stopped
/// — so paging the whole history walks every run exactly once, at every page size.
#[tokio::test]
async fn paging_the_history_walks_every_run_once() {
    let (router, engine) = emulator_with_engine();
    store_table(&router, DATABASE, TABLE).await;

    let mut started = Vec::new();
    for _ in 0..5 {
        started.push(start_run(&router, &engine, DATABASE, TABLE).await);
    }
    let newest_first: Vec<String> = started.iter().rev().cloned().collect();

    // The smallest and the largest page the model accepts, one in between, and one past
    // the number of runs there are.
    for page_size in [MIN_MAX_RESULTS, 2, 5, MAX_MAX_RESULTS] {
        let case = format!("pages of {page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut extra = json!({ "MaxResults": page_size });
            if let Some(cursor) = token.as_deref() {
                extra["NextToken"] = json!(cursor);
            }
            let (ids, next) = history_page(&router, &history_body(DATABASE, TABLE, extra)).await;
            assert!(
                i64::try_from(ids.len()).unwrap_or(i64::MAX) <= page_size,
                "case {case:?}: a page carries at most MaxResults runs, got {ids:?}"
            );
            walked.extend(ids);
            match next {
                Some(cursor) => token = Some(cursor),
                None => break,
            }
        }
        assert_eq!(
            walked, newest_first,
            "case {case:?}: paging walks every run exactly once, in the listing's order; got \
             {walked:?}"
        );
    }
}
