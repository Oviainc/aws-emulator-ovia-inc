// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Glue column-statistics task-run slice:
//! `StartColumnStatisticsTaskRun` and the `GetColumnStatisticsTaskRun` that reports the
//! run it started.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: `StartColumnStatisticsTaskRun` on a stored table records a column
//! statistics task run of it and answers 200 with the id it minted; the run carries the
//! table it covers and the configuration the request named — with the columns defaulting
//! to every column the STORED table declares — and `GetColumnStatisticsTaskRun` reports it
//! under that id, in a non-terminal `ColumnStatisticsState` until the status the clock
//! owes it falls due and in a terminal one afterwards. While a run of a table is in
//! flight, another start for THAT table is refused; a start for any other table is not.
//! The status the clock owes is decided by the table's own DATA: no object store is mounted
//! beside Glue here, so every table below points at data that cannot be read and every run
//! settles in the modelled failure status carrying that reason. The runs that DO read data
//! and settle `SUCCEEDED` are in [`crate::column_statistics_computation`].
//!
//! The recorded case — one table started with an explicit column list — is ONE row of the
//! tables below; the rows differ along which members the start names, how many columns it
//! lists (none, one, several), the sample size, the casing of the names, the catalog id,
//! and whether the covered table declares any columns at all.
//!
//! The due instants the lifecycle turns on are read and moved THROUGH THE STORE rather
//! than slept for, so the transient window is deterministic: one test sleeps, to pin that
//! the transition is owed by the wall clock and not by a reader.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    column_statistics_task_run_unreadable_data, ColumnStatisticsTaskRunEntry,
    ComAmazonawsGlueAwsglueState, COLUMN_STATISTICS_COMPUTATION_TYPE_FULL,
    COLUMN_STATISTICS_TASK_RUN_NO_TABLE_LOCATION, COLUMN_STATISTICS_TASK_RUN_SETTLES_AFTER,
    COLUMN_STATISTICS_TASK_RUN_STATE_FAILED, COLUMN_STATISTICS_TASK_RUN_STATE_STARTING,
    MAX_COLUMN_STATISTICS_TASK_RUNS, TERMINAL_COLUMN_STATISTICS_TASK_RUN_STATES,
};
// The fixed sentence the in-flight refusal carries, read from the crate rather than
// spelled a second time here.
use service_com_amazonaws_glue_awsglue::state::messages::COLUMN_STATISTICS_TASK_RUN_ALREADY_STARTED;
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator_on, error_code, error_message, ok, ACCOUNT_ID};

/// The IAM role a start names: "The IAM role that the service assumes to generate
/// statistics."
const ROLE: &str = "arn:aws:iam::000000000000:role/glue-column-stats-role";

/// The lower and upper bound of the window a transition this emulator owes falls due in.
/// [`COLUMN_STATISTICS_TASK_RUN_SETTLES_AFTER`] must sit inside it, and so must every
/// run's stored due instant.
const OWED_TRANSITION_WINDOW: (Duration, Duration) =
    (Duration::from_millis(250), Duration::from_millis(750));

/// An emulator plus the engine it runs on, so a test can read and move the due instants
/// the run lifecycle turns on instead of waiting for them.
fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    (emulator_on(engine.clone()), engine)
}

/// Store `database` holding `table` with the columns `columns` names, and answer nothing:
/// the catalog state every start below is resolved against. A database several rows share
/// is created once — a repeat create is the catalog's `AlreadyExistsException`, which is
/// not what these tests are about.
async fn store_table(router: &MultiServiceRouter, database: &str, table: &str, columns: &[&str]) {
    let (status, parsed) = call(
        router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": database } }),
    )
    .await;
    assert!(
        status == 200 || error_code(&parsed).ends_with("AlreadyExistsException"),
        "CreateDatabase must answer 200 or report the database is already stored, got {status} \
         with {parsed}"
    );
    let mut table_input = json!({ "Name": table, "TableType": "EXTERNAL_TABLE" });
    if !columns.is_empty() {
        table_input["StorageDescriptor"] = json!({
            "Location": table_location(table),
            "Columns": columns
                .iter()
                .map(|name| json!({ "Name": name, "Type": "string" }))
                .collect::<Vec<Value>>(),
        });
    }
    ok(
        router,
        "CreateTable",
        &json!({ "DatabaseName": database, "TableInput": table_input }),
    )
    .await;
}

