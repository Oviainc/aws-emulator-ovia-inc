// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Glue data-quality rule-recommendation slice:
//! `StartDataQualityRuleRecommendationRun`, the
//! `GetDataQualityRuleRecommendationRun` that reports the run it started, and the
//! `CancelDataQualityRuleRecommendationRun` that stops it.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: a start records a rule recommendation run of the data source its
//! `DataSource` addresses and answers 200 with the `RunId` it minted; the run carries that
//! data source and the configuration the request named — with every member the request
//! left unset filled in with the default the model documents for it — and the read reports
//! it under that id, in a non-terminal `TaskStatusType` until the status the clock owes it
//! falls due and in a terminal one afterwards, where a stored table yields the ruleset
//! recommended over the columns it declares and a data source the catalog holds nothing
//! for yields the catalog's own miss. While a run of a data source is in flight, another
//! start for THAT data source is refused with `ConflictException`, and so is a start
//! repeating a `ClientToken` an earlier start already used — a token stands for one start,
//! so the second carrying it is the duplicate the service refuses and records no run.
//!
//! The recorded case — a second start refused as a conflict — is ONE row of the tables
//! below; the rows differ along which `DataSource` member carries the table, which
//! optional members the start names, the number of workers, the timeout, the
//! recommendation mode, the casing of the names, the catalog id, and how many columns the
//! covered table declares.
//!
//! The due instants the lifecycle turns on are read and moved THROUGH THE STORE rather
//! than slept for, so the transient window is deterministic: one test sleeps, to pin that
//! the transition is owed by the wall clock and not by a reader.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    recommended_ruleset, ComAmazonawsGlueAwsglueState, DataQualityRuleRecommendationRunEntry,
    DATA_QUALITY_RULE_RECOMMENDATION_RUN_SETTLES_AFTER, DATA_QUALITY_RUN_STATE_FAILED,
    DATA_QUALITY_RUN_STATE_STARTING, DATA_QUALITY_RUN_STATE_STOPPED,
    DATA_QUALITY_RUN_STATE_STOPPING, DATA_QUALITY_RUN_STATE_SUCCEEDED,
    DEFAULT_DATA_QUALITY_RUN_TIMEOUT_MINUTES, DEFAULT_DATA_QUALITY_RUN_WORKERS,
    DEFAULT_RECOMMENDATION_MODE, MAX_DATA_QUALITY_RULESET_BYTES,
    MAX_DATA_QUALITY_RULE_RECOMMENDATION_RUNS, TERMINAL_DATA_QUALITY_RUN_STATES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{assert_echoed, call, emulator_on, error_code, error_message, ok, ACCOUNT_ID};

/// The IAM role a start names: "The IAM role that Glue assumes to access resources for the
/// run."
const ROLE: &str = "arn:aws:iam::000000000000:role/glue-data-quality-role";

/// The lower and upper bound of the window a transition this emulator owes falls due in.
/// [`DATA_QUALITY_RULE_RECOMMENDATION_RUN_SETTLES_AFTER`] must sit inside it, and so must
/// every run's stored due instant.
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
            "Location": format!("s3://bucket/{}/", table.to_lowercase()),
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

/// A `DataSource` addressing `database`.`table` through its plain `GlueTable` member, plus
/// whatever `extra` adds to that table reference.
fn glue_table_source(database: &str, table: &str, extra: Value) -> Value {
    let mut glue_table = json!({ "DatabaseName": database, "TableName": table });
    if let Some(members) = extra.as_object() {
        for (member, value) in members {
            glue_table[member] = value.clone();
        }
    }
    json!({ "GlueTable": glue_table })
}

/// A start body naming the two `@required` members, plus whatever `extra` adds.
fn start_body(data_source: &Value, extra: Value) -> Value {
    let mut body = json!({ "DataSource": data_source, "Role": ROLE });
    if let Some(members) = extra.as_object() {
        for (member, value) in members {
            body[member] = value.clone();
        }
    }
    body
}

/// Start a run and answer the `RunId` it reports.
async fn start_run(router: &MultiServiceRouter, body: &Value) -> String {
    let started = ok(router, "StartDataQualityRuleRecommendationRun", body).await;
    started
        .get("RunId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("StartDataQualityRuleRecommendationRun must answer a RunId, got {started}")
        })
        .to_string()
}

/// What `GetDataQualityRuleRecommendationRun` reports for one run.
async fn get_run(router: &MultiServiceRouter, run_id: &str) -> Value {
    ok(
        router,
        "GetDataQualityRuleRecommendationRun",
        &json!({ "RunId": run_id }),
    )
    .await
}

/// The rule recommendation runs the store holds.
fn stored_runs(engine: &Arc<engine::InMemoryEngine>) -> Vec<DataQualityRuleRecommendationRunEntry> {
    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let store = state.lock();
    store.data_quality_rule_recommendation_runs.clone()
}

