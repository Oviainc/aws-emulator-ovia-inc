// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what an execution reports when it reaches a state it cannot
//! TRANSITION OUT OF, driven over this service's own wire protocol (`awsJson1_0`,
//! `POST /` with `X-Amz-Target: AWSStepFunctions.<Operation>`) through the production
//! `MultiServiceRouter`.
//!
//! The rule under test: a state the walk reached and could not leave fails the execution
//! with `States.Runtime` and ONE cause — the state's own name, the id of the history
//! event it was entered at, and then the sentence that the transition out of the state
//! failed because the state points to no next state. The same cause is what the
//! `ExecutionFailed` history event reports, so `DescribeExecution` and
//! `GetExecutionHistory` cannot disagree about why the run stopped.
//!
//! The rows differ along what the rule generalises over: how the state came to have
//! nowhere to go (a `Choice` no rule matched and that declares no `Default`, with one
//! rule and with several, over a value that is absent and over one that is present but
//! compares unequal), the query language the rules are written in (JSONPath and JSONata),
//! and the state's POSITION in the definition — which is what makes the event id in the
//! cause differ from row to row. A nested case has its own test: a `Choice` inside a
//! `Map`'s `ItemProcessor` names ITSELF, not the `Map` around it.
//!
//! The last test is about what tells this failure APART: a `Choice` whose condition path
//! cannot be resolved fails with the same error name and a DIFFERENT cause, so a client
//! classifying a `States.Runtime` failure by its cause text can tell an unmatched
//! `Choice` from a condition that could not be evaluated.
//!
//! The harness (`Emulator`, `create_machine`, `start_execution`, `describe_execution`,
//! `string`) is the one `state_machine_lifecycle` builds and the history reader is
//! `execution_history`'s; `tests/all.rs` is this binary's crate root, so both are reached
//! through `crate::<suite>`.

use serde_json::{json, Value};

use service_com_amazonaws_sfn_awsstepfunctions::state::asl::RUNTIME_ERROR;

use crate::execution_history::history;
use crate::state_machine_lifecycle::{
    create_machine, describe_execution, start_execution, string, Emulator,
    PAST_THE_EXECUTION_DELAY,
};

/// The name every row's stuck state carries.
const STUCK: &str = "Route";

/// The diagnosis the service states for a state it could not transition out of, which is
/// fixed text: only the state's name and the event id around it vary.
const NO_NEXT_STATE: &str =
    "Failed to transition out of the state. The state does not point to a next state.";

/// One row: the stuck state, how many `Pass` states run before it, and the input.
struct Row {
    case: &'static str,
    /// The stuck state itself, which every row reaches last.
    stuck: Value,
    /// How many `Pass` states the execution walks before it, so the event id the cause
    /// names differs from row to row.
    before: usize,
    input: &'static str,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one JSONPath rule, over a member the input does not carry",
            stuck: json!({
                "Type": "Choice",
                "Choices": [{ "Variable": "$.kind", "IsPresent": true, "Next": "Done" }],
            }),
            before: 0,
            input: r#"{"other":1}"#,
        },
        Row {
            case: "three JSONPath rules, none of which matches",
            stuck: json!({
                "Type": "Choice",
                "Choices": [
                    { "Variable": "$.kind", "StringEquals": "alpha", "Next": "Done" },
                    { "Variable": "$.kind", "StringEquals": "beta", "Next": "Done" },
                    { "Variable": "$.count", "NumericGreaterThan": 10, "Next": "Done" },
                ],
            }),
            before: 1,
            input: r#"{"kind":"gamma","count":1}"#,
        },
        Row {
            case: "a rule over a value that is present and compares unequal",
            stuck: json!({
                "Type": "Choice",
                "Choices": [{ "Variable": "$.ready", "BooleanEquals": true, "Next": "Done" }],
            }),
            before: 2,
            input: r#"{"ready":false}"#,
        },
        Row {
            case: "a JSONata Condition that evaluates to false",
            stuck: json!({
                "Type": "Choice",
                "QueryLanguage": "JSONata",
                "Choices": [{ "Condition": "{% $states.input.kind = 'alpha' %}", "Next": "Done" }],
            }),
            before: 0,
            input: r#"{"kind":"omega"}"#,
        },
    ]
}

