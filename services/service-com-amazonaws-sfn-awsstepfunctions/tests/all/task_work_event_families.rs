// SPDX-License-Identifier: CC0-1.0

//! Which FAMILY of history events a `Task` state's own work is recorded under, driven
//! over both services' own wire protocols through the production `MultiServiceRouter`
//! with Step Functions and Lambda mounted on ONE engine — so the function really runs
//! and every event read back is one the run wrote.
//!
//! RULE: `HistoryEventType` names the lifecycle of a `Task` state's work twice over, and
//! which family one state's work is recorded under is decided by WHAT its `Resource`
//! names rather than by what the work turned out to be. A `Resource` that is the ARN of
//! a Lambda FUNCTION makes the task's unit of work one invocation of that function, so
//! its hand-over, start and outcome are `LambdaFunctionScheduled`,
//! `LambdaFunctionStarted` and `LambdaFunctionSucceeded`/`LambdaFunctionFailed`, with
//! the scheduled event naming the work by that function's own ARN and carrying the input
//! the invocation was handed; every other `Resource` names a service INTEGRATION, whose
//! unit of work is one API call, recorded under `TaskScheduled`, `TaskStarted` and
//! `TaskSucceeded`/`TaskFailed`, each naming the work by the kind of resource and the
//! action. Both families record the start of the work as its own event, so an attempt
//! that reached its work reports three events inside the state's own entered/exited
//! pair.
//!
//! RULE: `taskScheduledEventDetails.parameters` is "the JSON data passed to the resource
//! referenced in a task state" and is REQUIRED, with no `inputDetails` companion to say
//! a document was withheld — unlike every `input` and `output` member — so a read that
//! asked for no execution data still reports it, while those optional payload members go.
//!
//! RULE: resolving what the work is handed is the INTERPRETER's own step, taken before
//! anything is handed over: a `Parameters` path that cannot be resolved against the
//! state's input means the work was never submitted, so the state records no lifecycle
//! event for it at all and the execution fails on the state it was entered at.
//!
//! The rows differ along what the rule generalises over: the form of the `Resource` (a
//! bare function ARN and the optimized `arn:aws:states:::lambda:invoke` integration, plus
//! a non-Lambda integration), the outcome of the work (the function returned, the
//! function raised), and whether the work was handed over at all.
//!
//! The harness (`Chain`, `given_function_of`, `ran`, `sfn`) is the one
//! `lambda_task_control_flow` builds for the same two services; `tests/all.rs` is this
//! binary's crate root, so it is reached through `crate::<suite>`.

use serde_json::{json, Value};

use crate::lambda_function_timeout::event_types;
use crate::lambda_task_control_flow::Chain;
use crate::state_machine_lifecycle::string;

/// The handler every row invokes: it returns what the event asked it to carry, and
/// raises when the event asks for it, so the task's outcome is the run's own.
const HANDLER_SOURCE: &str = r#"
class OrderRejected(Exception):
    pass


def handler(event, context):
    if event.get("mode") == "raise":
        raise OrderRejected("order %s cannot be processed" % event.get("orderId"))
    return {"orderId": event.get("orderId"), "handled": True}
"#;

/// The `errorType` of the error document that handler's exception produces, which is the
/// name the failed unit of work reports.
const RAISED_ERROR_TYPE: &str = "OrderRejected";

/// One row: a `Resource` form, the input the state is handed, and the events its work
/// must write.
struct Row {
    case: &'static str,
    /// Built from the function's ARN, so each row names the same function its own way.
    state: fn(&str) -> Value,
    input: Value,
    /// The status the execution must reach.
    status: &'static str,
    /// The family's own hand-over, start and outcome events, in the order the work
    /// writes them. Empty for work that was never handed over.
    lifecycle: &'static [&'static str],
    /// The detail member the hand-over event carries, and what it must name the work by.
    scheduled: Option<(&'static str, &'static str, &'static str)>,
    /// The error the execution must report, where it failed.
    error: Option<&'static str>,
}

/// A `Task` state whose `Resource` is the function's ARN: the legacy form, whose unit of
/// work is one invocation of that function.
fn function_arn_task(function: &str) -> Value {
    json!({
        "Type": "Task",
        "Resource": function,
        "End": true,
    })
}

