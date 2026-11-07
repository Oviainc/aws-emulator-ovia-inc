// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what a terminal Athena query execution REPORTS about its output:
//! the result object its run wrote, the result set a read of that output reports, the row
//! count the statement wrote, the kind of statement it was, and the reuse and timing
//! members of its statistics.
//!
//! Driven over the wire (awsJson1_1, `x-amz-target: AmazonAthena.<Operation>`) through the
//! same router the emulator boots, so every assertion is about what an SDK client
//! observes.
//!
//! Each test states a rule and asserts it over a TABLE of inputs rather than one request:
//! the rows differ along what the rule generalises over — the class of statement
//! submitted, the form of the configured results location, whether the engine ran the
//! statement or reported it in error, and whether the statement read stored data.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_athena_amazonathena::state::{
    self, QUERY_EXECUTION_COMPLETION, QUERY_STATE_FAILED, QUERY_STATE_QUEUED, QUERY_STATE_RUNNING,
};
use smithy_server_runtime::MultiServiceRouter;

// The wire harness is shared by the suites of this test binary.
use crate::harness::{call, execution, router, start};

/// Where the queries in these tests write their results: a directory, written with the
/// trailing separator. Nothing is written to it.
const OUTPUT_LOCATION: &str = "s3://athena-results/outputs/";

/// The same directory written WITHOUT the trailing separator, so the rule that joins an
/// object under it is asserted on both forms of the input.
const OUTPUT_LOCATION_NO_SEPARATOR: &str = "s3://athena-results/outputs";

/// How long a test waits past [`QUERY_EXECUTION_COMPLETION`] for a started query to be
/// terminal: the span is owed from the submit, and this leaves room for the engine run and
/// the request walk around it.
const SETTLE_GRACE: Duration = Duration::from_millis(600);

/// A `StartQueryExecution` body for `sql` writing its results to `location`.
fn query_to(sql: &str, location: &str) -> Value {
    json!({
        "QueryString": sql,
        "ResultConfiguration": { "OutputLocation": location },
    })
}

/// A `StartQueryExecution` body for `sql`, in `database` so the statements of one test row
/// see each other's writes and another test's do not.
fn query_in(sql: &str, database: &str) -> Value {
    json!({
        "QueryString": sql,
        "QueryExecutionContext": { "Database": database },
        "ResultConfiguration": { "OutputLocation": OUTPUT_LOCATION },
    })
}

/// Wait out the span a started query's completion is due after, then return the execution
/// once it is terminal.
async fn settled(router: &MultiServiceRouter, id: &str) -> Value {
    ::tokio::time::sleep(QUERY_EXECUTION_COMPLETION + SETTLE_GRACE).await;
    for _ in 0..20 {
        let found = execution(router, id).await;
        let state = found["Status"]["State"].as_str().unwrap_or_default();
        if state != QUERY_STATE_QUEUED && state != QUERY_STATE_RUNNING {
            return found;
        }
        ::tokio::time::sleep(Duration::from_millis(200)).await;
    }
    panic!("the execution {id} never reached a terminal state");
}

/// One row rendered the way `GetQueryResults` renders it: a `Datum` per value.
fn row_of(values: &[&str]) -> Value {
    json!({
        "Data": values
            .iter()
            .map(|value| json!({ "VarCharValue": value }))
            .collect::<Vec<Value>>()
    })
}

