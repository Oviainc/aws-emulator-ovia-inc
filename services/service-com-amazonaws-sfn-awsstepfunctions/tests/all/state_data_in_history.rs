// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the DATA a state's lifecycle events carry, driven over this
//! service's own wire protocol (`awsJson1_0`, `POST /` with
//! `X-Amz-Target: AWSStepFunctions.<Operation>`) through the production
//! `MultiServiceRouter`.
//!
//! The rule under test: every `<Type>StateEntered` event of an execution's history
//! carries the JSON data the state was entered WITH, and every `<Type>StateExited`
//! event carries the JSON data that state EMITTED — its post-processing result, with
//! the state's own result-side data flow (`ResultSelector`, then `ResultPath`, then
//! `OutputPath`) already applied — each beside the `*Details` member that says the
//! data is included. `includeExecutionData: false` leaves both out, exactly as it
//! leaves the execution-level data out.
//!
//! The rows differ along what the rule generalises over: the TYPE of the state (a
//! `Pass`, a `Choice`, a `Map`, a `Parallel` and a `Task`, which are the types that
//! record a pair of lifecycle events), the data-flow members the state declares
//! (none, an `OutputPath` that narrows the document, a `Parameters` template, and the
//! `InputPath`/`Parameters`/`ResultSelector`/`ResultPath`/`OutputPath` chain applied
//! in the documented order) and the SHAPE of what the state emitted (a document, a
//! scalar and an array).
//!
//! The harness (`Emulator`, `create_machine`, `string`) is the one
//! `state_machine_lifecycle` builds and the history reader is `execution_history`'s;
//! `tests/all.rs` is this binary's crate root, so each is reached through
//! `crate::<suite>`.

use serde_json::{json, Value};

use crate::execution_history::history;
use crate::state_machine_lifecycle::{
    create_machine, string, Emulator, PAST_THE_EXECUTION_DELAY, REGION,
};

/// One row of the table: the case, the definition, the state whose lifecycle events
/// are read, and the data that state was entered with and emitted.
type DataRow = (&'static str, Value, &'static str, Value, Value);

/// The execution input every row is started with: two members, so a row that narrows
/// the document with a path emits something different from the whole of it.
fn execution_input() -> Value {
    json!({ "n": 1, "extra": "x" })
}

/// Start one execution of `definition` and return its history once the clock has made
/// the outcome due.
async fn ran(emulator: &Emulator, name: &str, definition: &Value, input: &Value) -> Vec<Value> {
    let machine = create_machine(emulator, name, &definition.to_string()).await;
    let started = emulator
        .ok(
            "StartExecution",
            &json!({
                "stateMachineArn": machine,
                "name": "only",
                "input": input.to_string()
            }),
        )
        .await;
    let execution = string(&started, "executionArn");
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);
    let described = emulator
        .ok("DescribeExecution", &json!({ "executionArn": execution }))
        .await;
    assert_eq!(
        described.get("status").and_then(Value::as_str),
        Some("SUCCEEDED"),
        "case {name:?}: expected the execution to succeed, got {described:?}"
    );
    history(emulator, &execution, json!({})).await
}

/// The one event of `event_type` in `events`, which must be there exactly once.
fn only<'a>(events: &'a [Value], case: &str, event_type: &str) -> &'a Value {
    let found: Vec<&Value> = events
        .iter()
        .filter(|event| event.get("type").and_then(Value::as_str) == Some(event_type))
        .collect();
    assert_eq!(
        found.len(),
        1,
        "case {case:?}: expected exactly one {event_type}, got {events:?}"
    );
    found[0]
}

/// The JSON data `member` of the event's detail holds, parsed.
fn data(event: &Value, details: &str, member: &str, case: &str) -> Value {
    let carried = event
        .get(details)
        .and_then(|held| held.get(member))
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a {member:?} in the {details} of {event:?}")
        });
    serde_json::from_str(carried)
        .unwrap_or_else(|error| panic!("case {case:?}: expected JSON {member:?}, got {error}"))
}