/// Move the due instant of every unsettled run to `at`, so a test can hold the transient
/// window open or make the owed transition due without sleeping.
fn set_due_instant(engine: &Arc<engine::InMemoryEngine>, at: SystemTime) {
    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let mut store = state.lock();
    for run in &mut store.data_quality_rule_recommendation_runs {
        if run.settles_at.is_some() {
            run.settles_at = Some(at);
        }
    }
}

/// An instant far enough out that no owed transition falls due during a test.
fn far_future() -> SystemTime {
    SystemTime::now() + Duration::from_secs(3600)
}

/// An instant already past, so the next call settles what the clock owed.
fn already_past() -> SystemTime {
    SystemTime::now() - Duration::from_secs(1)
}

/// A numeric member of a reported run, taking an absent member as the model's own default
/// of 0 — awsJson omits a member whose value equals its modelled default.
fn reported_number(run: &Value, member: &str) -> f64 {
    run.get(member).and_then(Value::as_f64).unwrap_or(0.0)
}

// ---------------------------------------------------------------------------
// Start, then read back
// ---------------------------------------------------------------------------

/// One row of the start-and-read-back table below: a stored table, a start over it, and
/// what the reported run must carry. A named structure rather than a tuple, so every column
/// of the table reads as what it is.
struct StartCase<'a> {
    /// What this row varies, named in each of its assertions.
    case: &'a str,
    database: &'a str,
    table: &'a str,
    /// The columns the stored table declares.
    columns: &'a [&'a str],
    /// The `DataSource` the start carries.
    data_source: Value,
    /// The start members beyond the two `@required` ones.
    extra: Value,
    /// The `NumberOfWorkers` the run must report.
    expected_workers: f64,
    /// The `Timeout` in minutes the run must report.
    expected_timeout: f64,
    /// The `RecommendationMode` the run must report.
    expected_mode: &'a str,
    /// The DQDL ruleset the finished run must recommend, or `None` when it recommends
    /// nothing.
    expected_ruleset: Option<String>,
}

/// The DQDL a finished run recommends over `columns`: one `ColumnExists` rule per column
/// the covered table declares, in the order it declares them. Written out here rather than
/// asked of the crate, so the expected text is pinned by this test.
fn expected_dqdl(columns: &[&str]) -> Option<String> {
    if columns.is_empty() {
        return None;
    }
    let rules: Vec<String> = columns
        .iter()
        .map(|column| format!("    ColumnExists \"{column}\""))
        .collect();
    Some(format!("Rules = [\n{}\n]", rules.join(",\n")))
}

