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
//! between), whether the request names a catalog id, the casing of the names, which table
//! of the database the runs belong to, what the start that recorded a run configured, and
//! the status the clock had reached for it.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The run lifecycle's clock helpers live with the suite that owns that lifecycle, so this
// suite moves the clock through the same ones.
use crate::column_statistics_task_runs::{already_past, far_future, set_due_instant};
use crate::wire::{emulator_on, ok, ACCOUNT_ID};
use service_com_amazonaws_glue_awsglue::state::{
    COLUMN_STATISTICS_TASK_RUN_STATE_STARTING, COLUMN_STATISTICS_TASK_RUN_STATE_STOPPED,
    TERMINAL_COLUMN_STATISTICS_TASK_RUN_STATES,
};

/// The IAM role a start names.
const ROLE: &str = "arn:aws:iam::000000000000:role/glue-column-stats-role";

/// The published bounds of `MaxResults` ("The maximum size of the response", `PageSize`
/// range 1..1000, from `smithy_query operation=GetColumnStatisticsTaskRuns`).
const MIN_MAX_RESULTS: i64 = 1;
const MAX_MAX_RESULTS: i64 = 1000;

const DATABASE: &str = "history_db";
const TABLE: &str = "history_table";
const OTHER_TABLE: &str = "other_history_table";

/// A catalog id that is NOT this account's, so a request naming it addresses another Data
/// Catalog's table of the same name (`CatalogID`: "The ID of the Data Catalog where the
/// table resides. If none is supplied, the Amazon Web Services account ID is used by
/// default").
const OTHER_CATALOG: &str = "210987654321";

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
    start_configured_run(router, engine, database, table, json!({})).await
}

/// The same, for a start that names more than the three `@required` members: `extra` is
/// merged into the body, so a row can vary what the run was configured with.
async fn start_configured_run(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    database: &str,
    table: &str,
    extra: Value,
) -> String {
    let id = start_in_flight_run(router, database, table, extra).await;
    set_due_instant(engine, already_past());
    id
}

/// Start one run of `table` and answer its id WITHOUT moving the clock, so the run is
/// still the one in flight for that table when this returns.
async fn start_in_flight_run(
    router: &MultiServiceRouter,
    database: &str,
    table: &str,
    extra: Value,
) -> String {
    let mut body = json!({ "DatabaseName": database, "TableName": table, "Role": ROLE });
    if let Some(members) = extra.as_object() {
        for (member, value) in members {
            body[member] = value.clone();
        }
    }
    let started = ok(router, "StartColumnStatisticsTaskRun", &body).await;
    started
        .get("ColumnStatisticsTaskRunId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("a start must mint a run id, got {started}"))
        .to_string()
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

// ---------------------------------------------------------------------------
// What each reported run carries
// ---------------------------------------------------------------------------

/// The entry the history of one table reports for ONE run id.
async fn run_in_page(
    router: &MultiServiceRouter,
    database: &str,
    table: &str,
    run_id: &str,
) -> Value {
    let page = ok(
        router,
        "GetColumnStatisticsTaskRuns",
        &history_body(database, table, json!({})),
    )
    .await;
    page.get("ColumnStatisticsTaskRuns")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("the listing must report a ColumnStatisticsTaskRuns list, got {page}"))
        .iter()
        .find(|run| {
            run.get("ColumnStatisticsTaskRunId").and_then(Value::as_str) == Some(run_id)
        })
        .cloned()
        .unwrap_or_else(|| panic!("the history must report the run {run_id}, got {page}"))
}

/// The `ColumnStatisticsTaskRun` the single-run read serves for one id.
async fn single_run(router: &MultiServiceRouter, run_id: &str) -> Value {
    let body = ok(
        router,
        "GetColumnStatisticsTaskRun",
        &json!({ "ColumnStatisticsTaskRunId": run_id }),
    )
    .await;
    body.get("ColumnStatisticsTaskRun")
        .cloned()
        .unwrap_or_else(|| panic!("GetColumnStatisticsTaskRun must report a run, got {body}"))
}

/// The `Status` a reported run stands in.
fn status_of(run: &Value) -> String {
    run.get("Status")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("each reported run carries its Status, got {run}"))
        .to_string()
}

/// Every run the history reports is element for element the `ColumnStatisticsTaskRun` the
/// single-run read serves for that id — whatever the start that recorded it configured, so
/// a client working from the listing is never less informed than one re-reading each run.
///
/// The rows differ along what the start named beyond the three `@required` members: an
/// explicit column list, none at all, a sample size, a catalog id, a security
/// configuration.
#[tokio::test]
async fn the_history_reports_the_projection_the_single_run_read_serves() {
    let (router, engine) = emulator_with_engine();
    store_table(&router, DATABASE, TABLE).await;

    // (case, the members this run's start names beyond DatabaseName/TableName/Role)
    let starts = [
        ("an explicit column list", json!({ "ColumnNameList": ["a"] })),
        ("no column list at all", json!({})),
        (
            "a sample size and a catalog id",
            json!({ "SampleSize": 50.0, "CatalogID": ACCOUNT_ID }),
        ),
        (
            "a security configuration",
            json!({ "SecurityConfiguration": "glue-stats-security" }),
        ),
    ];

    let mut recorded: Vec<(&str, String)> = Vec::new();
    for (case, extra) in starts {
        recorded.push((
            case,
            start_configured_run(&router, &engine, DATABASE, TABLE, extra).await,
        ));
    }

    for (case, id) in &recorded {
        let listed = run_in_page(&router, DATABASE, TABLE, id).await;
        let read = single_run(&router, id).await;
        assert_eq!(
            listed, read,
            "case {case:?}: the history reports the same run the single-run read serves; got \
             {listed} against {read}"
        );
    }
}