/// A state's lifecycle events report the data it was entered with and the data it
/// emitted.
#[tokio::test]
async fn a_states_lifecycle_events_carry_the_data_it_was_entered_with_and_emitted() {
    let rows: [DataRow; 5] = [
        (
            // No data-flow member at all: the state emits its own `Result`.
            "a Pass state that declares a result",
            json!({
                "StartAt": "Only",
                "States": {
                    "Only": { "Type": "Pass", "Result": { "ok": true }, "End": true }
                }
            }),
            "Pass",
            execution_input(),
            json!({ "ok": true }),
        ),
        (
            // The whole documented chain, in the order the stages apply: `InputPath`
            // narrows the input, `Parameters` builds the request, `ResultSelector`
            // reshapes the result, `ResultPath` places it and `OutputPath` selects
            // what the state emits.
            "a Task state whose result the result-side stages reshaped",
            json!({
                "StartAt": "Only",
                "States": {
                    "Only": {
                        "Type": "Task",
                        "Resource": "arn:aws:states:::aws-sdk:sfn:listStateMachines",
                        "InputPath": "$.n",
                        "Parameters": { "MaxResults": 1 },
                        "ResultSelector": { "count": 1 },
                        "ResultPath": "$.result",
                        "OutputPath": "$.result",
                        "End": true
                    }
                }
            }),
            "Task",
            execution_input(),
            json!({ "count": 1 }),
        ),
        (
            // A state that emits a SCALAR, selected out of its own input.
            "a Choice state whose OutputPath narrows the document",
            json!({
                "StartAt": "Only",
                "States": {
                    "Only": {
                        "Type": "Choice",
                        "OutputPath": "$.n",
                        "Choices": [{ "Variable": "$.n", "NumericEquals": 1, "Next": "Done" }],
                        "Default": "Done"
                    },
                    "Done": { "Type": "Pass", "End": true }
                }
            }),
            "Choice",
            execution_input(),
            json!(1),
        ),
        (
            // A state that emits an ARRAY: one element per item it iterated.
            "a Map state whose items each produced a document",
            json!({
                "StartAt": "Only",
                "States": {
                    "Only": {
                        "Type": "Map",
                        "ItemsPath": "$.items",
                        "ItemProcessor": {
                            "StartAt": "Each",
                            "States": {
                                "Each": {
                                    "Type": "Pass",
                                    "Parameters": { "v.$": "$.x" },
                                    "End": true
                                }
                            }
                        },
                        "End": true
                    }
                }
            }),
            "Map",
            json!({ "items": [{ "x": 1 }, { "x": 2 }] }),
            json!([{ "v": 1 }, { "v": 2 }]),
        ),
        (
            // And one that emits an array of its branches' results, in branch order.
            "a Parallel state whose branches each produced a result",
            json!({
                "StartAt": "Only",
                "States": {
                    "Only": {
                        "Type": "Parallel",
                        "Branches": [
                            {
                                "StartAt": "Left",
                                "States": {
                                    "Left": { "Type": "Pass", "Result": "l", "End": true }
                                }
                            },
                            {
                                "StartAt": "Right",
                                "States": {
                                    "Right": { "Type": "Pass", "Result": "r", "End": true }
                                }
                            }
                        ],
                        "End": true
                    }
                }
            }),
            "Parallel",
            execution_input(),
            json!(["l", "r"]),
        ),
    ];

    let emulator = Emulator::new();
    for (index, (case, definition, state_type, entered, exited)) in rows.into_iter().enumerate() {
        let events = ran(
            &emulator,
            &format!("state-data-{index}"),
            &definition,
            &entered,
        )
        .await;
        let entered_event = only(&events, case, &format!("{state_type}StateEntered"));
        assert_eq!(
            data(entered_event, "stateEnteredEventDetails", "input", case),
            entered,
            "case {case:?}: expected the data the state was entered with, got {entered_event:?}"
        );
        assert_eq!(
            entered_event
                .pointer("/stateEnteredEventDetails/inputDetails/truncated"),
            Some(&json!(false)),
            "case {case:?}: expected the input to be reported as included, got {entered_event:?}"
        );
        let exited_event = only(&events, case, &format!("{state_type}StateExited"));
        assert_eq!(
            data(exited_event, "stateExitedEventDetails", "output", case),
            exited,
            "case {case:?}: expected the data the state emitted, got {exited_event:?}"
        );
        assert_eq!(
            exited_event.pointer("/stateExitedEventDetails/outputDetails/truncated"),
            Some(&json!(false)),
            "case {case:?}: expected the output to be reported as included, got {exited_event:?}"
        );
        // The data a state emitted is the next one's input, which is the other side of
        // the same rule: the execution's own output is the last state's.
        let name = exited_event
            .pointer("/stateExitedEventDetails/name")
            .and_then(Value::as_str);
        assert_eq!(
            name,
            Some("Only"),
            "case {case:?}: expected the state's own name beside its data, got {exited_event:?}"
        );
    }
}

