// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what a `Task` state reports when the LAMBDA FUNCTION it invoked
//! overran the FUNCTION'S OWN `Timeout`, driven over both services' own wire protocols
//! through the production `MultiServiceRouter` with Step Functions and Lambda mounted on
//! ONE engine — so the function really runs and really outlives its bound.
//!
//! The rule under test: a run the function's own timeout ended is a FAILED invocation,
//! named by the error Lambda reports for it, and not the STATE's own timeout. So the task
//! reports `TaskFailed` carrying that error name and the error document the run produced
//! as its cause; a `Retry` naming the state's timeout error does not match it, so the
//! function is scheduled exactly ONCE and no `TaskTimedOut` event is written; and the
//! execution ends `FAILED` under the same name a client would retry or catch on.
//!
//! The rows differ along what the rule generalises over: the function's own `Timeout`
//! (which is also the figure its error message states), and what the state declares about
//! retrying — nothing at all, a `Retry` on the state's timeout error (which must not
//! match), and a `Retry` on the task-failure wildcard (which does, so a definition that
//! asks for the retries gets them).
//!
//! The harness (`Chain`, `given_function_of`, `ran`) is the one `lambda_task_control_flow`
//! builds for the same two services; `tests/all.rs` is this binary's crate root, so it is
//! reached through `crate::lambda_task_control_flow`.

use serde_json::{json, Value};

use crate::lambda_task_control_flow::Chain;
use crate::state_machine_lifecycle::string;

/// A handler that sleeps far longer than any `Timeout` a row gives it, so what ends the
/// run is that bound rather than the handler returning.
const SLEEPING_HANDLER: &str = r#"
import time


def handler(event, context):
    time.sleep(30)
    return {"slept": True}
"#;

/// The error name Lambda reports for a run its sandbox ended at the function's `Timeout`,
/// which is the name the failed task carries — and, because it is not the state's own
/// timeout error, the name a `Retry` on that error does not match.
const SANDBOX_TIMED_OUT: &str = "Sandbox.Timedout";

/// The Amazon States Language's own error name for the STATE's timeout, which a client
/// writes a `Retry` against and which this failure must NOT be reported under.
const STATES_TIMEOUT: &str = "States.Timeout";

/// The wildcard the Amazon States Language documents for a task failure, which DOES match
/// it.
const STATES_TASK_FAILED: &str = "States.TaskFailed";

/// One row: what the state declares about retrying, and how many attempts that makes.
struct Row {
    case: &'static str,
    /// The function's own `Timeout`, in seconds.
    timeout: u64,
    /// The `ErrorEquals` the state's `Retry` names, and `None` for a state declaring no
    /// `Retry` at all.
    retry_on: Option<&'static str>,
    /// How many times the task is scheduled: once for a retrier the failure does not
    /// match, and once per attempt for one it does.
    scheduled: usize,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "no Retry at all",
            timeout: 1,
            retry_on: None,
            scheduled: 1,
        },
        Row {
            case: "a Retry on the state's own timeout error, which does not match",
            timeout: 1,
            retry_on: Some(STATES_TIMEOUT),
            scheduled: 1,
        },
        Row {
            case: "a longer function timeout, which the message states",
            timeout: 2,
            retry_on: Some(STATES_TIMEOUT),
            scheduled: 1,
        },
        Row {
            case: "a Retry on the task-failure wildcard, which does match",
            timeout: 1,
            retry_on: Some(STATES_TASK_FAILED),
            scheduled: 2,
        },
    ]
}

/// The definition one row runs: one `Task` state invoking the function, with the row's
/// own `Retry`.
fn definition(function_arn: &str, retry_on: Option<&str>) -> Value {
    let mut task = json!({
        "Type": "Task",
        "Resource": "arn:aws:states:::lambda:invoke",
        "Parameters": { "FunctionName": function_arn, "Payload": {} },
        "End": true,
    });
    if let Some(error) = retry_on {
        task["Retry"] = json!([{
            "ErrorEquals": [error],
            "IntervalSeconds": 1,
            "MaxAttempts": 1,
            "BackoffRate": 1.0,
        }]);
    }
    json!({ "StartAt": "Invoke", "States": { "Invoke": task } })
}

/// The types of a history, in the order the events happened.
fn event_types(events: &[Value]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| event.get("type").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

/// The `taskFailedEventDetails` of the one `TaskFailed` event a run wrote.
fn task_failure(events: &[Value], case: &str) -> Value {
    let failures: Vec<&Value> = events
        .iter()
        .filter(|event| event.get("type").and_then(Value::as_str) == Some("TaskFailed"))
        .collect();
    assert_eq!(
        failures.len(),
        1,
        "case {case:?}: expected exactly one TaskFailed, got {:?}",
        event_types(events)
    );
    failures[0]
        .get("taskFailedEventDetails")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected the failure's detail on {:?}", failures[0]))
}

/// A function the sandbox ended at its own `Timeout` fails the task under Lambda's own
/// error name, so the state's timeout `Retry` does not match it and the function runs
/// once.
#[tokio::test]
async fn a_function_timeout_is_a_failed_task_and_not_the_states_own_timeout() {
    let chain = Chain::new();
    for (index, row) in rows().into_iter().enumerate() {
        let Row {
            case,
            timeout,
            retry_on,
            scheduled,
        } = row;
        let function = chain
            .given_function_of(&format!("slow-{index}"), SLEEPING_HANDLER, Some(timeout))
            .await;
        let described = chain
            .ran(
                case,
                &format!("function-timeout-{index}"),
                &definition(&function, retry_on),
                &json!({}),
            )
            .await;
        assert_eq!(
            (
                described.get("status").and_then(Value::as_str),
                described.get("error").and_then(Value::as_str),
            ),
            (Some("FAILED"), Some(SANDBOX_TIMED_OUT)),
            "case {case:?}: expected the execution to fail under {SANDBOX_TIMED_OUT}, got \
             {described}"
        );

        let answered = chain
            .sfn(
                case,
                "GetExecutionHistory",
                &json!({ "executionArn": string(&described, "executionArn") }),
            )
            .await;
        let events = answered
            .get("events")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let types = event_types(&events);

        // The attempts the definition really asked for, and no state-level timeout among
        // them: what ran out was the function's bound, not the state's.
        assert_eq!(
            types.iter().filter(|kind| *kind == "TaskScheduled").count(),
            scheduled,
            "case {case:?}: expected {scheduled} attempt(s), got {types:?}"
        );
        assert!(
            !types.iter().any(|kind| kind == "TaskTimedOut"),
            "case {case:?}: expected no state-level timeout event, got {types:?}"
        );

        // The failure the task reports: Lambda's own error name, and the error document
        // the run produced as the cause.
        let detail = task_failure(&events[events.len() - 2..], case);
        assert_eq!(
            detail.get("error").and_then(Value::as_str),
            Some(SANDBOX_TIMED_OUT),
            "case {case:?}: expected the sandbox timeout as the task's error, got {detail}"
        );
        let cause = detail
            .get("cause")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let document: Value = serde_json::from_str(cause).unwrap_or_else(|error| {
            panic!("case {case:?}: expected the cause to be the run's error document, got {error} for {cause:?}")
        });
        assert_eq!(
            document.get("errorType").and_then(Value::as_str),
            Some(SANDBOX_TIMED_OUT),
            "case {case:?}: expected the document to name the sandbox timeout, got {document}"
        );
        let stated = format!("Task timed out after {timeout}.00 seconds");
        let message = document
            .get("errorMessage")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            message.contains(&stated),
            "case {case:?}: expected the message to state {stated:?}, got {message:?}"
        );
    }
}