/// The definition one row runs: `before` `Pass` states, then the stuck one, and the
/// `Done` state its rules name — a state no state reaches is not a definition this
/// service accepts.
fn definition(row: &Row) -> String {
    let mut states = serde_json::Map::new();
    let mut start_at = STUCK.to_string();
    for index in 0..row.before {
        let name = format!("Step{index}");
        let next = if index + 1 == row.before {
            STUCK.to_string()
        } else {
            format!("Step{}", index + 1)
        };
        if index == 0 {
            start_at = name.clone();
        }
        states.insert(name, json!({ "Type": "Pass", "Next": next }));
    }
    states.insert(STUCK.to_string(), row.stuck.clone());
    states.insert("Done".to_string(), json!({ "Type": "Pass", "End": true }));
    json!({ "StartAt": start_at, "States": Value::Object(states) }).to_string()
}

/// The cause the service words for the state `name`, entered at event `entered`, that it
/// could not transition out of.
fn expected_cause(name: &str, entered: i64) -> String {
    format!(
        "An error occurred while executing the state '{name}' (entered at the event id \
         #{entered}). {NO_NEXT_STATE}"
    )
}

/// The id of the one `ChoiceStateEntered` event naming `name`.
fn entered_event_id(events: &[Value], case: &str, name: &str) -> i64 {
    let matching: Vec<&Value> = events
        .iter()
        .filter(|event| {
            event.get("type").and_then(Value::as_str) == Some("ChoiceStateEntered")
                && event
                    .get("stateEnteredEventDetails")
                    .and_then(|details| details.get("name"))
                    .and_then(Value::as_str)
                    == Some(name)
        })
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "case {case:?}: expected exactly one ChoiceStateEntered for {name}, got {events:?}"
    );
    matching[0]
        .get("id")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("case {case:?}: expected an id on {:?}", matching[0]))
}