/// Once a query has run, `ResultConfiguration.OutputLocation` names the result OBJECT the
/// run wrote — the configured directory, the execution's own id and the extension its
/// statement's results take — and not the directory it sits in. Until then the record
/// reports the configured directory, which is all there is to report.
#[tokio::test]
async fn a_terminal_query_reports_the_result_object_its_run_wrote() {
    let router = router();

    // (case, the SQL, the configured results location, the extension the object takes).
    // The rows differ along the class of statement submitted — rows or text output, run or
    // refused by the engine — and along whether the configured location was written with
    // the trailing separator.
    let cases: Vec<(&str, &str, &str, &str)> = vec![
        (
            "a projection",
            "SELECT 1 AS one",
            OUTPUT_LOCATION,
            state::RESULT_OBJECT_ROWS_SUFFIX,
        ),
        (
            "a definition statement",
            "CREATE TABLE located (id integer)",
            OUTPUT_LOCATION,
            state::RESULT_OBJECT_TEXT_SUFFIX,
        ),
        (
            "a write",
            "INSERT INTO located VALUES (1)",
            OUTPUT_LOCATION,
            state::RESULT_OBJECT_ROWS_SUFFIX,
        ),
        (
            "a utility statement",
            "EXPLAIN SELECT 1",
            OUTPUT_LOCATION,
            state::RESULT_OBJECT_TEXT_SUFFIX,
        ),
        (
            "a definition statement the engine reported in error",
            "CREATE TABLE located (id integer)",
            OUTPUT_LOCATION,
            state::RESULT_OBJECT_TEXT_SUFFIX,
        ),
        (
            "a projection the engine reported in error",
            "SELECT * FROM nowhere_at_all",
            OUTPUT_LOCATION,
            state::RESULT_OBJECT_ROWS_SUFFIX,
        ),
        (
            "a results directory written without its trailing separator",
            "SELECT 2 AS two",
            OUTPUT_LOCATION_NO_SEPARATOR,
            state::RESULT_OBJECT_ROWS_SUFFIX,
        ),
    ];

    let mut started: Vec<(&str, String, &str, &str)> = Vec::new();
    for (case, sql, location, suffix) in &cases {
        let id = start(&router, &query_to(sql, location)).await;
        // Read straight back, before the completion the clock owes: the query has not run
        // to a terminal state, so the configured directory is still what it reports.
        let found = execution(&router, &id).await;
        assert_eq!(
            found["ResultConfiguration"]["OutputLocation"].as_str(),
            Some(*location),
            "case {case:?}: a query that has not finished reports the configured \
             directory {location}, got {found:?}"
        );
        started.push((case, id, location, suffix));
    }

    for (case, id, location, suffix) in &started {
        let found = settled(&router, id).await;
        let expected = format!("{}/{id}{suffix}", location.trim_end_matches('/'));
        let reported = found["ResultConfiguration"]["OutputLocation"]
            .as_str()
            .map(str::to_string);
        assert_eq!(
            reported.as_deref(),
            Some(expected.as_str()),
            "case {case:?}: expected the result object {expected}, got {reported:?}"
        );
    }
}

/// What a row of the read-the-output table expects of `GetQueryResults`.
enum Expected {
    /// The output is reported: the row count the statement wrote, and the rows the read
    /// reports (the column-label row first, where the statement's results are rows).
    Served {
        /// The `UpdateCount` the read must report, `None` where it must report none.
        update_count: Option<i64>,
        /// Every row of the one page, in order.
        rows: Vec<Vec<&'static str>>,
    },
    /// There is no output to read, so the read is refused.
    Refused,
}