/// What `StartDataQualityRuleRecommendationRun` records is what
/// `GetDataQualityRuleRecommendationRun` reports: the data source the run covers, the
/// configuration the request named, the documented default of every member it left unset,
/// and — once the clock has finished the run — the ruleset recommended over the columns the
/// covered table declares.
///
/// The rows differ along which `DataSource` member carries the table, which optional
/// members the start names, the number of workers, the timeout (absent, the published lower
/// bound, a value well above it), the recommendation mode (absent and each modelled value),
/// the casing of the names, whether the start names a catalog id, and how many columns the
/// covered table declares (none, one, several).
#[tokio::test]
async fn a_started_run_is_read_back_through_get_data_quality_rule_recommendation_run() {
    let (router, engine) = emulator_with_engine();

    let cases: Vec<StartCase<'_>> = vec![
        StartCase {
            // The recorded case's start: a plain `GlueTable` and nothing optional.
            case: "a GlueTable and no optional member",
            database: "dq_db",
            table: "events",
            columns: &["id", "amount", "country"],
            data_source: glue_table_source("dq_db", "events", json!({})),
            extra: json!({}),
            expected_workers: f64::from(DEFAULT_DATA_QUALITY_RUN_WORKERS),
            expected_timeout: f64::from(DEFAULT_DATA_QUALITY_RUN_TIMEOUT_MINUTES),
            expected_mode: DEFAULT_RECOMMENDATION_MODE,
            expected_ruleset: expected_dqdl(&["id", "amount", "country"]),
        },
        StartCase {
            case: "every optional member named",
            database: "dq_db",
            table: "orders",
            columns: &["order_id", "total"],
            data_source: glue_table_source(
                "dq_db",
                "orders",
                json!({ "ConnectionName": "dq-connection",
                        "AdditionalOptions": { "pushDownPredicate": "day=1" } }),
            ),
            extra: json!({
                "CreatedRulesetName": "orders-ruleset",
                "NumberOfWorkers": 2,
                "Timeout": 10,
                "RecommendationMode": "ADVANCED",
                "DataQualitySecurityConfiguration": "dq-security",
                "AdditionalRunOptions": { "CustomLogGroupPrefix": "/custom/dq" },
            }),
            expected_workers: 2.0,
            expected_timeout: 10.0,
            expected_mode: "ADVANCED",
            expected_ruleset: expected_dqdl(&["order_id", "total"]),
        },
        StartCase {
            // The other member a `DataSource` can carry: "an Glue table for Data Quality
            // Operations", which addresses a table the same way.
            case: "a DataQualityGlueTable",
            database: "dq_db",
            table: "customers",
            columns: &["customer_id"],
            data_source: json!({
                "DataQualityGlueTable": {
                    "DatabaseName": "dq_db",
                    "TableName": "customers",
                    "PreProcessingQuery": "SELECT * FROM customers",
                }
            }),
            extra: json!({ "RecommendationMode": "BASIC" }),
            expected_workers: f64::from(DEFAULT_DATA_QUALITY_RUN_WORKERS),
            expected_timeout: f64::from(DEFAULT_DATA_QUALITY_RUN_TIMEOUT_MINUTES),
            expected_mode: "BASIC",
            expected_ruleset: expected_dqdl(&["customer_id"]),
        },
        StartCase {
            // A table stored without a storage descriptor declares no columns, so there is
            // nothing to recommend a rule over.
            case: "a table that declares no columns",
            database: "dq_db",
            table: "columnless",
            columns: &[],
            data_source: glue_table_source("dq_db", "columnless", json!({})),
            extra: json!({}),
            expected_workers: f64::from(DEFAULT_DATA_QUALITY_RUN_WORKERS),
            expected_timeout: f64::from(DEFAULT_DATA_QUALITY_RUN_TIMEOUT_MINUTES),
            expected_mode: DEFAULT_RECOMMENDATION_MODE,
            expected_ruleset: None,
        },
        StartCase {
            // Database and table names are folded to lowercase when stored, so a start that
            // spells them in another casing still addresses the stored table.
            case: "mixed-case names and an explicit catalog id",
            database: "DQ_DB_Mixed",
            table: "Daily_Sales",
            columns: &["Day", "Revenue"],
            data_source: glue_table_source(
                "DQ_DB_Mixed",
                "Daily_Sales",
                json!({ "CatalogId": ACCOUNT_ID }),
            ),
            extra: json!({}),
            expected_workers: f64::from(DEFAULT_DATA_QUALITY_RUN_WORKERS),
            expected_timeout: f64::from(DEFAULT_DATA_QUALITY_RUN_TIMEOUT_MINUTES),
            expected_mode: DEFAULT_RECOMMENDATION_MODE,
            expected_ruleset: expected_dqdl(&["Day", "Revenue"]),
        },
        StartCase {
            // `Timeout` has `range 1..`: the published lower bound.
            case: "the published lower timeout bound",
            database: "dq_db",
            table: "timeout_low",
            columns: &["a"],
            data_source: glue_table_source("dq_db", "timeout_low", json!({})),
            extra: json!({ "Timeout": 1, "NumberOfWorkers": 1 }),
            expected_workers: 1.0,
            expected_timeout: 1.0,
            expected_mode: DEFAULT_RECOMMENDATION_MODE,
            expected_ruleset: expected_dqdl(&["a"]),
        },
        StartCase {
            case: "a timeout well above the lower bound",
            database: "dq_db",
            table: "timeout_high",
            columns: &["a", "b", "c", "d"],
            data_source: glue_table_source("dq_db", "timeout_high", json!({})),
            extra: json!({ "Timeout": 525_600, "NumberOfWorkers": 100 }),
            expected_workers: 100.0,
            expected_timeout: 525_600.0,
            expected_mode: DEFAULT_RECOMMENDATION_MODE,
            expected_ruleset: expected_dqdl(&["a", "b", "c", "d"]),
        },
    ];

    for StartCase {
        case,
        database,
        table,
        columns,
        data_source,
        extra,
        expected_workers,
        expected_timeout,
        expected_mode,
        expected_ruleset,
    } in cases
    {
        store_table(&router, database, table, columns).await;
        let run_id = start_run(&router, &start_body(&data_source, extra.clone())).await;
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
            run.get("RunId").and_then(Value::as_str),
            Some(run_id.as_str()),
            "case {case:?}: the run is reported under the id the start minted, got {run}"
        );
        assert_eq!(
            run.get("Status").and_then(Value::as_str),
            Some(DATA_QUALITY_RUN_STATE_STARTING),
            "case {case:?}: a run whose owed transition is not due yet is still \
             {DATA_QUALITY_RUN_STATE_STARTING}, got {run}"
        );
        assert_echoed(
            case,
            &data_source,
            run.get("DataSource").unwrap_or(&Value::Null),
        );
        assert_eq!(
            run.get("Role").and_then(Value::as_str),
            Some(ROLE),
            "case {case:?}: the run reports the role the start named, got {run}"
        );
        assert_eq!(
            reported_number(&run, "NumberOfWorkers"),
            expected_workers,
            "case {case:?}: expected NumberOfWorkers {expected_workers}, got {run}"
        );
        assert_eq!(
            reported_number(&run, "Timeout"),
            expected_timeout,
            "case {case:?}: expected Timeout {expected_timeout}, got {run}"
        );
        assert_eq!(
            run.get("RecommendationMode").and_then(Value::as_str),
            Some(expected_mode),
            "case {case:?}: expected RecommendationMode {expected_mode:?}, got {run}"
        );
        assert_eq!(
            run.get("CreatedRulesetName").and_then(Value::as_str),
            extra.get("CreatedRulesetName").and_then(Value::as_str),
            "case {case:?}: the run reports the ruleset name the start named, got {run}"
        );
        assert_eq!(
            run.get("DataQualitySecurityConfiguration")
                .and_then(Value::as_str),
            extra
                .get("DataQualitySecurityConfiguration")
                .and_then(Value::as_str),
            "case {case:?}: the run reports the security configuration the start named, got {run}"
        );
        assert_eq!(
            run.get("AdditionalRunOptions"),
            extra.get("AdditionalRunOptions"),
            "case {case:?}: the run reports the additional run options the start named, got {run}"
        );
        assert!(
            run.get("CompletedOn").is_none() && run.get("RecommendedRuleset").is_none(),
            "case {case:?}: a run that has not finished has neither a CompletedOn nor a \
             recommended ruleset, got {run}"
        );
        assert!(
            run.get("ErrorString").is_none(),
            "case {case:?}: a run that has not failed carries no ErrorString, got {run}"
        );
        assert_eq!(
            reported_number(&run, "ExecutionTime"),
            0.0,
            "case {case:?}: a run still in flight has consumed no completed time, got {run}"
        );
        let started_on = reported_number(&run, "StartedOn");
        assert!(
            started_on > 0.0
                && (reported_number(&run, "LastModifiedOn") - started_on).abs() < f64::EPSILON,
            "case {case:?}: a run that has not moved reports one instant for its start and its \
             last modification, got {run}"
        );

        // A read reports the run, it never moves it: a second read of the same not-yet-due
        // run answers exactly what the first did.
        let read_again = get_run(&router, &run_id).await;
        assert_eq!(
            read_again, run,
            "case {case:?}: reading a run must not change what a later caller sees, got \
             {read_again} after {run}"
        );

        // Now let the clock's owed status fall due: the run finishes over the stored table.
        set_due_instant(&engine, already_past());
        let settled = get_run(&router, &run_id).await;
        assert_eq!(
            settled.get("Status").and_then(Value::as_str),
            Some(DATA_QUALITY_RUN_STATE_SUCCEEDED),
            "case {case:?}: a run over a stored table finishes \
             {DATA_QUALITY_RUN_STATE_SUCCEEDED}, got {settled}"
        );
        assert!(
            TERMINAL_DATA_QUALITY_RUN_STATES
                .contains(&settled["Status"].as_str().unwrap_or_default()),
            "case {case:?}: the settled status must be terminal, got {settled}"
        );
        assert_eq!(
            settled
                .get("RecommendedRuleset")
                .and_then(Value::as_str)
                .map(str::to_string),
            expected_ruleset,
            "case {case:?}: expected the recommended ruleset {expected_ruleset:?}, got {settled}"
        );
        assert!(
            settled.get("ErrorString").is_none(),
            "case {case:?}: a run that finished its analysis carries no ErrorString, got {settled}"
        );
        assert!(
            reported_number(&settled, "CompletedOn") > 0.0,
            "case {case:?}: a finished run reports a CompletedOn, got {settled}"
        );
        let read_settled_again = get_run(&router, &run_id).await;
        assert_eq!(
            read_settled_again, settled,
            "case {case:?}: a settled run reports the same instants on every later read, got \
             {read_settled_again} after {settled}"
        );
    }
}

