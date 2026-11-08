// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListColumnStatisticsTaskRuns`: the account-wide index of column
//! statistics task run ids.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: the listing reports the id of every column statistics task run this
//! account holds — whichever table, database or catalog the run covers, since the request
//! carries no member that narrows the set — newest first, a page of at most `MaxResults`
//! ids at a time (and never more ids than the response's own list may hold) with a
//! continuation token for the rest, and an empty page for an account with no run at all.
//! Reading the index writes nothing, so a run is left exactly in the state the clock had
//! put it in.
//!
//! The rows below differ along how many runs the account has (none, one, several, as many
//! as the store keeps), which table and database they belong to, the page size (each
//! published bound of `MaxResults`, one in between, and one past the number of runs there
//! are), and the status the clock had reached for each run.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The run lifecycle's clock helpers live with the suite that owns that lifecycle, so this
// suite moves the clock through the same ones.
use crate::column_statistics_task_runs::{already_past, far_future, set_due_instant};
use crate::wire::{emulator_on, ok, string_list, ACCOUNT_ID};
use service_com_amazonaws_glue_awsglue::state::{
    ColumnStatisticsRunOutcome, ColumnStatisticsTaskRunEntry, ComAmazonawsGlueAwsglueState,
    COLUMN_STATISTICS_TASK_RUN_STATE_STARTING, COLUMN_STATISTICS_TASK_RUN_STATE_STOPPED,
    TERMINAL_COLUMN_STATISTICS_TASK_RUN_STATES,
};

/// The IAM role a start names.
const ROLE: &str = "arn:aws:iam::000000000000:role/glue-column-stats-role";

/// The published bounds of `MaxResults` ("The maximum size of the response", `PageSize`
/// range 1..1000, from `smithy_query operation=ListColumnStatisticsTaskRuns`).
const MIN_MAX_RESULTS: i64 = 1;
const MAX_MAX_RESULTS: i64 = 1000;

/// The most ids ONE response may carry, from the output member's own list constraint
/// (`ColumnStatisticsTaskRunIds` targets `ColumnStatisticsTaskRunIdList`, length 0..100,
/// from `smithy_query operation=ListColumnStatisticsTaskRuns`).
const MAX_RESPONSE_IDS: usize = 100;

const DATABASE: &str = "index_db";
const OTHER_DATABASE: &str = "other_index_db";
const TABLE: &str = "index_table";
const OTHER_TABLE: &str = "other_index_table";

/// A catalog id that is NOT this account's, so a run recorded under it covers another Data
/// Catalog's table — which the account-wide index still reports, since its request carries
/// no catalog member to narrow by.
const OTHER_CATALOG: &str = "210987654321";

/// An emulator plus the engine it runs on, so a test can hold the in-flight window open,
/// or seed the store, instead of driving hundreds of requests.
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