/// `GetQueryResults` reports the OUTPUT a terminal query execution's run wrote: the rows
/// of a statement whose results are rows, and the empty-but-present result set of one
/// whose result is the engine's text output — which a DDL or UTILITY statement wrote
/// whether its run reported the statement done or in error. A query that wrote no output
/// at all — one still running, one this emulator stopped, one whose rows were never
/// produced — is refused.
#[tokio::test]
async fn a_read_reports_the_output_of_every_query_that_wrote_one() {
    let router = router();
    let database = "outputs";

    // The rows differ along the class of statement and along whether the engine ran it:
    // a definition statement it ran and one it reported in error, a utility statement it
    // reported in error, a projection it ran and one it could not, and a statement this
    // emulator's own confinement stopped before the engine reported anything at all.
    let cases: Vec<(&str, &str, Expected)> = vec![
        (
            "a definition statement the engine ran",
            "CREATE TABLE readable (id integer)",
            Expected::Served {
                update_count: None,
                rows: Vec::new(),
            },
        ),
        (
            "a definition statement the engine reported in error",
            "CREATE TABLE readable (id integer)",
            Expected::Served {
                update_count: None,
                rows: Vec::new(),
            },
        ),
        (
            "a utility statement the engine reported in error",
            "EXPLAIN SELECT * FROM nowhere_at_all",
            Expected::Served {
                update_count: None,
                rows: Vec::new(),
            },
        ),
        (
            "a projection the engine ran",
            "SELECT 'north' AS region",
            Expected::Served {
                update_count: Some(0),
                rows: vec![vec!["region"], vec!["north"]],
            },
        ),
        (
            "a write the engine ran",
            "INSERT INTO readable VALUES (4)",
            Expected::Served {
                update_count: Some(1),
                rows: Vec::new(),
            },
        ),
        (
            "a projection whose rows were never produced",
            "SELECT * FROM nowhere_at_all",
            Expected::Refused,
        ),
        (
            "a statement this emulator stopped",
            "ATTACH DATABASE 'host.db' AS stolen",
            Expected::Refused,
        ),
    ];

    let mut started: Vec<(&str, String, &Expected)> = Vec::new();
    for (case, sql, expected) in &cases {
        let id = start(&router, &query_in(sql, database)).await;
        // A query that has not finished has written no output yet, whatever it will
        // write: the read is refused and the refusal names the state it is in.
        let (status, refused) =
            call(&router, "GetQueryResults", &json!({ "QueryExecutionId": id })).await;
        let message = refused["Message"].as_str().unwrap_or_default().to_string();
        assert_eq!(
            status, 400,
            "case {case:?}: a read of an unfinished query must answer 400, got {status} \
             with {refused:?}"
        );
        assert!(
            message.contains(QUERY_STATE_QUEUED) || message.contains(QUERY_STATE_RUNNING),
            "case {case:?}: the refusal must name the state the query is in, got \
             {message:?}"
        );
        started.push((case, id, expected));
    }

    for (case, id, expected) in &started {
        settled(&router, id).await;
        let (status, body) =
            call(&router, "GetQueryResults", &json!({ "QueryExecutionId": id })).await;
        match expected {
            Expected::Refused => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: a query that wrote no output must be refused, got \
                     {status} with {body:?}"
                );
                let kind = body["__type"].as_str().unwrap_or("<no __type>");
                assert!(
                    kind.ends_with("InvalidRequestException"),
                    "case {case:?}: expected InvalidRequestException, got {body:?}"
                );
                assert_eq!(
                    body["Message"].as_str(),
                    Some(state::query_did_not_succeed(QUERY_STATE_FAILED).as_str()),
                    "case {case:?}: unexpected message in {body:?}"
                );
            }
            Expected::Served { update_count, rows } => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: the output of a terminal query must be readable, got \
                     {status} with {body:?}"
                );
                // The collections are PRESENT, empty or not: an output with no rows is
                // not the same answer as no output at all.
                let reported = body["ResultSet"]["Rows"].as_array().cloned();
                let expected_rows: Vec<Value> =
                    rows.iter().map(|row| row_of(row)).collect::<Vec<Value>>();
                assert_eq!(
                    reported.as_deref(),
                    Some(expected_rows.as_slice()),
                    "case {case:?}: expected the rows {expected_rows:?}, got {body:?}"
                );
                assert!(
                    body["ResultSet"]["ResultSetMetadata"]["ColumnInfo"].is_array(),
                    "case {case:?}: the column metadata must be present, got {body:?}"
                );
                assert_eq!(
                    body["UpdateCount"].as_i64(),
                    *update_count,
                    "case {case:?}: expected UpdateCount {update_count:?}, got {body:?}"
                );
                assert_eq!(
                    body["NextToken"],
                    Value::Null,
                    "case {case:?}: one page holds the whole output, so no token follows \
                     it, got {body:?}"
                );
            }
        }
    }
}