/// The `cause` the `ExecutionFailed` event of a history reports.
fn execution_failed_cause(events: &[Value], case: &str) -> String {
    let failed = events
        .iter()
        .find(|event| event.get("type").and_then(Value::as_str) == Some("ExecutionFailed"))
        .unwrap_or_else(|| panic!("case {case:?}: expected an ExecutionFailed event in {events:?}"));
    failed
        .get("executionFailedEventDetails")
        .and_then(|details| details.get("cause"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A state the walk cannot leave fails the execution with `States.Runtime`, and the cause
/// names the state, the event it was entered at and the transition that failed — the same
/// cause in `DescribeExecution` and in the `ExecutionFailed` history event.
#[tokio::test]
async fn a_state_with_nowhere_to_go_names_itself_and_the_failed_transition() {
    let emulator = Emulator::new();
    let rows = rows();

    // Every row is started first and read afterwards, so one wait covers every outcome
    // the clock owed.
    let mut started = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        let machine = create_machine(&emulator, &format!("stuck-{index}"), &definition(row)).await;
        started.push(start_execution(&emulator, &machine, "only", row.input).await);
    }
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    for (row, execution) in rows.iter().zip(started) {
        let case = row.case;
        let described = describe_execution(&emulator, &execution).await;
        assert_eq!(
            (
                described.get("status").and_then(Value::as_str),
                described.get("error").and_then(Value::as_str),
            ),
            (Some("FAILED"), Some(RUNTIME_ERROR)),
            "case {case:?}: expected FAILED with {RUNTIME_ERROR}, got {described:?}"
        );
        let events = history(&emulator, &execution, json!({})).await;
        let entered = entered_event_id(&events, case, STUCK);
        // The id is the one the history really holds for this state, and the state's
        // position in the definition is what makes it differ row by row.
        let expected_entered = 2 + 2 * i64::try_from(row.before).expect("the count fits");
        assert_eq!(
            entered, expected_entered,
            "case {case:?}: expected the state to be entered at event #{expected_entered}, got \
             #{entered} in {events:?}"
        );
        let expected = expected_cause(STUCK, entered);
        let cause = string(&described, "cause");
        assert_eq!(
            cause, expected,
            "case {case:?}: expected the cause {expected:?}, got {cause:?}"
        );
        let reported = execution_failed_cause(&events, case);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the ExecutionFailed event to report the same cause, got \
             {reported:?}"
        );
    }
}

/// The state the cause names is the state that got stuck, including one inside a `Map`'s
/// `ItemProcessor`: the enclosing `Map` state does not take the diagnosis over.
#[tokio::test]
async fn a_nested_stuck_state_names_itself_rather_than_the_map_around_it() {
    let emulator = Emulator::new();
    let definition = json!({
        "StartAt": "Fan",
        "States": {
            "Fan": {
                "Type": "Map",
                "ItemsPath": "$.items",
                "ItemProcessor": {
                    "StartAt": STUCK,
                    "States": {
                        STUCK: {
                            "Type": "Choice",
                            "Choices": [{
                                "Variable": "$.n",
                                "NumericEquals": 99,
                                "Next": "Done",
                            }],
                        },
                        "Done": { "Type": "Pass", "End": true },
                    },
                },
                "End": true,
            },
        },
    })
    .to_string();
    let case = "a Choice state inside an ItemProcessor";
    let machine = create_machine(&emulator, "stuck-nested", &definition).await;
    let execution = start_execution(&emulator, &machine, "only", r#"{"items":[{"n":1}]}"#).await;
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    let described = describe_execution(&emulator, &execution).await;
    assert_eq!(
        (
            described.get("status").and_then(Value::as_str),
            described.get("error").and_then(Value::as_str),
        ),
        (Some("FAILED"), Some(RUNTIME_ERROR)),
        "case {case:?}: expected FAILED with {RUNTIME_ERROR}, got {described:?}"
    );
    let events = history(&emulator, &execution, json!({})).await;
    let entered = entered_event_id(&events, case, STUCK);
    let expected = expected_cause(STUCK, entered);
    let cause = string(&described, "cause");
    assert_eq!(
        cause, expected,
        "case {case:?}: expected the cause {expected:?}, got {cause:?}"
    );
    assert!(
        !cause.contains("'Fan'"),
        "case {case:?}: expected the cause to name the stuck state alone, got {cause:?}"
    );
}

/// The two ways a `Choice` state can fail are told apart by their CAUSE: a rule nothing
/// matched reports the failed transition, and a rule whose condition path resolves to
/// nothing reports the condition — both under `States.Runtime`, so the cause is what a
/// client classifies on.
#[tokio::test]
async fn an_unmatched_choice_and_an_unresolvable_condition_report_different_causes() {
    let emulator = Emulator::new();
    let definition = json!({
        "StartAt": STUCK,
        "States": {
            STUCK: {
                "Type": "Choice",
                "Choices": [{ "Variable": "$.kind", "StringEquals": "alpha", "Next": "Done" }],
            },
            "Done": { "Type": "Pass", "End": true },
        },
    })
    .to_string();
    let machine = create_machine(&emulator, "stuck-vs-path", &definition).await;
    // The SAME definition, run twice: an input the rule can be evaluated over and does
    // not match, and one the rule's path cannot be resolved against at all.
    let unmatched = start_execution(&emulator, &machine, "unmatched", r#"{"kind":"omega"}"#).await;
    let unresolved = start_execution(&emulator, &machine, "unresolved", r#"{"other":1}"#).await;
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    let mut causes = Vec::new();
    for (case, execution) in [
        ("a rule nothing matched", &unmatched),
        ("a condition path that resolves to nothing", &unresolved),
    ] {
        let described = describe_execution(&emulator, execution).await;
        assert_eq!(
            described.get("error").and_then(Value::as_str),
            Some(RUNTIME_ERROR),
            "case {case:?}: expected {RUNTIME_ERROR}, got {described:?}"
        );
        causes.push((case, string(&described, "cause")));
    }

    let (unmatched_case, unmatched_cause) = &causes[0];
    assert!(
        unmatched_cause.contains(NO_NEXT_STATE) && unmatched_cause.contains(STUCK),
        "case {unmatched_case:?}: expected the cause to name the stuck state and the failed \
         transition, got {unmatched_cause:?}"
    );
    let (unresolved_case, unresolved_cause) = &causes[1];
    assert!(
        !unresolved_cause.contains(NO_NEXT_STATE),
        "case {unresolved_case:?}: expected a cause of its own rather than the failed \
         transition, got {unresolved_cause:?}"
    );
    assert_ne!(
        unmatched_cause, unresolved_cause,
        "the two failures of one Choice state must be distinguishable by their cause"
    );
}