/// Why a run over the table [`store_table`] stored could not generate statistics: no
/// object store is mounted beside Glue in this suite, so a table that points at data
/// cannot have it read, and a table stored without a storage descriptor points at none at
/// all.
///
/// Read from the crate rather than spelled here, so the reason a run reports and the reason
/// asserted cannot drift apart.
pub fn unreadable_reason(table: &str, columns: &[&str]) -> String {
    if columns.is_empty() {
        return COLUMN_STATISTICS_TASK_RUN_NO_TABLE_LOCATION.to_string();
    }
    column_statistics_task_run_unreadable_data(&table_location(table))
}

/// Where [`store_table`] points a stored table's data at.
pub fn table_location(table: &str) -> String {
    format!("s3://bucket/{}/", table.to_lowercase())
}

/// Start a run and answer the `ColumnStatisticsTaskRunId` it reports.
async fn start_run(router: &MultiServiceRouter, body: &Value) -> String {
    let started = ok(router, "StartColumnStatisticsTaskRun", body).await;
    started
        .get("ColumnStatisticsTaskRunId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("StartColumnStatisticsTaskRun must answer a ColumnStatisticsTaskRunId, got {started}")
        })
        .to_string()
}

/// The `ColumnStatisticsTaskRun` structure `GetColumnStatisticsTaskRun` reports for one
/// run.
async fn get_run(router: &MultiServiceRouter, run_id: &str) -> Value {
    let body = ok(
        router,
        "GetColumnStatisticsTaskRun",
        &json!({ "ColumnStatisticsTaskRunId": run_id }),
    )
    .await;
    body.get("ColumnStatisticsTaskRun")
        .cloned()
        .unwrap_or_else(|| {
            panic!("GetColumnStatisticsTaskRun must report a ColumnStatisticsTaskRun, got {body}")
        })
}

/// A start body naming the three `@required` members, plus whatever `extra` adds.
fn start_body(database: &str, table: &str, extra: Value) -> Value {
    let mut body = json!({ "DatabaseName": database, "TableName": table, "Role": ROLE });
    if let Some(members) = extra.as_object() {
        for (member, value) in members {
            body[member] = value.clone();
        }
    }
    body
}

/// The column statistics task runs the store holds.
fn stored_runs(engine: &Arc<engine::InMemoryEngine>) -> Vec<ColumnStatisticsTaskRunEntry> {
    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let store = state.lock();
    store.column_statistics_task_runs.clone()
}

/// Move the due instant of every unsettled run to `at`, so a test can hold the transient
/// window open or make the owed transition due without sleeping.
///
/// Shared with the settings suite ([`crate::column_statistics_task_settings`]), which reads
/// the attempt a settings record reports through the same run lifecycle, so both suites
/// move the clock through ONE helper.
pub(crate) fn set_due_instant(engine: &Arc<engine::InMemoryEngine>, at: SystemTime) {
    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let mut store = state.lock();
    for run in &mut store.column_statistics_task_runs {
        if run.settles_at.is_some() {
            run.settles_at = Some(at);
        }
    }
}

/// An instant far enough out that no owed transition falls due during a test.
pub(crate) fn far_future() -> SystemTime {
    SystemTime::now() + Duration::from_secs(3600)
}

/// An instant already past, so the next call settles what the clock owed.
pub(crate) fn already_past() -> SystemTime {
    SystemTime::now() - Duration::from_secs(1)
}