/// `SubstatementType` is the KIND of statement that ran, read off the parse of the SQL
/// submitted — reported for a statement the engine ran and for one it reported in error
/// alike, because the kind of statement is a fact about the statement and not about its
/// outcome.
#[tokio::test]
async fn the_substatement_type_reported_is_the_kind_of_statement_submitted() {
    let router = router();
    let database = "kinds";

    // (case, the SQL, the SubstatementType it must report). The rows cover one statement
    // of each kind this service names, including kinds no recorded query used: a write, a
    // table defined from a query, a view, an update and a delete.
    let cases: Vec<(&str, &str, &str)> = vec![
        ("a projection", "SELECT 1 AS one", "SELECT"),
        (
            "a table definition",
            "CREATE TABLE kinds_table (id integer)",
            "CREATE_TABLE",
        ),
        (
            "a table defined from a query",
            "CREATE TABLE kinds_copy AS SELECT 1 AS id",
            "CREATE_TABLE_AS_SELECT",
        ),
        (
            "a view definition",
            "CREATE VIEW kinds_view AS SELECT 1 AS id",
            "CREATE_VIEW",
        ),
        ("a write", "INSERT INTO kinds_table VALUES (1)", "INSERT_INTO"),
        ("an update", "UPDATE kinds_table SET id = 2", "UPDATE"),
        ("a delete", "DELETE FROM kinds_table", "DELETE"),
        ("a database definition", "CREATE DATABASE kinds_db", "CREATE_DATABASE"),
        ("a database drop", "DROP DATABASE kinds_db", "DROP_DATABASE"),
        ("a table drop", "DROP TABLE kinds_table", "DROP_TABLE"),
        ("a view drop", "DROP VIEW kinds_view", "DROP_VIEW"),
        ("a listing of the tables", "SHOW TABLES", "SHOW_TABLES"),
        ("a listing of the databases", "SHOW DATABASES", "SHOW_DATABASES"),
        ("a plan", "EXPLAIN SELECT 1", "EXPLAIN"),
    ];

    let mut started: Vec<(&str, String, &str)> = Vec::new();
    for (case, sql, expected) in &cases {
        started.push((case, start(&router, &query_in(sql, database)).await, expected));
    }

    for (case, id, expected) in &started {
        let found = execution(&router, id).await;
        assert_eq!(
            found["SubstatementType"].as_str(),
            Some(*expected),
            "case {case:?}: expected SubstatementType {expected}, got {found:?}"
        );
    }
}

