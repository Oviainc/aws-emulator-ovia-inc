// SPDX-License-Identifier: CC0-1.0

//! What a caller's `WaitTimeSeconds` decides, driven over the wire through the
//! shared harness in [`super::emulator`]: every call below is a real `POST /`
//! with `X-Amz-Target: RedshiftData.<Operation>`, so what is asserted is the
//! emulator's HTTP behaviour and not a handler's return value.
//!
//! The rule under test: `WaitTimeSeconds` bounds the RESPONSE and never the
//! execution — "If the SQL statement does not complete within the specified
//! time, the response returns the current status" — so a statement reaches the
//! terminal status its execution decided whatever its caller asked to wait for,
//! a caller whose wait runs out first is answered with the status the statement
//! has reached by then, and a caller whose wait outlasts the service's own
//! execution bound is answered with the verdict itself.
//!
//! It is asserted over a TABLE rather than one case: the rows differ along the
//! patience the submit asks for (none at all, the shortest the model admits, the
//! longest it admits) and along how long the statement itself runs (one the
//! engine finishes at once, one that runs until the engine's own bound stops
//! it).

use serde_json::json;

use service_com_amazonaws_redshiftdata_redshiftdata::state::MAX_ENGINE_DEADLINE;

use super::emulator::{id_of, number, wait_for_terminal, Emulator, UNBOUNDED_SQL, WAIT_SECONDS};

/// The shortest wait the model admits (`WaitTimeSeconds`, range 1..30).
const SHORTEST_MODELLED_WAIT_SECONDS: i32 = 1;

/// A statement the engine runs to completion as soon as it picks it up.
const QUICK_SQL: &str = "SELECT 1";

/// What the engine decides for a row's statement, whatever its caller waited
/// for.
#[derive(Clone, Copy, Debug)]
enum Verdict {
    /// The statement ran to completion.
    Finished,
    /// One of the engine's execution bounds stopped it, so it failed carrying
    /// the engine's own interruption message.
    Interrupted,
}

/// One row of the table: a statement, the patience its submit asks for, the
/// statuses that submit's own response may report, and the verdict the engine
/// must reach for it.
struct Row {
    label: &'static str,
    sql: &'static str,
    /// The `WaitTimeSeconds` the submit carries, or `None` for a submit that
    /// names no wait at all.
    wait: Option<i32>,
    /// The statuses the submit's own response may report. A submit that names
    /// no wait reports the status the statement has at that instant; a wait
    /// SHORTER than the service's whole execution bound reports whatever the
    /// statement has reached by the time it expires, which is the non-terminal
    /// status or — if a bound of the engine stopped the statement inside that
    /// wait — the verdict; only a wait that covers the service's execution
    /// bound is sure to carry the verdict.
    answered: &'static [&'static str],
    verdict: Verdict,
}

