// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHAT DECIDES an execution's outcome, driven over this
//! service's own wire protocol.
//!
//! The rule under test: an execution's terminal status, `error` and `cause` are
//! decided by the definition it runs and the input it was started with — a `Succeed`
//! or an `End`, a `Fail` state's own error, the definition's own `TimeoutSeconds` —
//! and by `StopExecution`. What this emulator's own interpreter spends on the way may
//! not decide one: the ceilings it keeps on a run are quotas on the WORK a definition
//! asks for (its state transitions, the bytes a state may carry, the patterns and
//! paths one visit may evaluate), so the same definition over the same input reaches
//! the same terminal status however long this process took to interpret it, and a run
//! that reaches its `Succeed` ends `SUCCEEDED` whatever the interpreter spent getting
//! there.
//!
//! The table spans the ways a run ENDS, with every row asking for interpretation work
//! on the way: a long loop that reaches a `Succeed`, the same loop reaching a `Fail`,
//! the same loop under a definition-level `TimeoutSeconds` (the one span that may cut a
//! run short), and a loop that never ends, which the state-transition quota stops — a
//! quota counted in transitions and not in seconds. The first row carries a document
//! and a `StringMatches` pattern whose match is tens of millions of character steps,
//! repeated on every turn: the case a wall-clock ceiling decides and a work quota does
//! not.
//!
//! The harness (`Emulator`, `create_machine`, `start_execution`, `describe_execution`,
//! `string`) is the one `state_machine_lifecycle` already builds; `tests/all.rs` is
//! this binary's crate root, so it is reached through `crate::`.

use std::time::Duration;

use serde_json::{json, Value};

use crate::state_machine_lifecycle::{
    create_machine, describe_execution, start_execution, string, Emulator,
    PAST_THE_EXECUTION_DELAY,
};

/// How many turns of its loop every row below runs: two transitions each, well inside
/// the state-transition quota, and enough turns that the pattern of the heavy row is
/// matched over and over.
const TURNS: u64 = 150;

/// The characters the document of the HEAVY row carries, and the length of the pattern
/// matched against it on every turn.
///
/// The product is what one match costs, and the row spends it `TURNS` times, so the
/// row's interpretation is tens of millions of character steps rather than the few
/// thousand a plain definition takes.
const HEAVY_DOCUMENT_CHARS: usize = 10_000;
const HEAVY_PATTERN_CHARS: usize = 60;

/// The characters the document of every other row carries: those rows are about which
/// terminal status the definition produces, so they carry a document rather than a
/// load.
const LIGHT_DOCUMENT_CHARS: usize = 400;

/// The seconds the row that declares one gives its whole execution.
const DEFINITION_TIMEOUT_SECONDS: u64 = 1;

/// The seconds that row's loop waits on each turn, which is far past the timeout above:
/// the `Wait` is what takes the run past the span the definition allows it.
const HOLD_SECONDS: u64 = 3_600;

/// The error and cause the `Fail` row's own state declares.
const FAIL_ERROR: &str = "Tenant.NotProvisioned";
const FAIL_CAUSE: &str = "the loop finished without provisioning the tenant";

/// Wording no cause may carry: this emulator's own interpretation ceiling used to end a
/// run on the clock, and a cause that says so is about the emulator rather than about
/// the definition.
const CLOCK_WORDING: [&str; 3] = ["interpreted for longer", "seconds this emulator", "budget"];

/// A `StringMatches` pattern of about `chars` characters that cannot match
/// [`document`]'s text, so matching it backtracks over the whole document: `*` followed
/// by a literal the text never carries.
fn pattern(chars: usize) -> String {
    format!("*{}zz", "ab".repeat(chars.saturating_sub(3) / 2))
}

/// The execution input: a turn counter the loop counts with, and a document of about
/// `chars` characters the loop carries through its template on every turn.
fn document(chars: usize) -> String {
    json!({ "n": 0, "blob": "ab".repeat(chars / 2) }).to_string()
}