/// Start one run of `table` WITHOUT moving the clock, so the run is still the one in flight
/// for that table when this returns, and answer the id the start minted.
async fn start_in_flight_run(
    router: &MultiServiceRouter,
    database: &str,
    table: &str,
) -> String {
    let started = ok(
        router,
        "StartColumnStatisticsTaskRun",
        &json!({ "DatabaseName": database, "TableName": table, "Role": ROLE }),
    )
    .await;
    started
        .get("ColumnStatisticsTaskRunId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("a start must mint a run id, got {started}"))
        .to_string()
}

/// Start one run of `table`, then let the clock finish it: one table has at most one run in
/// flight, so the next start of the same table is served only once this one has reached a
/// terminal state. The due instant is moved THROUGH THE STORE rather than slept for.
async fn start_run(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    database: &str,
    table: &str,
) -> String {
    let id = start_in_flight_run(router, database, table).await;
    set_due_instant(engine, already_past());
    id
}

/// One page of the index, as (the ids it reports in order, its NextToken).
async fn index_page(router: &MultiServiceRouter, body: &Value) -> (Vec<String>, Option<String>) {
    let page = ok(router, "ListColumnStatisticsTaskRuns", body).await;
    let ids = string_list(&page, "ColumnStatisticsTaskRunIds");
    let next = page
        .get("NextToken")
        .and_then(Value::as_str)
        .map(str::to_string);
    (ids, next)
}

/// The `Status` `GetColumnStatisticsTaskRun` reports for one run.
async fn status_of(router: &MultiServiceRouter, run_id: &str) -> String {
    let body = ok(
        router,
        "GetColumnStatisticsTaskRun",
        &json!({ "ColumnStatisticsTaskRunId": run_id }),
    )
    .await;
    body.get("ColumnStatisticsTaskRun")
        .and_then(|run| run.get("Status"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("the single-run read must report a Status, got {body}"))
        .to_string()
}

// ---------------------------------------------------------------------------
// What the index reports
// ---------------------------------------------------------------------------

/// The index reports the id of every run the account holds, newest first, across tables and
/// databases alike — and every id it reports is a run the single-run read serves.
#[tokio::test]
async fn the_index_reports_every_run_of_the_account_newest_first() {
    let (router, engine) = emulator_with_engine();

    // An account with no run at all has an empty index: the operation models no miss for
    // it.
    let (empty, next) = index_page(&router, &json!({})).await;
    assert!(
        empty.is_empty() && next.is_none(),
        "case {:?}: an account with no run reports an empty page and no token, got {empty:?} \
         with {next:?}",
        "no run yet"
    );

    store_table(&router, DATABASE, TABLE).await;
    store_table(&router, DATABASE, OTHER_TABLE).await;
    store_table(&router, OTHER_DATABASE, TABLE).await;

    // (case, the table the run covers) — the runs are started in this order, each after the
    // clock finished the last, so the index has an order to report.
    let starts = [
        ("the first run of one table", DATABASE, TABLE),
        ("a run of another table of that database", DATABASE, OTHER_TABLE),
        ("a run of a table of another database", OTHER_DATABASE, TABLE),
        ("a second run of the first table", DATABASE, TABLE),
    ];
    let mut started: Vec<(&str, String)> = Vec::new();
    for (case, database, table) in starts {
        started.push((case, start_run(&router, &engine, database, table).await));
    }

    let newest_first: Vec<String> = started
        .iter()
        .rev()
        .map(|(_, id)| id.clone())
        .collect();
    // (case, the request that must report exactly those ids)
    for (case, body) in [
        ("no page size at all", json!({})),
        (
            "the largest page the model accepts",
            json!({ "MaxResults": MAX_MAX_RESULTS }),
        ),
        (
            "a page size past the number of runs there are",
            json!({ "MaxResults": 7 }),
        ),
    ] {
        let (ids, next) = index_page(&router, &body).await;
        assert_eq!(
            ids, newest_first,
            "case {case:?}: the index reports every run of the account, newest first; got {ids:?}"
        );
        assert!(
            next.is_none(),
            "case {case:?}: a page that reached the end carries no token, got {next:?}"
        );
    }

    // Every id the index reports names a run this service serves, so the index is an index
    // of real records and not of strings.
    for (case, id) in &started {
        let status = status_of(&router, id).await;
        assert!(
            TERMINAL_COLUMN_STATISTICS_TASK_RUN_STATES.contains(&status.as_str()),
            "case {case:?}: the run {id} the index reports is readable and has finished, got \
             {status:?}"
        );
    }
}

/// A page is at most `MaxResults` ids, and its token resumes the index where it stopped —
/// so paging the whole index walks every run exactly once, at every page size.
#[tokio::test]
async fn paging_the_index_walks_every_run_once() {
    let (router, engine) = emulator_with_engine();
    store_table(&router, DATABASE, TABLE).await;
    store_table(&router, DATABASE, OTHER_TABLE).await;

    let mut started = Vec::new();
    for round in 0..5 {
        let table = if round % 2 == 0 { TABLE } else { OTHER_TABLE };
        started.push(start_run(&router, &engine, DATABASE, table).await);
    }
    let newest_first: Vec<String> = started.iter().rev().cloned().collect();

    // The smallest and the largest page the model accepts, one in between, and one past the
    // number of runs there are.
    for page_size in [MIN_MAX_RESULTS, 2, 5, MAX_MAX_RESULTS] {
        let case = format!("pages of {page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut body = json!({ "MaxResults": page_size });
            if let Some(cursor) = token.as_deref() {
                body["NextToken"] = json!(cursor);
            }
            let (ids, next) = index_page(&router, &body).await;
            assert!(
                i64::try_from(ids.len()).unwrap_or(i64::MAX) <= page_size,
                "case {case:?}: a page carries at most MaxResults ids, got {ids:?}"
            );
            walked.extend(ids);
            match next {
                Some(cursor) => token = Some(cursor),
                None => break,
            }
        }
        assert_eq!(
            walked, newest_first,
            "case {case:?}: paging walks every run exactly once, in the index's order; got \
             {walked:?}"
        );
    }
}

/// One stored run, terminal, started `index` seconds after the epoch — so a seeded set has
/// a known newest-first order. Every third run covers ANOTHER catalog's table of the same
/// name, so the seeded set spans catalogs as well as instants. Seeded through the store
/// rather than over the wire, because the bound under test is the bound on ONE RESPONSE,
/// which is only observable from a store holding more runs than a response may carry.
fn seeded_run(index: usize) -> ColumnStatisticsTaskRunEntry {
    let started_at = SystemTime::UNIX_EPOCH + Duration::from_secs(index as u64);
    let catalog_id = if index.is_multiple_of(3) {
        OTHER_CATALOG
    } else {
        ACCOUNT_ID
    };
    ColumnStatisticsTaskRunEntry {
        id: format!("seeded-run-{index:04}"),
        catalog_id: catalog_id.to_string(),
        database_name: DATABASE.to_string(),
        table_name: TABLE.to_string(),
        column_name_list: vec!["a".to_string()],
        role: ROLE.to_string(),
        sample_size: 0.0,
        security_configuration: None,
        status: COLUMN_STATISTICS_TASK_RUN_STATE_STOPPED.to_string(),
        settles_at: None,
        creation_time: started_at,
        start_time: started_at,
        last_updated: started_at,
        end_time: Some(started_at),
        error_message: None,
        // A seeded run has already come to rest, so what its reading of the table's data
        // produced is behind it: it covers no column and publishes nothing.
        outcome: ColumnStatisticsRunOutcome::Computed(Vec::new()),
    }
}

/// How many runs the store is seeded with for the bound below: half again as many as one
/// response may carry, so a page that ignored the bound would be caught. They go into the
/// store's own list directly, because the recorder trims it to the runs the store keeps and
/// a trimmed store could never hold more runs than one page.
const SEEDED_RUNS: usize = MAX_RESPONSE_IDS + MAX_RESPONSE_IDS / 2;

/// A page carries no more ids than the response's own list may hold, however large a page
/// the request asks for — and its token still walks the rest, whichever catalog each run
/// covers, so neither the bound on one response nor a run's catalog hides a run.
#[tokio::test]
async fn a_page_never_carries_more_ids_than_the_response_may_hold() {
    let (router, engine) = emulator_with_engine();
    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        let mut store = state.lock();
        for index in 0..SEEDED_RUNS {
            store
                .column_statistics_task_runs
                .push(seeded_run(index));
        }
    }
    let newest_first: Vec<String> = (0..SEEDED_RUNS)
        .rev()
        .map(|index| seeded_run(index).id)
        .collect();

    // The largest page the model accepts, and one id more than a response may hold: both
    // ask for more ids than the output list may carry.
    for page_size in [MAX_MAX_RESULTS, i64::try_from(MAX_RESPONSE_IDS + 1).unwrap_or(MAX_MAX_RESULTS)] {
        let case = format!("a page of {page_size} over a store of {SEEDED_RUNS} runs");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            let mut body = json!({ "MaxResults": page_size });
            if let Some(cursor) = token.as_deref() {
                body["NextToken"] = json!(cursor);
            }
            let (ids, next) = index_page(&router, &body).await;
            pages += 1;
            assert!(
                ids.len() <= MAX_RESPONSE_IDS,
                "case {case:?}: a page carries at most {MAX_RESPONSE_IDS} ids, got {} of them",
                ids.len()
            );
            walked.extend(ids);
            match next {
                Some(cursor) => token = Some(cursor),
                None => break,
            }
        }
        assert!(
            pages > 1,
            "case {case:?}: a store holding more runs than one response may carry takes more \
             than one page, got {pages}"
        );
        assert_eq!(
            walked, newest_first,
            "case {case:?}: the bounded pages still walk every run the store holds, newest \
             first; got {} ids",
            walked.len()
        );
    }
}

/// Reading the index writes nothing: a run whose owed transition is still in the future is
/// left in flight however often the index is read, and the index reports the same ids on
/// every call.
#[tokio::test]
async fn reading_the_index_leaves_every_run_as_it_was() {
    let (router, engine) = emulator_with_engine();
    store_table(&router, DATABASE, TABLE).await;

    let in_flight = start_in_flight_run(&router, DATABASE, TABLE).await;
    set_due_instant(&engine, far_future());

    let (first, _) = index_page(&router, &json!({})).await;
    let (again, _) = index_page(&router, &json!({})).await;
    assert_eq!(
        first, again,
        "case {:?}: two reads of the index report the same ids, got {again:?} after {first:?}",
        "read twice"
    );
    assert_eq!(
        first,
        vec![in_flight.clone()],
        "case {:?}: the index reports the run that is in flight, got {first:?}",
        "one run in flight"
    );
    assert_eq!(
        status_of(&router, &in_flight).await,
        COLUMN_STATISTICS_TASK_RUN_STATE_STARTING,
        "case {:?}: reading the index moves no status, so the run is still in flight",
        "read twice"
    );
}