/// The same work asked for through the optimized integration, which names the function
/// in its `Parameters` instead.
fn optimized_task(function: &str) -> Value {
    json!({
        "Type": "Task",
        "Resource": "arn:aws:states:::lambda:invoke",
        "Parameters": { "FunctionName": function, "Payload.$": "$" },
        "End": true,
    })
}

/// A `Parameters` whose path resolves to nothing, so the interpreter fails the execution
/// before any work is handed over.
fn unresolvable_task(function: &str) -> Value {
    json!({
        "Type": "Task",
        "Resource": "arn:aws:states:::lambda:invoke",
        "Parameters": { "FunctionName": function, "Payload.$": "$.absent" },
        "End": true,
    })
}

/// A `Resource` naming an integration of ANOTHER service, which records the `Task*`
/// family exactly as the optimized Lambda one does.
fn other_integration_task(_function: &str) -> Value {
    json!({
        "Type": "Task",
        "Resource": "arn:aws:states:::sqs:sendMessage",
        "Parameters": { "QueueUrl": "https://sqs.us-west-2.amazonaws.com/000000000000/absent", "MessageBody": "m" },
        "End": true,
    })
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a function ARN whose invocation returns",
            state: function_arn_task,
            input: json!({ "orderId": "o-1" }),
            status: "SUCCEEDED",
            lifecycle: &[
                "LambdaFunctionScheduled",
                "LambdaFunctionStarted",
                "LambdaFunctionSucceeded",
            ],
            scheduled: Some(("lambdaFunctionScheduledEventDetails", "resource", "")),
            error: None,
        },
        Row {
            case: "a function ARN whose invocation raises",
            state: function_arn_task,
            input: json!({ "orderId": "o-2", "mode": "raise" }),
            status: "FAILED",
            lifecycle: &[
                "LambdaFunctionScheduled",
                "LambdaFunctionStarted",
                "LambdaFunctionFailed",
            ],
            scheduled: Some(("lambdaFunctionScheduledEventDetails", "resource", "")),
            error: Some(RAISED_ERROR_TYPE),
        },
        Row {
            case: "the optimized integration on the same function, which returns",
            state: optimized_task,
            input: json!({ "orderId": "o-3" }),
            status: "SUCCEEDED",
            lifecycle: &["TaskScheduled", "TaskStarted", "TaskSucceeded"],
            scheduled: Some(("taskScheduledEventDetails", "resourceType", "lambda")),
            error: None,
        },
        Row {
            case: "the optimized integration whose function raises",
            state: optimized_task,
            input: json!({ "orderId": "o-4", "mode": "raise" }),
            status: "FAILED",
            lifecycle: &["TaskScheduled", "TaskStarted", "TaskFailed"],
            scheduled: Some(("taskScheduledEventDetails", "resourceType", "lambda")),
            error: Some(RAISED_ERROR_TYPE),
        },
        Row {
            case: "an integration of another service, whose call the service refuses",
            state: other_integration_task,
            input: json!({ "orderId": "o-5" }),
            status: "FAILED",
            lifecycle: &["TaskScheduled", "TaskStarted", "TaskFailed"],
            scheduled: Some(("taskScheduledEventDetails", "resourceType", "sqs")),
            error: None,
        },
        Row {
            case: "a Parameters path that resolves to nothing, so nothing was handed over",
            state: unresolvable_task,
            input: json!({ "orderId": "o-6" }),
            status: "FAILED",
            lifecycle: &[],
            scheduled: None,
            error: Some("States.Runtime"),
        },
    ]
}

/// The positions of every event of type `event_type`, in the order they happened.
fn positions(events: &[Value], event_type: &str) -> Vec<usize> {
    events
        .iter()
        .enumerate()
        .filter(|(_, event)| event.get("type").and_then(Value::as_str) == Some(event_type))
        .map(|(position, _)| position)
        .collect()
}