/// A state whose work FAILED and whose `Catch` answered for it leaves on its exited
/// event the output the catcher placed: "when a state reports an error and it matches
/// a catcher, the state's output is the error output".
#[tokio::test]
async fn a_caught_states_exited_event_carries_the_error_output_it_left_with() {
    let emulator = Emulator::new();
    let absent = format!("arn:aws:states:{REGION}:000000000000:stateMachine:no-such-machine");
    let definition = json!({
        "StartAt": "Work",
        "States": {
            "Work": {
                "Type": "Task",
                "Resource": "arn:aws:states:::aws-sdk:sfn:describeStateMachine",
                "Parameters": { "StateMachineArn": absent },
                "ResultPath": "$.result",
                "Catch": [{
                    "ErrorEquals": ["States.ALL"],
                    "ResultPath": "$.failure",
                    "Next": "Handled"
                }],
                "End": true
            },
            "Handled": { "Type": "Pass", "End": true }
        }
    });
    let case = "a task whose catcher answered for its failure";
    let events = ran(&emulator, "state-data-caught", &definition, &execution_input()).await;
    let exited = only(&events, case, "TaskStateExited");
    let output = data(exited, "stateExitedEventDetails", "output", case);
    // The catcher placed the `{Error, Cause}` document at its own `ResultPath`, so the
    // state left with the input it was entered with plus that document.
    assert_eq!(
        output.pointer("/n"),
        Some(&json!(1)),
        "case {case:?}: expected the state's own input under the error output, got {output}"
    );
    let error = output
        .pointer("/failure/Error")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected the caught error in {output}"));
    assert!(
        error.starts_with("StepFunctions."),
        "case {case:?}: expected the invoked service's own error name, got {error:?}"
    );
}

/// "You can select whether execution data (input or output of a history event) is
/// returned": asked for without it, a state's lifecycle events name the state and
/// carry neither its input nor its output.
#[tokio::test]
async fn a_history_asked_for_without_execution_data_carries_no_state_data() {
    let emulator = Emulator::new();
    let definition = json!({
        "StartAt": "Only",
        "States": { "Only": { "Type": "Pass", "Result": { "ok": true }, "End": true } }
    });
    let machine = create_machine(&emulator, "state-data-withheld", &definition.to_string()).await;
    let started = emulator
        .ok(
            "StartExecution",
            &json!({ "stateMachineArn": machine, "name": "only", "input": "{}" }),
        )
        .await;
    let execution = string(&started, "executionArn");
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);
    let events = history(
        &emulator,
        &execution,
        json!({ "includeExecutionData": false }),
    )
    .await;
    let case = "a history asked for without execution data";
    for (event_type, details, member) in [
        ("PassStateEntered", "stateEnteredEventDetails", "input"),
        ("PassStateExited", "stateExitedEventDetails", "output"),
    ] {
        let event = only(&events, case, event_type);
        let held = event
            .get(details)
            .unwrap_or_else(|| panic!("case {case:?}: expected {details} on {event:?}"));
        assert_eq!(
            held.get("name").and_then(Value::as_str),
            Some("Only"),
            "case {case:?}: expected {details} to name the state, got {event:?}"
        );
        assert!(
            held.get(member).is_none_or(Value::is_null),
            "case {case:?}: expected no {member} without execution data, got {event:?}"
        );
    }
}