/// A caller's `WaitTimeSeconds` bounds its own response and nothing else: the
/// statement still reaches the verdict its execution produced, and a later read
/// reports it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_callers_wait_bounds_its_own_response_and_not_the_execution() {
    let emulator = Emulator::new();

    // The rows that ask for a long wait come last, so the executions of the
    // rows before them are already in flight while those requests wait rather
    // than being started one after another.
    let rows = [
        Row {
            label: "no wait at all, for a statement the engine's bound stops",
            sql: UNBOUNDED_SQL,
            wait: None,
            // Nothing waited for this execution, and the execution runs until a
            // bound of the engine stops it, so the response reports the status
            // the statement has at that instant.
            answered: &["SUBMITTED"],
            verdict: Verdict::Interrupted,
        },
        Row {
            label: "the shortest wait the model admits, for a statement the engine's bound stops",
            sql: UNBOUNDED_SQL,
            wait: Some(SHORTEST_MODELLED_WAIT_SECONDS),
            // This wait does not cover the service's execution bound, so what
            // it reports is the status reached by the time it expires: the
            // non-terminal one, or the verdict if one of the engine's bounds
            // happened to stop the statement inside the wait. Which one it is
            // must not depend on the machine, so both are admitted here and the
            // verdict itself is pinned by the read below.
            answered: &["SUBMITTED", "FAILED"],
            verdict: Verdict::Interrupted,
        },
        Row {
            label: "no wait at all, for a statement that finishes at once",
            sql: QUICK_SQL,
            wait: None,
            // Nobody waited, so whether this statement's execution has already
            // reported when the response is written is the engine's business.
            answered: &["SUBMITTED", "FINISHED"],
            verdict: Verdict::Finished,
        },
        Row {
            label: "the shortest wait the model admits, for a statement that finishes at once",
            sql: QUICK_SQL,
            wait: Some(SHORTEST_MODELLED_WAIT_SECONDS),
            answered: &["SUBMITTED", "FINISHED"],
            verdict: Verdict::Finished,
        },
        Row {
            label: "a wait that outlasts the service's own execution bound",
            sql: UNBOUNDED_SQL,
            wait: Some(WAIT_SECONDS),
            // This wait covers the engine's bound, so the verdict is in this
            // very response.
            answered: &["FAILED"],
            verdict: Verdict::Interrupted,
        },
    ];

    let mut submitted: Vec<(&Row, String)> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        // Each row runs on a database of its own: the submissions of ONE
        // database run one at a time, so rows sharing one would queue behind
        // each other instead of running side by side.
        let mut request = json!({
            "ClusterIdentifier": "my-cluster",
            "Database": format!("dev{index}"),
            "DbUser": "awsuser",
            "Sql": row.sql,
        });
        if let Some(wait) = row.wait {
            request["WaitTimeSeconds"] = json!(wait);
        }
        let response = emulator.ok("ExecuteStatement", &request).await;
        let answered = response["Status"].as_str().unwrap_or_default().to_string();
        assert!(
            row.answered.contains(&answered.as_str()),
            "case {}: a submit asking to wait {:?} must be answered with one of {:?}, got \
             {response}",
            row.label,
            row.wait,
            row.answered
        );
        submitted.push((row, id_of(&response)));
    }

    for (row, id) in submitted {
        let described = wait_for_terminal(&emulator, row.label, &id).await;
        assert_eq!(
            described["QueryString"], row.sql,
            "case {}: the record read back must be this row's own statement, got {described}",
            row.label
        );
        match row.verdict {
            Verdict::Finished => {
                assert_eq!(
                    described["Status"], "FINISHED",
                    "case {}: a statement the engine completed must report FINISHED, got \
                     {described}",
                    row.label
                );
                assert!(
                    described["Error"].is_null(),
                    "case {}: a statement that ran reports no error, got {described}",
                    row.label
                );
            }
            Verdict::Interrupted => {
                assert_eq!(
                    described["Status"], "FAILED",
                    "case {}: a statement the engine's bound stopped must report FAILED, got \
                     {described}",
                    row.label
                );
                let error = described["Error"].as_str().unwrap_or_default();
                assert!(
                    error.to_ascii_lowercase().contains("interrupt"),
                    "case {}: the engine's own interruption must say why it stopped, got \
                     {described}",
                    row.label
                );
                // `Duration` is the engine's measured run, in nanoseconds: what
                // ended this statement is the service's own execution bound, so
                // its run is that bound and not the caller's patience (with the
                // same span again as slack, for the gap a loaded machine leaves
                // between two of the engine's progress callbacks).
                let duration = number(&described, "Duration");
                assert!(
                    duration <= bounded_run_nanos(),
                    "case {}: the span reported must be the engine's bounded run, got \
                     {duration} against {}: {described}",
                    row.label,
                    bounded_run_nanos()
                );
            }
        }
        assert!(
            number(&described, "Duration") > 0,
            "case {}: a statement that reached a verdict reports how long its execution took, \
             got {described}",
            row.label
        );
    }

    // And the service is still serving every one of those statements: a caller
    // that stopped waiting left nothing behind.
    let id = emulator.run(QUICK_SQL).await;
    let described = emulator.ok("DescribeStatement", &json!({"Id": id})).await;
    assert_eq!(
        described["Status"], "FINISHED",
        "a statement submitted after the rows above must still run, got {described}"
    );
}

/// The longest run, in nanoseconds, a statement the service's own execution
/// bound stopped may report: that bound, and the same span again as slack for
/// the gap a loaded machine leaves between two of the engine's progress
/// callbacks.
fn bounded_run_nanos() -> i64 {
    i64::try_from(
        MAX_ENGINE_DEADLINE
            .saturating_add(MAX_ENGINE_DEADLINE)
            .as_nanos(),
    )
    .unwrap_or(i64::MAX)
}