/// The one event of type `event_type`, where the history holds exactly one.
///
/// `pub(crate)`, not private: `tests/all.rs` is this binary's crate root, so the sibling
/// suite that reads the same history through all three services reaches this rather than
/// carrying a second copy of it, and so does `numbered` below.
pub(crate) fn only<'a>(events: &'a [Value], event_type: &str, case: &str) -> &'a Value {
    let found = positions(events, event_type);
    assert_eq!(
        found.len(),
        1,
        "case {case:?}: expected exactly one {event_type}, got {:?}",
        event_types(events)
    );
    &events[found[0]]
}

/// The detail `member` of the one event of type `event_type`, for the same reason
/// [`only`] is reachable.
pub(crate) fn detail(events: &[Value], event_type: &str, member: &str, case: &str) -> Value {
    let event = only(events, event_type, case);
    event
        .get(member)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected {member} on {event}"))
}

/// The events of one execution, read with the members `extra` carries.
async fn history(chain: &Chain, case: &str, execution: &str, extra: Value) -> Vec<Value> {
    let mut request = json!({ "executionArn": execution });
    let members = request.as_object_mut().expect("the request is an object");
    for (member, value) in extra.as_object().expect("the extras are an object") {
        members.insert(member.clone(), value.clone());
    }
    let answered = chain.sfn(case, "GetExecutionHistory", &request).await;
    answered
        .get("events")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected events in {answered}"))
}

/// The family of events a `Task` state's work writes is the one its `Resource` names,
/// and every event of that family names the work the way that family names it.
#[tokio::test]
async fn the_resource_decides_the_family_of_events_the_work_writes() {
    let chain = Chain::new();

    for (index, row) in rows().into_iter().enumerate() {
        let Row {
            case,
            state,
            input,
            status,
            lifecycle,
            scheduled,
            error,
        } = row;
        let function = chain
            .given_function_of(&format!("families-{index}"), HANDLER_SOURCE, None)
            .await;
        let definition = json!({
            "StartAt": "Work",
            "States": { "Work": state(&function) },
        });
        let described = chain
            .ran(case, &format!("families-{index}"), &definition, &input)
            .await;
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some(status),
            "case {case:?}: expected the execution to end {status}, got {described}"
        );
        if let Some(error) = error {
            assert_eq!(
                described.get("error").and_then(Value::as_str),
                Some(error),
                "case {case:?}: expected the execution to report {error}, got {described}"
            );
        }

        let execution = string(&described, "executionArn");
        let events = history(&chain, case, &execution, json!({})).await;
        let present = event_types(&events);

        // Exactly the family's own events, each once, in the order the work writes
        // them — and none of the OTHER family's, whatever this work turned out to be.
        for event_type in [
            "TaskScheduled",
            "TaskStarted",
            "TaskSucceeded",
            "TaskFailed",
            "TaskTimedOut",
            "LambdaFunctionScheduled",
            "LambdaFunctionStarted",
            "LambdaFunctionSucceeded",
            "LambdaFunctionFailed",
            "LambdaFunctionTimedOut",
        ] {
            let expected = usize::from(lifecycle.contains(&event_type));
            let counted = positions(&events, event_type).len();
            assert_eq!(
                counted, expected,
                "case {case:?}: expected {expected} {event_type} events, got {present:?}"
            );
        }
        let order: Vec<usize> = lifecycle
            .iter()
            .map(|event_type| {
                *positions(&events, event_type)
                    .first()
                    .unwrap_or_else(|| panic!("case {case:?}: expected a {event_type}"))
            })
            .collect();
        assert!(
            order.windows(2).all(|pair| pair[0] < pair[1]),
            "case {case:?}: expected the work's events in order, got {present:?}"
        );
        // Each of them sits inside the pair the state was entered with, after the state
        // was entered.
        let entered = *positions(&events, "TaskStateEntered")
            .first()
            .unwrap_or_else(|| panic!("case {case:?}: expected a TaskStateEntered in {present:?}"));
        assert!(
            order.iter().all(|position| *position > entered),
            "case {case:?}: expected the work's events after the state was entered, got \
             {present:?}"
        );

        // The hand-over names the work the way its own family names it: the function's
        // ARN alone for an invocation the `Resource` named, the kind of resource and the
        // action for an integration's call.
        if let Some((member, naming, expected)) = scheduled {
            let event = only(&events, lifecycle[0], case);
            let detail = event
                .get(member)
                .unwrap_or_else(|| panic!("case {case:?}: expected {member} on {event}"));
            let expected = if expected.is_empty() {
                function.clone()
            } else {
                expected.to_string()
            };
            assert_eq!(
                detail.get(naming).and_then(Value::as_str),
                Some(expected.as_str()),
                "case {case:?}: expected the hand-over to name the work {expected:?}, got {detail}"
            );
            assert!(
                detail.get("resourceType").is_some() != naming.eq("resource"),
                "case {case:?}: expected only the {member} family's naming members, got {detail}"
            );
        }

        // The outcome of a failed invocation carries the error and the cause the run
        // produced, under the family's own failure detail.
        if let (Some(error), Some(outcome)) = (error, lifecycle.last()) {
            if outcome.ends_with("Failed") {
                let member = format!(
                    "{}EventDetails",
                    outcome[..1].to_lowercase() + &outcome[1..]
                );
                let event = only(&events, outcome, case);
                let detail = event
                    .get(&member)
                    .unwrap_or_else(|| panic!("case {case:?}: expected {member} on {event}"));
                assert_eq!(
                    detail.get("error").and_then(Value::as_str),
                    Some(error),
                    "case {case:?}: expected the failure to report {error}, got {detail}"
                );
            }
        }
    }
}