/// The `ColumnNameList` a reported run carries, in the order it reports them.
fn reported_columns(run: &Value) -> Vec<String> {
    run.get("ColumnNameList")
        .and_then(Value::as_array)
        .map(|names| {
            names
                .iter()
                .filter_map(|name| name.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// A numeric member of a reported run, taking an absent member as the model's own default
/// of 0 — awsJson omits a member whose value equals its modelled default.
fn reported_number(run: &Value, member: &str) -> f64 {
    run.get(member).and_then(Value::as_f64).unwrap_or(0.0)
}

// ---------------------------------------------------------------------------
// Start, then read back
// ---------------------------------------------------------------------------

/// One row of the start-and-read-back table below: a stored table, a start of it, and
/// what the run reported for it must carry. A named structure rather than a tuple, so
/// every column of the table reads as what it is.
struct StartCase<'a> {
    /// What this row varies, named in each of its assertions.
    case: &'a str,
    database: &'a str,
    table: &'a str,
    /// The columns the stored table declares.
    columns: &'a [&'a str],
    /// The start members beyond the three `@required` ones.
    extra: Value,
    /// The columns the run must cover.
    expected_columns: Vec<&'a str>,
    /// The `SampleSize` the run must report.
    expected_sample: f64,
}

/// What `StartColumnStatisticsTaskRun` records is what `GetColumnStatisticsTaskRun`
/// reports: the table the run covers, the configuration the request named, and the
/// columns the request listed — else every column the stored table declares.
///
/// Each row is a stored table plus a start of it, and the members the reported run must
/// carry. The rows differ along the number of columns listed (none, an empty list, one,
/// two, every one the table has), whether the table declares columns at all, the sample
/// size (absent, each published bound, one in between), the casing of the names, and
/// whether the start names a catalog id or a security configuration.
#[tokio::test]
async fn a_started_run_is_read_back_through_get_column_statistics_task_run() {
    let (router, engine) = emulator_with_engine();

    let cases: Vec<StartCase<'_>> = vec![
        StartCase {
            // The recorded case: an explicit column list over a stored table.
            case: "an explicit column list",
            database: "stats_db",
            table: "events",
            columns: &["id", "amount", "country"],
            extra: json!({ "ColumnNameList": ["id", "amount"] }),
            expected_columns: vec!["id", "amount"],
            expected_sample: 0.0,
        },
        StartCase {
            // "If none is supplied, all column names for the table will be used by
            // default" — the default is read from the stored table.
            case: "no column list at all",
            database: "stats_db",
            table: "orders",
            columns: &["order_id", "total", "placed_at"],
            extra: json!({}),
            expected_columns: vec!["order_id", "total", "placed_at"],
            expected_sample: 0.0,
        },
        StartCase {
            case: "one column of several",
            database: "stats_db",
            table: "customers",
            columns: &["customer_id", "email"],
            extra: json!({ "ColumnNameList": ["email"] }),
            expected_columns: vec!["email"],
            expected_sample: 0.0,
        },
        StartCase {
            // A table stored without a storage descriptor declares no columns, so the
            // default list is empty.
            case: "a table that declares no columns",
            database: "stats_db",
            table: "columnless",
            columns: &[],
            extra: json!({}),
            expected_columns: vec![],
            expected_sample: 0.0,
        },
        StartCase {
            // `SampleSize` has `range 0..100`: the lower bound, which is also the
            // model's default and means the entire table.
            case: "the lower sample-size bound",
            database: "stats_db",
            table: "sampled_low",
            columns: &["a"],
            extra: json!({ "SampleSize": 0.0 }),
            expected_columns: vec!["a"],
            expected_sample: 0.0,
        },
        StartCase {
            case: "the upper sample-size bound",
            database: "stats_db",
            table: "sampled_high",
            columns: &["a", "b"],
            extra: json!({ "SampleSize": 100.0 }),
            expected_columns: vec!["a", "b"],
            expected_sample: 100.0,
        },
        StartCase {
            case: "a sample size between the bounds",
            database: "stats_db",
            table: "sampled_mid",
            columns: &["a"],
            extra: json!({ "SampleSize": 12.5 }),
            expected_columns: vec!["a"],
            expected_sample: 12.5,
        },
        StartCase {
            // An explicitly EMPTY list supplies no column name, so it takes the same
            // default as an absent one.
            case: "an empty column list",
            database: "stats_db",
            table: "empty_list",
            columns: &["a", "b"],
            extra: json!({ "ColumnNameList": [] }),
            expected_columns: vec!["a", "b"],
            expected_sample: 0.0,
        },
        StartCase {
            // Database and table names are folded to lowercase when stored, so the run
            // reports the stored spelling whatever casing the start sent.
            case: "mixed-case names",
            database: "Stats_DB_Mixed",
            table: "Daily_Sales",
            columns: &["Day", "Revenue"],
            extra: json!({ "ColumnNameList": ["Day"] }),
            expected_columns: vec!["Day"],
            expected_sample: 0.0,
        },
        StartCase {
            // "The ID of the Data Catalog where the table reside. If none is supplied,
            // the Amazon Web Services account ID is used by default."
            case: "the catalog named explicitly",
            database: "stats_db_catalog",
            table: "explicit_catalog",
            columns: &["a"],
            extra: json!({ "CatalogID": ACCOUNT_ID, "SecurityConfiguration": "stats-security" }),
            expected_columns: vec!["a"],
            expected_sample: 0.0,
        },
    ];

    for StartCase {
        case,
        database,
        table,
        columns,
        extra,
        expected_columns,
        expected_sample,
    } in cases
    {
        store_table(&router, database, table, columns).await;
        let run_id = start_run(&router, &start_body(database, table, extra.clone())).await;
        assert!(
            run_id.len() == 32
                && run_id
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
            "case {case:?}: the run id must be 32 lowercase hex characters, got {run_id:?}"
        );

        // Hold the owed transition open, so the run is read in its transient status.
        set_due_instant(&engine, far_future());
        let run = get_run(&router, &run_id).await;
        assert_eq!(
            run.get("ColumnStatisticsTaskRunId").and_then(Value::as_str),
            Some(run_id.as_str()),
            "case {case:?}: the run is reported under the id the start minted, got {run}"
        );
        assert_eq!(
            run.get("Status").and_then(Value::as_str),
            Some(COLUMN_STATISTICS_TASK_RUN_STATE_STARTING),
            "case {case:?}: a run whose owed transition is not due yet is still \
             {COLUMN_STATISTICS_TASK_RUN_STATE_STARTING}, got {run}"
        );
        assert_eq!(
            run.get("DatabaseName").and_then(Value::as_str),
            Some(database.to_lowercase().as_str()),
            "case {case:?}: the run covers the stored database, got {run}"
        );
        assert_eq!(
            run.get("TableName").and_then(Value::as_str),
            Some(table.to_lowercase().as_str()),
            "case {case:?}: the run covers the stored table, got {run}"
        );
        assert_eq!(
            reported_columns(&run),
            expected_columns
                .iter()
                .map(|name| (*name).to_string())
                .collect::<Vec<String>>(),
            "case {case:?}: expected the covered columns {expected_columns:?}, got {run}"
        );
        assert_eq!(
            run.get("Role").and_then(Value::as_str),
            Some(ROLE),
            "case {case:?}: the run reports the role the start named, got {run}"
        );
        assert_eq!(
            run.get("CatalogID").and_then(Value::as_str),
            Some(ACCOUNT_ID),
            "case {case:?}: the run reports the catalog it was resolved in, got {run}"
        );
        assert_eq!(
            run.get("CustomerId").and_then(Value::as_str),
            Some(ACCOUNT_ID),
            "case {case:?}: the run reports the account it belongs to, got {run}"
        );
        assert_eq!(
            run.get("ComputationType").and_then(Value::as_str),
            Some(COLUMN_STATISTICS_COMPUTATION_TYPE_FULL),
            "case {case:?}: a run started this way computes the whole statistic set, got {run}"
        );
        assert_eq!(
            reported_number(&run, "SampleSize"),
            expected_sample,
            "case {case:?}: expected SampleSize {expected_sample}, got {run}"
        );
        assert_eq!(
            run.get("SecurityConfiguration").and_then(Value::as_str),
            extra.get("SecurityConfiguration").and_then(Value::as_str),
            "case {case:?}: the run reports the security configuration the start named, got {run}"
        );
        // No worker ran, so nothing reports a worker type or a resource figure.
        assert!(
            run.get("WorkerType").is_none(),
            "case {case:?}: no worker generated statistics, so the run names no WorkerType, \
             got {run}"
        );
        assert_eq!(
            reported_number(&run, "NumberOfWorkers"),
            0.0,
            "case {case:?}: no worker generated statistics, got {run}"
        );
        assert_eq!(
            reported_number(&run, "DPUSeconds"),
            0.0,
            "case {case:?}: no worker consumed DPU seconds, got {run}"
        );
        assert!(
            run.get("EndTime").is_none(),
            "case {case:?}: a run that has not finished has no EndTime, got {run}"
        );
        assert!(
            run.get("ErrorMessage").is_none(),
            "case {case:?}: a run that has not failed carries no ErrorMessage, got {run}"
        );
        let creation_time = reported_number(&run, "CreationTime");
        assert!(
            creation_time > 0.0
                && (reported_number(&run, "StartTime") - creation_time).abs() < f64::EPSILON
                && (reported_number(&run, "LastUpdated") - creation_time).abs() < f64::EPSILON,
            "case {case:?}: a run that has not moved reports one instant for its creation, \
             its start and its last update, got {run}"
        );

        // A read reports the run, it never moves it: a second read of the same
        // not-yet-due run answers exactly what the first did.
        let read_again = get_run(&router, &run_id).await;
        assert_eq!(
            read_again, run,
            "case {case:?}: reading a run must not change what a later caller sees, got \
             {read_again} after {run}"
        );

        // Now let the clock's owed status fall due: the run finishes in the modelled
        // failure status, because nothing generated statistics for it.
        set_due_instant(&engine, already_past());
        let settled = get_run(&router, &run_id).await;
        assert_eq!(
            settled.get("Status").and_then(Value::as_str),
            Some(COLUMN_STATISTICS_TASK_RUN_STATE_FAILED),
            "case {case:?}: once the owed transition is due the run is \
             {COLUMN_STATISTICS_TASK_RUN_STATE_FAILED}, got {settled}"
        );
        assert!(
            TERMINAL_COLUMN_STATISTICS_TASK_RUN_STATES
                .contains(&settled["Status"].as_str().unwrap_or_default()),
            "case {case:?}: the settled status must be terminal, got {settled}"
        );
        assert_eq!(
            settled.get("ErrorMessage").and_then(Value::as_str),
            Some(unreadable_reason(table, columns).as_str()),
            "case {case:?}: a run that could not read the table's data says so, got {settled}"
        );
        assert!(
            reported_number(&settled, "EndTime") > 0.0,
            "case {case:?}: a finished run reports an EndTime, got {settled}"
        );
        let read_settled_again = get_run(&router, &run_id).await;
        assert_eq!(
            read_settled_again, settled,
            "case {case:?}: a settled run reports the same instants on every later read, got \
             {read_settled_again} after {settled}"
        );
    }
}

/// The transition this emulator owes a run is due inside the window the project fixes,
/// and never at once: a reader straight after the start still sees the transient status.
#[tokio::test]
async fn the_owed_transition_is_due_inside_the_chosen_window() {
    let (lower, upper) = OWED_TRANSITION_WINDOW;
    assert!(
        COLUMN_STATISTICS_TASK_RUN_SETTLES_AFTER >= lower
            && COLUMN_STATISTICS_TASK_RUN_SETTLES_AFTER <= upper,
        "case {:?}: the owed transition must fall due in {lower:?}..={upper:?}, got {:?}",
        "the configured span",
        COLUMN_STATISTICS_TASK_RUN_SETTLES_AFTER
    );

    let (router, engine) = emulator_with_engine();
    store_table(&router, "window_db", "window_table", &["a"]).await;
    let before = SystemTime::now();
    let run_id = start_run(&router, &start_body("window_db", "window_table", json!({}))).await;

    let runs = stored_runs(&engine);
    let run = runs
        .iter()
        .find(|run| run.id == run_id)
        .unwrap_or_else(|| panic!("the store must hold run {run_id:?}"));
    let due = run
        .settles_at
        .unwrap_or_else(|| panic!("a run in flight owes a transition, got {run:?}"));
    let span = due
        .duration_since(before)
        .unwrap_or_else(|error| panic!("the due instant must be after the start, got {error}"));
    assert!(
        span >= lower && span <= upper + Duration::from_millis(100),
        "case {:?}: the stored due instant must be {lower:?}..={upper:?} out, got {span:?}",
        "the stored due instant"
    );

    // Read straight after the start: the run is still in flight, so the status the start
    // recorded is observable.
    let reported = get_run(&router, &run_id).await;
    assert_eq!(
        reported.get("Status").and_then(Value::as_str),
        Some(COLUMN_STATISTICS_TASK_RUN_STATE_STARTING),
        "case {:?}: a run read straight after the start is still in flight, got {reported}",
        "read straight after the start"
    );
}

/// The clock owes the transition, not the reader: a run left alone for longer than the
/// span is terminal on its first read afterwards, with nothing in the store touched.
#[tokio::test]
async fn the_wall_clock_settles_a_run_nobody_read() {
    let (router, _engine) = emulator_with_engine();
    store_table(&router, "clock_db", "clock_table", &["a"]).await;
    let run_id = start_run(&router, &start_body("clock_db", "clock_table", json!({}))).await;

    tokio::time::sleep(COLUMN_STATISTICS_TASK_RUN_SETTLES_AFTER + Duration::from_millis(150)).await;

    let run = get_run(&router, &run_id).await;
    assert_eq!(
        run.get("Status").and_then(Value::as_str),
        Some(COLUMN_STATISTICS_TASK_RUN_STATE_FAILED),
        "case {:?}: the wall clock alone makes the owed transition due, got {run}",
        "slept past the span"
    );
}

// ---------------------------------------------------------------------------
// One run per table at a time
// ---------------------------------------------------------------------------

/// A start for a table whose run is in flight is refused; a start for any other table is
/// not; and once the in-flight run has finished the same table can be started again.
#[tokio::test]
async fn a_start_is_refused_while_that_table_has_a_run_in_flight() {
    let (router, engine) = emulator_with_engine();
    store_table(&router, "busy_db", "busy_table", &["a", "b"]).await;
    store_table(&router, "busy_db", "other_table", &["a"]).await;
    store_table(&router, "quiet_db", "busy_table", &["a"]).await;

    let first = start_run(&router, &start_body("busy_db", "busy_table", json!({}))).await;
    set_due_instant(&engine, far_future());

    // (case, the start that must be refused while `first` is in flight)
    let refused: Vec<(&str, Value)> = vec![
        (
            "the same table again",
            start_body("busy_db", "busy_table", json!({})),
        ),
        (
            "the same table in another casing",
            start_body("BUSY_DB", "Busy_Table", json!({})),
        ),
        (
            "the same table with a different column list",
            start_body("busy_db", "busy_table", json!({ "ColumnNameList": ["b"] })),
        ),
    ];
    for (case, body) in refused {
        let (status, parsed) = call(&router, "StartColumnStatisticsTaskRun", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a second start for a table with a run in flight is refused, got \
             {status} with {parsed}"
        );
        assert!(
            error_code(&parsed).ends_with("ColumnStatisticsTaskRunningException"),
            "case {case:?}: expected ColumnStatisticsTaskRunningException, got {}",
            error_code(&parsed)
        );
        // The refusal's wording is this error class's own fixed sentence: it names
        // neither the table the request addressed nor the run in flight, so every row
        // above is refused with the very same text.
        assert_eq!(
            error_message(&parsed),
            COLUMN_STATISTICS_TASK_RUN_ALREADY_STARTED,
            "case {case:?}: the refusal carries this error class's fixed message, got {:?}",
            error_message(&parsed)
        );
    }

    // Another table is a different statistic set, so its run is unaffected.
    for (case, database, table) in [
        ("another table of the same database", "busy_db", "other_table"),
        ("the same table name in another database", "quiet_db", "busy_table"),
    ] {
        let other = start_run(&router, &start_body(database, table, json!({}))).await;
        assert_ne!(
            other, first,
            "case {case:?}: each start mints its own run id, got {other} twice"
        );
    }

    // Once the clock has finished the in-flight run, the table can be started again.
    set_due_instant(&engine, already_past());
    let again = start_run(&router, &start_body("busy_db", "busy_table", json!({}))).await;
    assert_ne!(
        again, first,
        "case {:?}: the restart is a new run, got the first run's id again",
        "after the first run finished"
    );
    let finished = get_run(&router, &first).await;
    assert_eq!(
        finished.get("Status").and_then(Value::as_str),
        Some(COLUMN_STATISTICS_TASK_RUN_STATE_FAILED),
        "case {:?}: the finished run keeps its terminal status, got {finished}",
        "after the restart"
    );
    assert!(
        stored_runs(&engine).len() <= MAX_COLUMN_STATISTICS_TASK_RUNS,
        "case {:?}: the store keeps at most {MAX_COLUMN_STATISTICS_TASK_RUNS} runs, got {}",
        "the retention bound",
        stored_runs(&engine).len()
    );
}

// ---------------------------------------------------------------------------
// Misses
// ---------------------------------------------------------------------------

/// A start that addresses no stored table, and a read that addresses no stored run, are
/// each refused as a miss with the operation's modelled `EntityNotFoundException`.
#[tokio::test]
async fn a_request_that_addresses_nothing_stored_is_refused_as_a_miss() {
    let (router, _engine) = emulator_with_engine();
    store_table(&router, "miss_db", "miss_table", &["a"]).await;

    // (case, operation, body, the text the refusal must carry)
    let cases: Vec<(&str, &str, Value, &str)> = vec![
        (
            "a database the catalog does not hold",
            "StartColumnStatisticsTaskRun",
            start_body("no_such_db", "miss_table", json!({})),
            "Database no_such_db not found.",
        ),
        (
            "a table the database does not hold",
            "StartColumnStatisticsTaskRun",
            start_body("miss_db", "no_such_table", json!({})),
            "Table no_such_table not found.",
        ),
        (
            "a run id nothing minted",
            "GetColumnStatisticsTaskRun",
            json!({ "ColumnStatisticsTaskRunId": "ffffffffffffffffffffffffffffffff" }),
            // The read's miss names the MEMBER the request addressed the run by and
            // withholds the id it sent, as the settings miss of the same surface does.
            "For given input ColumnStatisticsTaskRunId, associated ColumnStatisticsTaskRun \
             does not exist",
        ),
    ];

    for (case, operation, body, expected) in cases {
        let (status, parsed) = call(&router, operation, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a miss is refused with 400, got {status} with {parsed}"
        );
        assert!(
            error_code(&parsed).ends_with("EntityNotFoundException"),
            "case {case:?}: expected EntityNotFoundException, got {}",
            error_code(&parsed)
        );
        let message = error_message(&parsed);
        assert!(
            message.contains(expected),
            "case {case:?}: expected a refusal carrying {expected:?}, got {message:?}"
        );
    }

    // A `CatalogID` naming another account's Data Catalog is not a miss: the caller
    // holds no grant on that catalog, which is decided before any lookup, so the start
    // is refused as unauthorized and names the catalog ARN it addressed.
    let foreign = "210987654321";
    let (status, parsed) = call(
        &router,
        "StartColumnStatisticsTaskRun",
        &start_body("miss_db", "miss_table", json!({ "CatalogID": foreign })),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {:?}: expected 400, got {status} with {parsed}",
        "a catalog of another account"
    );
    assert!(
        error_code(&parsed).ends_with("AccessDeniedException"),
        "case {:?}: expected AccessDeniedException, got {}",
        "a catalog of another account",
        error_code(&parsed)
    );
    assert!(
        error_message(&parsed).contains(foreign),
        "case {:?}: the refusal names the catalog it refused, got {:?}",
        "a catalog of another account",
        error_message(&parsed)
    );
}