/// A column name carrying a quote cannot end the DQDL string it sits in: the quote is
/// escaped, so the recommended ruleset stays one well-formed rule list. A table wide enough
/// to overrun the modelled ceiling on that member is recommended as many rules as fit.
#[tokio::test]
async fn a_recommended_ruleset_escapes_quotes_and_stays_inside_the_modelled_ceiling() {
    // (case, the columns the table declares, the DQDL they must yield)
    let cases: Vec<(&str, Vec<String>, Option<String>)> = vec![
        ("no column at all", vec![], None),
        (
            "one plain column",
            vec!["id".to_string()],
            Some("Rules = [\n    ColumnExists \"id\"\n]".to_string()),
        ),
        (
            "two columns",
            vec!["id".to_string(), "amount".to_string()],
            Some(
                "Rules = [\n    ColumnExists \"id\",\n    ColumnExists \"amount\"\n]".to_string(),
            ),
        ),
        (
            "a column name carrying a quote",
            vec!["odd\"name".to_string()],
            Some("Rules = [\n    ColumnExists \"odd\\\"name\"\n]".to_string()),
        ),
    ];
    for (case, columns, expected) in cases {
        let actual = recommended_ruleset(&columns);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the ruleset {expected:?}, got {actual:?}"
        );
    }

    // Wider than the member can carry: `DataQualityRulesetString` is `length 1..65536`.
    let wide: Vec<String> = (0..10_000).map(|index| format!("column_{index}")).collect();
    let ruleset = recommended_ruleset(&wide).unwrap_or_default();
    assert!(
        ruleset.len() <= MAX_DATA_QUALITY_RULESET_BYTES
            && ruleset.starts_with("Rules = [\n")
            && ruleset.ends_with("\n]"),
        "case {:?}: the ruleset must stay inside {MAX_DATA_QUALITY_RULESET_BYTES} bytes and stay a \
         rule list, got {} bytes ending {:?}",
        "a table wider than the member can carry",
        ruleset.len(),
        ruleset.chars().rev().take(20).collect::<String>()
    );
    assert!(
        ruleset.contains("    ColumnExists \"column_0\""),
        "case {:?}: the rules it does carry are the first columns the table declares, got {:?}",
        "a table wider than the member can carry",
        ruleset.chars().take(60).collect::<String>()
    );
}