/// A read that asked for no execution data still reports what the work was handed under
/// `taskScheduledEventDetails.parameters`, and withholds the optional `input`/`output`
/// payload members beside it.
#[tokio::test]
async fn the_required_parameters_of_a_hand_over_survive_a_read_without_execution_data() {
    let chain = Chain::new();
    let function = chain
        .given_function_of("parameters-kept", HANDLER_SOURCE, None)
        .await;

    // (case, the state, the member its hand-over reports the document under, the detail)
    let rows: Vec<(&str, Value, &str, &str)> = vec![
        (
            "the optimized integration, whose hand-over carries required parameters",
            optimized_task(&function),
            "taskScheduledEventDetails",
            "parameters",
        ),
        (
            "a function ARN, whose hand-over carries an optional input",
            function_arn_task(&function),
            "lambdaFunctionScheduledEventDetails",
            "input",
        ),
    ];

    for (index, (case, state, member, document)) in rows.into_iter().enumerate() {
        let definition = json!({ "StartAt": "Work", "States": { "Work": state } });
        let described = chain
            .ran(
                case,
                &format!("parameters-kept-{index}"),
                &definition,
                &json!({ "orderId": "o-1" }),
            )
            .await;
        let execution = string(&described, "executionArn");

        // Read WITH the data, the hand-over reports the document the attempt built.
        let events = history(&chain, case, &execution, json!({})).await;
        let event_type = if member.starts_with("task") {
            "TaskScheduled"
        } else {
            "LambdaFunctionScheduled"
        };
        let with_data = only(&events, event_type, case)
            .get(member)
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: expected {member} in the history"));
        let built = with_data
            .get(document)
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: expected {document} on {with_data}"));
        assert!(
            built.contains("orderId"),
            "case {case:?}: expected the document the attempt built, got {built:?}"
        );

        // Read WITHOUT it, the REQUIRED member stays and the optional one goes.
        let events = history(
            &chain,
            case,
            &execution,
            json!({ "includeExecutionData": false }),
        )
        .await;
        let without = only(&events, event_type, case)
            .get(member)
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: expected {member} without the data"));
        let required = document == "parameters";
        let reported = without.get(document).and_then(Value::as_str);
        assert_eq!(
            reported.is_some_and(|text| !text.is_empty()),
            required,
            "case {case:?}: expected {document} to be reported={required} without execution \
             data, got {without}"
        );
        // And the state's own entered event, whose `input` is optional, loses it either
        // way: what `includeExecutionData` withholds is the payload members the model
        // pairs with an `inputDetails`/`outputDetails` flag.
        let entered = only(&events, "TaskStateEntered", case)
            .get("stateEnteredEventDetails")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: expected the entered detail"));
        assert!(
            entered.get("input").is_none_or(Value::is_null),
            "case {case:?}: expected no state input without execution data, got {entered}"
        );
    }
}