/// The definition one row runs: turns of a loop that rewrites its document through a
/// payload template, counts with an intrinsic and matches [`pattern`] over the
/// document, and then reaches `ending`.
///
/// `waiting` is the seconds the loop waits on each turn, for the row whose own
/// `TimeoutSeconds` must be what ends it, and `timeout` is that `TimeoutSeconds`.
fn definition(ending: Value, waiting: Option<u64>, timeout: Option<u64>) -> String {
    let after_step = if waiting.is_some() { "Hold" } else { "Check" };
    let mut states = json!({
        "Step": {
            "Type": "Pass",
            "Parameters": {
                "n.$": "States.MathAdd($.n, 1)",
                "blob.$": "$.blob",
                "tag.$": "States.Format('turn {}', $.n)",
            },
            "Next": after_step,
        },
        "Check": {
            "Type": "Choice",
            "Choices": [
                // Evaluated on every turn and matching nothing: the work the row asks
                // the interpreter for.
                {
                    "Variable": "$.blob",
                    "StringMatches": pattern(HEAVY_PATTERN_CHARS),
                    "Next": "Ending",
                },
                { "Variable": "$.n", "NumericLessThan": TURNS, "Next": "Step" },
            ],
            "Default": "Ending",
        },
        "Ending": ending,
    });
    if let Some(seconds) = waiting {
        states["Hold"] = json!({ "Type": "Wait", "Seconds": seconds, "Next": "Check" });
    }
    let mut written = json!({ "StartAt": "Step", "States": states });
    if let Some(seconds) = timeout {
        written["TimeoutSeconds"] = json!(seconds);
    }
    written.to_string()
}

/// One row: the definition it runs, the input it is started with, and what the
/// definition itself decides the execution ends as.
struct OutcomeCase {
    case: &'static str,
    definition: String,
    input: String,
    status: &'static str,
    error: Option<&'static str>,
    /// Text the `cause` must carry, for a row whose definition names one.
    cause_names: Option<&'static str>,
    /// Whether the output must report the loop having run every turn, which is what
    /// tells a run that was DRIVEN to its `Succeed` from one abandoned on the way.
    counts_every_turn: bool,
}

fn cases() -> Vec<OutcomeCase> {
    vec![
        OutcomeCase {
            case: "a long loop over a large document, matching a pattern on every turn",
            definition: definition(json!({ "Type": "Succeed" }), None, None),
            input: document(HEAVY_DOCUMENT_CHARS),
            status: "SUCCEEDED",
            error: None,
            cause_names: None,
            counts_every_turn: true,
        },
        OutcomeCase {
            case: "the same loop reaching a Fail state",
            definition: definition(
                json!({ "Type": "Fail", "Error": FAIL_ERROR, "Cause": FAIL_CAUSE }),
                None,
                None,
            ),
            input: document(LIGHT_DOCUMENT_CHARS),
            status: "FAILED",
            error: Some(FAIL_ERROR),
            cause_names: Some(FAIL_CAUSE),
            counts_every_turn: false,
        },
        OutcomeCase {
            // The ONE span that may cut a run short: "the maximum number of seconds an
            // execution of the state machine can run".
            case: "the same loop whose own TimeoutSeconds runs out first",
            definition: definition(
                json!({ "Type": "Succeed" }),
                Some(HOLD_SECONDS),
                Some(DEFINITION_TIMEOUT_SECONDS),
            ),
            input: document(LIGHT_DOCUMENT_CHARS),
            status: "TIMED_OUT",
            error: None,
            cause_names: None,
            counts_every_turn: false,
        },
        OutcomeCase {
            // The emulator's remaining ceiling on a run, and a quota in TRANSITIONS: a
            // loop that goes round more times than one execution may is stopped by how
            // much work it asked for, and the cause says so, naming no span of time.
            case: "a loop whose exit its own data never reaches, which the state-transition \
                   quota stops",
            definition: json!({
                "StartAt": "Step",
                "States": {
                    "Step": {
                        "Type": "Pass",
                        "Parameters": { "n.$": "States.MathAdd($.n, 1)" },
                        "Next": "Check",
                    },
                    // A turn count no run of this loop reaches inside the quota, so the
                    // definition declares a terminal state the loop never gets to.
                    "Check": {
                        "Type": "Choice",
                        "Choices": [
                            { "Variable": "$.n", "NumericLessThan": 1_000_000, "Next": "Step" },
                        ],
                        "Default": "Ending",
                    },
                    "Ending": { "Type": "Succeed" },
                },
            })
            .to_string(),
            input: document(LIGHT_DOCUMENT_CHARS),
            status: "FAILED",
            error: Some("States.Runtime"),
            cause_names: Some("state transitions"),
            counts_every_turn: false,
        },
    ]
}