// ---------------------------------------------------------------------------
// One run per data source at a time
// ---------------------------------------------------------------------------

/// A start for a data source whose run is in flight is refused with the operation's
/// modelled `ConflictException`; a start for any other data source is not; and once the
/// in-flight run has finished the same data source can be started again.
#[tokio::test]
async fn a_start_is_refused_while_that_data_source_has_a_run_in_flight() {
    let (router, engine) = emulator_with_engine();
    store_table(&router, "busy_db", "busy_table", &["a", "b"]).await;
    store_table(&router, "busy_db", "other_table", &["a"]).await;
    store_table(&router, "quiet_db", "busy_table", &["a"]).await;

    let first = start_run(
        &router,
        &start_body(&glue_table_source("busy_db", "busy_table", json!({})), json!({})),
    )
    .await;
    set_due_instant(&engine, far_future());

    // (case, the start that must be refused while `first` is in flight, the table name the
    // refusal must carry — the spelling that request sent)
    let refused: Vec<(&str, Value, &str, &str)> = vec![
        (
            "the same data source again",
            start_body(&glue_table_source("busy_db", "busy_table", json!({})), json!({})),
            "busy_db",
            "busy_table",
        ),
        (
            "the same data source in another casing",
            start_body(&glue_table_source("BUSY_DB", "Busy_Table", json!({})), json!({})),
            "BUSY_DB",
            "Busy_Table",
        ),
        (
            "the same data source with another configuration",
            start_body(
                &glue_table_source("busy_db", "busy_table", json!({ "CatalogId": ACCOUNT_ID })),
                json!({ "NumberOfWorkers": 9, "RecommendationMode": "ADVANCED" }),
            ),
            "busy_db",
            "busy_table",
        ),
        (
            "the same table through the data-quality table member",
            start_body(
                &json!({ "DataQualityGlueTable": {
                    "DatabaseName": "busy_db", "TableName": "busy_table" } }),
                json!({}),
            ),
            "busy_db",
            "busy_table",
        ),
    ];
    for (case, body, database, table) in refused {
        let (status, parsed) = call(&router, "StartDataQualityRuleRecommendationRun", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a second start for a data source with a run in flight is refused, \
             got {status} with {parsed}"
        );
        assert!(
            error_code(&parsed).ends_with("ConflictException"),
            "case {case:?}: expected ConflictException, got {}",
            error_code(&parsed)
        );
        let message = error_message(&parsed);
        assert!(
            message.contains(database) && message.contains(table),
            "case {case:?}: the refusal names the table as the request spelled it \
             ({database}.{table}), got {message:?}"
        );
    }
    assert_eq!(
        stored_runs(&engine).len(),
        1,
        "case {:?}: a refused start records no run, got {:?}",
        "the refusals persisted nothing",
        stored_runs(&engine)
    );

    // Another data source is a different set of recommendations, so its run is unaffected.
    for (case, database, table) in [
        ("another table of the same database", "busy_db", "other_table"),
        ("the same table name in another database", "quiet_db", "busy_table"),
    ] {
        let other = start_run(
            &router,
            &start_body(&glue_table_source(database, table, json!({})), json!({})),
        )
        .await;
        assert_ne!(
            other, first,
            "case {case:?}: each start mints its own run id, got {other} twice"
        );
    }

    // Once the clock has finished the in-flight run, the data source can be started again.
    set_due_instant(&engine, already_past());
    let again = start_run(
        &router,
        &start_body(&glue_table_source("busy_db", "busy_table", json!({})), json!({})),
    )
    .await;
    assert_ne!(
        again, first,
        "case {:?}: the restart is a new run, got the first run's id again",
        "after the first run finished"
    );
    let finished = get_run(&router, &first).await;
    assert_eq!(
        finished.get("Status").and_then(Value::as_str),
        Some(DATA_QUALITY_RUN_STATE_SUCCEEDED),
        "case {:?}: the finished run keeps its terminal status, got {finished}",
        "after the restart"
    );
    assert!(
        stored_runs(&engine).len() <= MAX_DATA_QUALITY_RULE_RECOMMENDATION_RUNS,
        "case {:?}: the store keeps at most {MAX_DATA_QUALITY_RULE_RECOMMENDATION_RUNS} runs, \
         got {}",
        "the retention bound",
        stored_runs(&engine).len()
    );
}