/// Every event of one history as `(type, id, previousEventId)`.
pub(crate) fn numbered(events: &[Value]) -> Vec<(String, i64, i64)> {
    events
        .iter()
        .map(|event| {
            (
                event
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                event.get("id").and_then(Value::as_i64).unwrap_or_default(),
                event
                    .get("previousEventId")
                    .and_then(Value::as_i64)
                    .unwrap_or_default(),
            )
        })
        .collect()
}

/// The numbering a list of event types takes when it is the WHOLE history: "numbered
/// sequentially, starting at one", each naming the event before it — except the event
/// written straight after the execution's own `ExecutionStarted`, which has no
/// predecessor and so names none, exactly as the start itself does.
fn expected_numbering(types: &[&str]) -> Vec<(String, i64, i64)> {
    types
        .iter()
        .enumerate()
        .map(|(position, event_type)| {
            let id = i64::try_from(position).expect("the count fits") + 1;
            let previous = if id <= 2 { 0 } else { id - 1 };
            ((*event_type).to_string(), id, previous)
        })
        .collect()
}

/// A `Task` state whose work failed records ONE event per lifecycle transition and then
/// the execution's own failure, with contiguous ids — and a redrive appends
/// `ExecutionRedriven` and a fresh cycle that continues the same numbering.
#[tokio::test]
async fn each_lifecycle_transition_is_its_own_event_and_a_redrive_continues_the_numbering() {
    let chain = Chain::new();
    let case = "a Task whose invocation raised, then redriven";
    let function = chain
        .given_function_of("lifecycle-raiser", HANDLER_SOURCE, None)
        .await;
    let definition = json!({
        "StartAt": "Work",
        "States": { "Work": optimized_task(&function) },
    });
    let described = chain
        .ran(
            case,
            "lifecycle-raiser",
            &definition,
            &json!({ "orderId": "o-1", "mode": "raise" }),
        )
        .await;
    let execution = string(&described, "executionArn");

    // The first attempt: the state's own pair around the work's three events, and then
    // the execution's failure. A failed state records no exited event.
    let first = &[
        "ExecutionStarted",
        "TaskStateEntered",
        "TaskScheduled",
        "TaskStarted",
        "TaskFailed",
        "ExecutionFailed",
    ];
    let events = history(&chain, case, &execution, json!({})).await;
    assert_eq!(
        numbered(&events),
        expected_numbering(first),
        "case {case:?}: expected one event per lifecycle transition, got {events:?}"
    );

    // Redriven, the attempt takes up the state it stopped at: the boundary is its own
    // event and the re-run's lifecycle events continue the same id sequence, so a
    // consumer that stored a `previousEventId` reconstructs one chain.
    chain
        .sfn(case, "RedriveExecution", &json!({ "executionArn": execution }))
        .await;
    chain.settled(case, &execution).await;
    let mut whole: Vec<&str> = first.to_vec();
    whole.extend([
        "ExecutionRedriven",
        "TaskScheduled",
        "TaskStarted",
        "TaskFailed",
        "ExecutionFailed",
    ]);
    let events = history(&chain, case, &execution, json!({})).await;
    assert_eq!(
        numbered(&events),
        expected_numbering(&whole),
        "case {case:?}: expected the redriven attempt to continue the numbering, got {events:?}"
    );
    // The boundary names how many times the execution had been redriven when it was
    // written, which is what makes a redriven attempt tell itself from the first.
    let boundary = only(&events, "ExecutionRedriven", case)
        .get("executionRedrivenEventDetails")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected the redrive detail"));
    assert_eq!(
        boundary.get("redriveCount").and_then(Value::as_i64),
        Some(1),
        "case {case:?}: expected the first redrive to be counted, got {boundary}"
    );

    // Read from the END, the same events in reverse: the page a client that wants the
    // latest first asks for opens on the terminal event of the redriven attempt.
    let reversed = history(&chain, case, &execution, json!({ "reverseOrder": true })).await;
    let mut backwards = expected_numbering(&whole);
    backwards.reverse();
    assert_eq!(
        numbered(&reversed),
        backwards,
        "case {case:?}: expected the reverse order to be the same events read from the end, got \
         {reversed:?}"
    );
}