/// The history reports the status the CLOCK had reached for each run: the non-terminal one
/// while the owed transition is still in the future, the terminal one once it has fallen
/// due, and the stopped one for a run somebody stopped — and reading the history moves no
/// status, so a later caller sees what the previous one saw.
#[tokio::test]
async fn the_history_reports_the_status_the_clock_had_reached() {
    let (router, engine) = emulator_with_engine();
    store_table(&router, DATABASE, TABLE).await;

    // A run whose owed transition is held off is reported in flight, however often the
    // history is read.
    let in_flight = start_in_flight_run(&router, DATABASE, TABLE, json!({})).await;
    set_due_instant(&engine, far_future());
    let first = run_in_page(&router, DATABASE, TABLE, &in_flight).await;
    assert_eq!(
        status_of(&first),
        COLUMN_STATISTICS_TASK_RUN_STATE_STARTING,
        "case {:?}: a run whose transition is not due yet is reported in flight, got {first}",
        "the transition still in the future"
    );
    assert!(
        first.get("EndTime").is_none(),
        "case {:?}: a run that has not finished reports no EndTime, got {first}",
        "the transition still in the future"
    );
    let again = run_in_page(&router, DATABASE, TABLE, &in_flight).await;
    assert_eq!(
        again, first,
        "case {:?}: reading the history leaves the run exactly as it was, got {again} after \
         {first}",
        "read twice"
    );

    // Once the clock owes it the next status, THAT is what the history reports, terminal
    // and carrying the instant the run ended.
    set_due_instant(&engine, already_past());
    let settled = run_in_page(&router, DATABASE, TABLE, &in_flight).await;
    let settled_status = status_of(&settled);
    assert!(
        TERMINAL_COLUMN_STATISTICS_TASK_RUN_STATES.contains(&settled_status.as_str()),
        "case {:?}: a run whose transition fell due is reported in a terminal status, got \
         {settled}",
        "the transition fallen due"
    );
    assert!(
        settled.get("EndTime").is_some(),
        "case {:?}: a finished run reports the instant it ended, got {settled}",
        "the transition fallen due"
    );
    assert_eq!(
        settled,
        single_run(&router, &in_flight).await,
        "case {:?}: the history and the single-run read agree on the settled run, got {settled}",
        "the transition fallen due"
    );

    // A run somebody stopped is reported stopped: the next start of the table is served
    // only because the stop ended the one in flight.
    let stopped = start_in_flight_run(&router, DATABASE, TABLE, json!({})).await;
    set_due_instant(&engine, far_future());
    ok(
        &router,
        "StopColumnStatisticsTaskRun",
        &json!({ "DatabaseName": DATABASE, "TableName": TABLE }),
    )
    .await;
    let reported = run_in_page(&router, DATABASE, TABLE, &stopped).await;
    assert_eq!(
        status_of(&reported),
        COLUMN_STATISTICS_TASK_RUN_STATE_STOPPED,
        "case {:?}: a stopped run is reported stopped, got {reported}",
        "stopped before its transition fell due"
    );
    // The earlier run is still part of the history: a stop ends one run, it drops none.
    let (ids, _) = history_page(&router, &history_body(DATABASE, TABLE, json!({}))).await;
    assert_eq!(
        ids,
        vec![stopped.clone(), in_flight.clone()],
        "case {:?}: the history keeps every run of the table, newest first, got {ids:?}",
        "after a stop"
    );
}

/// A table this catalog has no run of has an empty history — this operation models no miss
/// for it — whether the database, the table or the CATALOG is the part that holds no such
/// run.
#[tokio::test]
async fn a_table_this_catalog_has_no_run_of_reports_an_empty_page() {
    let (router, engine) = emulator_with_engine();
    store_table(&router, DATABASE, TABLE).await;
    let run = start_run(&router, &engine, DATABASE, TABLE).await;

    for (case, body) in [
        (
            "a database no table of which was ever stored",
            history_body("no_such_history_db", TABLE, json!({})),
        ),
        (
            "a table that was never stored",
            history_body(DATABASE, "no_such_history_table", json!({})),
        ),
        (
            "another catalog's table of the same name",
            history_body(DATABASE, TABLE, json!({ "CatalogID": OTHER_CATALOG })),
        ),
    ] {
        let (ids, next) = history_page(&router, &body).await;
        assert!(
            ids.is_empty() && next.is_none(),
            "case {case:?}: a table this catalog has no run of reports an empty page and no \
             token, got {ids:?} with {next:?}"
        );
    }

    // The run those pages do not report is still reported by the page that addresses it,
    // so the empty pages above are empty for the right reason.
    let (ids, _) = history_page(&router, &history_body(DATABASE, TABLE, json!({}))).await;
    assert_eq!(
        ids,
        vec![run.clone()],
        "case {:?}: the catalog that holds the run reports it, got {ids:?}",
        "the addressed catalog"
    );
}