/// `CreatedRulesetName` is "a name for the ruleset" the run creates: it is recorded on the
/// run and reported with it, and it refuses nothing — the service models no rule here that a
/// second start naming the same ruleset breaks, and the ruleset surface this emulator holds
/// no record of yet is `CreateDataQualityRuleset`'s own ground.
#[tokio::test]
async fn a_created_ruleset_name_is_recorded_on_the_run_and_refuses_nothing() {
    let (router, engine) = emulator_with_engine();
    store_table(&router, "ruleset_db", "first_table", &["a"]).await;
    store_table(&router, "ruleset_db", "second_table", &["b"]).await;

    let shared = "shared-ruleset";
    // (case, the data source the start addresses, the ruleset name it names)
    let cases: Vec<(&str, &str, &str)> = vec![
        ("the first start", "first_table", shared),
        ("another data source, the same ruleset name", "second_table", shared),
        ("another data source, another ruleset name", "second_table", "another-ruleset"),
    ];
    let mut run_ids: Vec<String> = Vec::new();
    for (case, table, ruleset) in cases {
        // Each start addresses a data source with no run of its own in flight, so the only
        // question this row asks is whether the ruleset name refuses anything.
        set_due_instant(&engine, already_past());
        let run_id = start_run(
            &router,
            &start_body(
                &glue_table_source("ruleset_db", table, json!({})),
                json!({ "CreatedRulesetName": ruleset }),
            ),
        )
        .await;
        assert!(
            !run_ids.contains(&run_id),
            "case {case:?}: each start mints its own run id, got {run_id} twice"
        );
        set_due_instant(&engine, far_future());
        let run = get_run(&router, &run_id).await;
        assert_eq!(
            run.get("CreatedRulesetName").and_then(Value::as_str),
            Some(ruleset),
            "case {case:?}: the run reports the ruleset name the start named, got {run}"
        );
        run_ids.push(run_id);
    }
    assert_eq!(
        stored_runs(&engine).len(),
        run_ids.len(),
        "case {:?}: every start recorded its own run, got {:?}",
        "one run per start",
        stored_runs(&engine).len()
    );
}

// ---------------------------------------------------------------------------
// Idempotency
// ---------------------------------------------------------------------------

/// `ClientToken` is "used for idempotency … to avoid creating or starting multiple
/// instances of the same resource": a start repeating a token an earlier start already used
/// is refused as a duplicate and records no second run, whatever else it names, while a
/// start under a token of its own is a run of its own.
#[tokio::test]
async fn a_start_repeating_a_client_token_is_refused_as_a_duplicate() {
    let (router, engine) = emulator_with_engine();
    store_table(&router, "token_db", "token_table", &["a"]).await;
    store_table(&router, "token_db", "other_table", &["a"]).await;

    let token = "11111111-2222-3333-4444-555555555555";
    let body = start_body(
        &glue_table_source("token_db", "token_table", json!({})),
        json!({ "ClientToken": token }),
    );
    let first = start_run(&router, &body).await;

    // (case, a start that carries the token again)
    let duplicates: Vec<(&str, Value)> = vec![
        ("the same start again", body.clone()),
        (
            "the same token with another number of workers",
            start_body(
                &glue_table_source("token_db", "token_table", json!({})),
                json!({ "ClientToken": token, "NumberOfWorkers": 7 }),
            ),
        ),
        (
            "the same token with another data source",
            start_body(
                &glue_table_source("token_db", "other_table", json!({})),
                json!({ "ClientToken": token }),
            ),
        ),
        (
            "the same token naming a ruleset to create",
            start_body(
                &glue_table_source("token_db", "token_table", json!({})),
                json!({ "ClientToken": token, "CreatedRulesetName": "token-ruleset" }),
            ),
        ),
        ("the same token with another role", {
            let mut other = body.clone();
            other["Role"] = json!("arn:aws:iam::000000000000:role/another-role");
            other
        }),
    ];
    for (case, duplicate) in duplicates {
        let (status, parsed) =
            call(&router, "StartDataQualityRuleRecommendationRun", &duplicate).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a start repeating a spent token is refused, got {status} with {parsed}"
        );
        assert!(
            error_code(&parsed).ends_with("ConflictException"),
            "case {case:?}: expected ConflictException, got {}",
            error_code(&parsed)
        );
        assert_eq!(
            error_message(&parsed),
            format!("Idempotent start Data Quality run with token {token}"),
            "case {case:?}: expected the duplicate-start refusal naming the token, got {:?}",
            error_message(&parsed)
        );
    }
    assert_eq!(
        stored_runs(&engine).len(),
        1,
        "case {:?}: a refused start records no run, got {:?}",
        "the refusals persisted nothing",
        stored_runs(&engine)
    );

    // A token of its own over a data source of its own is a run of its own.
    let other = start_run(
        &router,
        &start_body(
            &glue_table_source("token_db", "other_table", json!({})),
            json!({ "ClientToken": "66666666-7777-8888-9999-000000000000" }),
        ),
    )
    .await;
    assert_ne!(
        other, first,
        "case {:?}: a start under another token is another run, got {other} twice",
        "another client token"
    );
}