/// A query execution reports the result-reuse configuration it RAN under — the one its
/// submission carried, and reuse turned off where it carried none — and its statistics
/// report that no previous result was reused, with the service's own spans around the
/// engine's measured between the record's instants.
#[tokio::test]
async fn a_query_reports_the_result_reuse_it_ran_under_and_the_spans_it_took() {
    let router = router();

    // (case, the submission, the ResultReuseByAgeConfiguration it must report). The rows
    // differ along what the submission asked for: nothing at all, reuse off, reuse on
    // with a maximum age, and an empty reuse configuration.
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "a submission that asked for no reuse",
            query_to("SELECT 1 AS one", OUTPUT_LOCATION),
            json!({ "Enabled": false }),
        ),
        (
            "a submission that turned reuse off",
            json!({
                "QueryString": "SELECT 2 AS two",
                "ResultConfiguration": { "OutputLocation": OUTPUT_LOCATION },
                "ResultReuseConfiguration": {
                    "ResultReuseByAgeConfiguration": { "Enabled": false }
                },
            }),
            json!({ "Enabled": false }),
        ),
        (
            "a submission that turned reuse on with a maximum age",
            json!({
                "QueryString": "SELECT 3 AS three",
                "ResultConfiguration": { "OutputLocation": OUTPUT_LOCATION },
                "ResultReuseConfiguration": {
                    "ResultReuseByAgeConfiguration": { "Enabled": true, "MaxAgeInMinutes": 60 }
                },
            }),
            json!({ "Enabled": true, "MaxAgeInMinutes": 60 }),
        ),
        (
            "a submission that carried an empty reuse configuration",
            json!({
                "QueryString": "SELECT 4 AS four",
                "ResultConfiguration": { "OutputLocation": OUTPUT_LOCATION },
                "ResultReuseConfiguration": {},
            }),
            json!({ "Enabled": false }),
        ),
    ];

    let mut started: Vec<(&str, String, &Value)> = Vec::new();
    for (case, request, expected) in &cases {
        started.push((case, start(&router, request).await, expected));
    }

    for (case, id, expected) in &started {
        let found = settled(&router, id).await;
        assert_eq!(
            &found["ResultReuseConfiguration"]["ResultReuseByAgeConfiguration"],
            *expected,
            "case {case:?}: expected the reuse configuration {expected}, got {found:?}"
        );
        let statistics = &found["Statistics"];
        assert_eq!(
            statistics["ResultReuseInformation"]["ReusedPreviousResult"].as_bool(),
            Some(false),
            "case {case:?}: every query here is run, so no previous result was reused, \
             got {statistics:?}"
        );
        // The service's own spans are measurements between the record's instants, so they
        // are reported, non-negative, and no longer than the whole execution took.
        let total = statistics["TotalExecutionTimeInMillis"].as_i64();
        for member in [
            "ServicePreProcessingTimeInMillis",
            "ServiceProcessingTimeInMillis",
        ] {
            let span = statistics[member].as_i64();
            assert!(
                span.zip(total).is_some_and(|(span, total)| (0..=total).contains(&span)),
                "case {case:?}: expected 0 <= {member} <= {total:?}, got {statistics:?}"
            );
        }
    }
}

/// `DataScannedInBytes` is "the number of bytes in the data that was queried", so a
/// statement that read no stored data at all scanned nothing however many values it
/// computed, and one that read a table scanned the bytes the engine read for it.
#[tokio::test]
async fn only_a_statement_that_reads_stored_data_scans_bytes() {
    let router = router();
    let database = "scans";

    // The table the scanning rows read, written first so there is stored data to read.
    for sql in [
        "CREATE TABLE scanned (id integer, item varchar)",
        "INSERT INTO scanned VALUES (1, 'desk'), (2, 'lamp'), (3, 'chair')",
    ] {
        start(&router, &query_in(sql, database)).await;
    }

    // (case, the SQL, whether it must report bytes scanned). The rows differ along where
    // the values a statement answers with came from: a literal, an expression over
    // literals, every column of a stored table, one column of it, and an aggregate over
    // it.
    let cases: Vec<(&str, &str, bool)> = vec![
        ("a literal projection", "SELECT 1 AS one", false),
        (
            "an expression over literals",
            "SELECT 2 + 3 AS computed, length('abcde') AS width",
            false,
        ),
        ("a plan of a literal projection", "EXPLAIN SELECT 1", false),
        ("every column of a stored table", "SELECT * FROM scanned", true),
        ("one column of a stored table", "SELECT item FROM scanned", true),
        (
            "an aggregate over a stored table",
            "SELECT count(*) AS held FROM scanned",
            true,
        ),
    ];

    let mut started: Vec<(&str, String, bool)> = Vec::new();
    for (case, sql, scans) in &cases {
        started.push((case, start(&router, &query_in(sql, database)).await, *scans));
    }

    for (case, id, scans) in &started {
        let found = settled(&router, id).await;
        let scanned = found["Statistics"]["DataScannedInBytes"].as_i64();
        if *scans {
            assert!(
                scanned.is_some_and(|scanned| scanned > 0),
                "case {case:?}: a statement that read stored data must report the bytes \
                 it read, got {found:?}"
            );
        } else {
            assert_eq!(
                scanned,
                Some(0),
                "case {case:?}: a statement that read no stored data scanned nothing, got \
                 {found:?}"
            );
        }
    }
}