/// Long enough that every row's outcome has come due: the settle delay, plus the one
/// row whose own `TimeoutSeconds` its outcome is owed after.
fn past_every_outcome() -> Duration {
    PAST_THE_EXECUTION_DELAY
        .saturating_add(Duration::from_secs(DEFINITION_TIMEOUT_SECONDS))
        .saturating_add(Duration::from_millis(300))
}

/// The `cause` a description carries, or the empty string where it carries none.
fn cause(described: &Value) -> String {
    described
        .get("cause")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Every row ends as its own definition decides, however much interpretation the run
/// asked for on the way.
#[tokio::test]
async fn an_executions_outcome_is_its_definitions_and_never_a_clocks() {
    let emulator = Emulator::new();
    let rows = cases();

    // Every row is started first and read afterwards, so one wait covers the outcomes
    // the clock owed all of them.
    let mut started = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        let name = format!("outcome-{index}");
        let machine = create_machine(&emulator, &name, &row.definition).await;
        started.push(start_execution(&emulator, &machine, "only", &row.input).await);
    }
    std::thread::sleep(past_every_outcome());

    for (row, execution) in rows.iter().zip(started) {
        let case = row.case;
        let described = describe_execution(&emulator, &execution).await;
        assert_eq!(
            (
                described.get("status").and_then(Value::as_str),
                described.get("error").and_then(Value::as_str),
            ),
            (Some(row.status), row.error),
            "case {case:?}: expected the definition's own outcome {} {:?}, got {described:?}",
            row.status,
            row.error
        );
        let reported = cause(&described);
        if let Some(names) = row.cause_names {
            assert!(
                reported.contains(names),
                "case {case:?}: expected the cause to name {names:?}, got {reported:?}"
            );
        }
        for wording in CLOCK_WORDING {
            assert!(
                !reported.contains(wording),
                "case {case:?}: expected no cause about this emulator's own clock, got \
                 {reported:?} carrying {wording:?}"
            );
        }
        if row.counts_every_turn {
            let output: Value = serde_json::from_str(&string(&described, "output"))
                .unwrap_or_else(|error| {
                    panic!("case {case:?}: expected JSON output, got {error}")
                });
            assert_eq!(
                output.get("n").and_then(Value::as_u64),
                Some(TURNS),
                "case {case:?}: expected the loop to have run every one of its {TURNS} turns, \
                 got {:?}",
                output.get("n")
            );
            assert_eq!(
                output.get("tag").and_then(Value::as_str),
                Some(format!("turn {}", TURNS - 1).as_str()),
                "case {case:?}: expected the last turn's own tag, got {:?}",
                output.get("tag")
            );
        }
    }
}

/// The SAME definition and input reach the same terminal status and the same output on
/// every run, which is what makes a run's outcome the definition's rather than the
/// host's: a ceiling measured on the clock would let the machine's own speed and load
/// decide between them.
#[tokio::test]
async fn the_same_definition_and_input_end_the_same_way_every_run() {
    let emulator = Emulator::new();
    let written = definition(json!({ "Type": "Succeed" }), None, None);
    let input = document(LIGHT_DOCUMENT_CHARS);
    let machine = create_machine(&emulator, "repeatable", &written).await;

    let mut started = Vec::new();
    for run in 0..3 {
        started.push(start_execution(&emulator, &machine, &format!("run-{run}"), &input).await);
    }
    std::thread::sleep(past_every_outcome());

    let mut outcomes = Vec::new();
    for execution in &started {
        let described = describe_execution(&emulator, execution).await;
        outcomes.push((
            described
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            described
                .get("output")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            cause(&described),
        ));
    }
    let first = outcomes.first().cloned().expect("three runs were started");
    assert_eq!(
        first.0, "SUCCEEDED",
        "case \"repeated runs\": expected the definition's own outcome, got {outcomes:?}"
    );
    for (index, outcome) in outcomes.iter().enumerate() {
        assert_eq!(
            outcome, &first,
            "case \"repeated runs\": expected run {index} to end exactly as the first did \
             ({first:?}), got {outcome:?}"
        );
    }
}