// ---------------------------------------------------------------------------
// Cancelling a run
// ---------------------------------------------------------------------------

/// A cancel stops the run it addresses: a run in flight moves into `STOPPING` and the clock
/// owes it `STOPPED`; a run that has already finished keeps the status it finished in; and a
/// cancel of a run nothing minted is the operation's modelled miss.
#[tokio::test]
async fn a_cancel_stops_the_run_it_addresses() {
    let (router, engine) = emulator_with_engine();
    store_table(&router, "cancel_db", "in_flight", &["a"]).await;
    store_table(&router, "cancel_db", "already_done", &["b"]).await;

    // A run still in flight: the cancel moves it into STOPPING, and the clock owes it
    // STOPPED.
    let in_flight = start_run(
        &router,
        &start_body(&glue_table_source("cancel_db", "in_flight", json!({})), json!({})),
    )
    .await;
    set_due_instant(&engine, far_future());
    ok(
        &router,
        "CancelDataQualityRuleRecommendationRun",
        &json!({ "RunId": in_flight }),
    )
    .await;
    let stopping = get_run(&router, &in_flight).await;
    assert_eq!(
        stopping.get("Status").and_then(Value::as_str),
        Some(DATA_QUALITY_RUN_STATE_STOPPING),
        "case {:?}: a cancelled run is draining, got {stopping}",
        "a run in flight"
    );
    set_due_instant(&engine, already_past());
    let stopped = get_run(&router, &in_flight).await;
    assert_eq!(
        stopped.get("Status").and_then(Value::as_str),
        Some(DATA_QUALITY_RUN_STATE_STOPPED),
        "case {:?}: the clock owes a cancelled run its stop, got {stopped}",
        "a cancelled run once its stop is due"
    );
    assert!(
        stopped.get("RecommendedRuleset").is_none(),
        "case {:?}: a run that was stopped recommends no ruleset, got {stopped}",
        "a cancelled run once its stop is due"
    );

    // A run the clock has already finished: the cancel leaves it as it is.
    let finished = start_run(
        &router,
        &start_body(
            &glue_table_source("cancel_db", "already_done", json!({})),
            json!({}),
        ),
    )
    .await;
    set_due_instant(&engine, already_past());
    let before = get_run(&router, &finished).await;
    assert_eq!(
        before.get("Status").and_then(Value::as_str),
        Some(DATA_QUALITY_RUN_STATE_SUCCEEDED),
        "case {:?}: the run finished before the cancel, got {before}",
        "a run that already finished"
    );
    ok(
        &router,
        "CancelDataQualityRuleRecommendationRun",
        &json!({ "RunId": finished }),
    )
    .await;
    let after = get_run(&router, &finished).await;
    assert_eq!(
        after, before,
        "case {:?}: a cancel of a finished run changes nothing, got {after} after {before}",
        "a run that already finished"
    );

    // A run nothing minted.
    let (status, parsed) = call(
        &router,
        "CancelDataQualityRuleRecommendationRun",
        &json!({ "RunId": "ffffffffffffffffffffffffffffffff" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {:?}: a cancel of a run nothing minted is refused, got {status} with {parsed}",
        "a run id nothing minted"
    );
    assert!(
        error_code(&parsed).ends_with("EntityNotFoundException"),
        "case {:?}: expected EntityNotFoundException, got {}",
        "a run id nothing minted",
        error_code(&parsed)
    );
}

// ---------------------------------------------------------------------------
// The window the clock owes
// ---------------------------------------------------------------------------

/// The transition this emulator owes a run is due inside the window the project fixes, and
/// never at once: a reader straight after the start still sees the transient status.
#[tokio::test]
async fn the_owed_transition_is_due_inside_the_chosen_window() {
    let (lower, upper) = OWED_TRANSITION_WINDOW;
    assert!(
        DATA_QUALITY_RULE_RECOMMENDATION_RUN_SETTLES_AFTER >= lower
            && DATA_QUALITY_RULE_RECOMMENDATION_RUN_SETTLES_AFTER <= upper,
        "case {:?}: the owed transition must fall due in {lower:?}..={upper:?}, got {:?}",
        "the configured span",
        DATA_QUALITY_RULE_RECOMMENDATION_RUN_SETTLES_AFTER
    );

    let (router, engine) = emulator_with_engine();
    store_table(&router, "window_db", "window_table", &["a"]).await;
    let before = SystemTime::now();
    let run_id = start_run(
        &router,
        &start_body(&glue_table_source("window_db", "window_table", json!({})), json!({})),
    )
    .await;

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
        Some(DATA_QUALITY_RUN_STATE_STARTING),
        "case {:?}: a run read straight after the start is still in flight, got {reported}",
        "read straight after the start"
    );
}

/// The clock owes the transition, not the reader: a run left alone for longer than the span
/// is terminal on its first read afterwards.
#[tokio::test]
async fn the_wall_clock_settles_a_run_nobody_read() {
    let (router, _engine) = emulator_with_engine();
    store_table(&router, "clock_db", "clock_table", &["a"]).await;
    let run_id = start_run(
        &router,
        &start_body(&glue_table_source("clock_db", "clock_table", json!({})), json!({})),
    )
    .await;

    tokio::time::sleep(DATA_QUALITY_RULE_RECOMMENDATION_RUN_SETTLES_AFTER + Duration::from_millis(150))
        .await;

    let run = get_run(&router, &run_id).await;
    assert_eq!(
        run.get("Status").and_then(Value::as_str),
        Some(DATA_QUALITY_RUN_STATE_SUCCEEDED),
        "case {:?}: the wall clock alone makes the owed transition due, got {run}",
        "slept past the span"
    );
}

// ---------------------------------------------------------------------------
// Misses and bad input
// ---------------------------------------------------------------------------

/// A start whose data source the catalog holds nothing for is ACCEPTED — this operation
/// models no `EntityNotFoundException` — and the run it records fails when the clock settles
/// it, carrying the catalog's own miss.
#[tokio::test]
async fn a_run_over_a_data_source_the_catalog_does_not_hold_fails() {
    let (router, engine) = emulator_with_engine();
    store_table(&router, "miss_db", "miss_table", &["a"]).await;

    // (case, the data source, the text the failed run's ErrorString must carry)
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a database the catalog does not hold",
            glue_table_source("no_such_db", "miss_table", json!({})),
            "Database no_such_db not found.",
        ),
        (
            "a table the database does not hold",
            glue_table_source("miss_db", "no_such_table", json!({})),
            "Table no_such_table not found.",
        ),
        (
            "a catalog of another account",
            glue_table_source("miss_db", "miss_table", json!({ "CatalogId": "210987654321" })),
            "Database miss_db not found.",
        ),
    ];

    for (case, data_source, expected) in cases {
        let run_id = start_run(&router, &start_body(&data_source, json!({}))).await;
        set_due_instant(&engine, already_past());
        let run = get_run(&router, &run_id).await;
        assert_eq!(
            run.get("Status").and_then(Value::as_str),
            Some(DATA_QUALITY_RUN_STATE_FAILED),
            "case {case:?}: a run with nothing to analyse finishes \
             {DATA_QUALITY_RUN_STATE_FAILED}, got {run}"
        );
        assert_eq!(
            run.get("ErrorString").and_then(Value::as_str),
            Some(expected),
            "case {case:?}: expected the error string {expected:?}, got {run}"
        );
        assert!(
            run.get("RecommendedRuleset").is_none(),
            "case {case:?}: a run that analysed nothing recommends no ruleset, got {run}"
        );
    }
}

/// A read of a run nothing minted, and a start the service cannot read a data source out of,
/// are each refused — the read as the operation's modelled miss, the start as its modelled
/// bad input — and a value outside a modelled constraint is refused at the boundary.
#[tokio::test]
async fn a_request_that_addresses_nothing_is_refused() {
    let (router, _engine) = emulator_with_engine();
    store_table(&router, "bad_db", "bad_table", &["a"]).await;

    // (case, operation, body, the expected error code, the text the refusal must carry)
    let cases: Vec<(&str, &str, Value, &str, &str)> = vec![
        (
            "a run id nothing minted",
            "GetDataQualityRuleRecommendationRun",
            json!({ "RunId": "ffffffffffffffffffffffffffffffff" }),
            "EntityNotFoundException",
            "ffffffffffffffffffffffffffffffff",
        ),
        (
            "a data source naming neither kind of table",
            "StartDataQualityRuleRecommendationRun",
            start_body(&json!({}), json!({})),
            "InvalidInputException",
            "GlueTable",
        ),
    ];

    for (case, operation, body, expected_code, expected_text) in cases {
        let (status, parsed) = call(&router, operation, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: the request is refused with 400, got {status} with {parsed}"
        );
        assert!(
            error_code(&parsed).ends_with(expected_code),
            "case {case:?}: expected {expected_code}, got {}",
            error_code(&parsed)
        );
        assert!(
            error_message(&parsed).contains(expected_text),
            "case {case:?}: expected a refusal carrying {expected_text:?}, got {:?}",
            error_message(&parsed)
        );
    }

    // One past the published lower bound of `Timeout` (`range 1..`) is refused before the
    // handler runs, so no run is recorded for it.
    let (status, parsed) = call(
        &router,
        "StartDataQualityRuleRecommendationRun",
        &start_body(
            &glue_table_source("bad_db", "bad_table", json!({})),
            json!({ "Timeout": 0 }),
        ),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {:?}: a timeout below the modelled range is refused, got {status} with {parsed}",
        "one past the lower timeout bound"
    );
    assert_ne!(
        error_code(&parsed),
        "NotImplemented",
        "case {:?}: the refusal is a bad-request refusal, not a scaffold answer, got {parsed}",
        "one past the lower timeout bound"
    );
}
