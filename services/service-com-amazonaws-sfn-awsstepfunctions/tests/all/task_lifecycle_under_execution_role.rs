// SPDX-License-Identifier: CC0-1.0

//! What the EXECUTION ROLE does to the history a `Task` state's work writes, driven
//! through Step Functions, IAM and Lambda mounted on ONE engine — the shape
//! `core/router/src/lib.rs` mounts them in — so the `roleArn` is a role IAM's own
//! handlers hold, the access gate in front of the invocation really runs, and the
//! function really runs.
//!
//! RULE: the gate decides what the attempt CLOSES with, not whether it began. Whatever
//! the role's policies say, an attempt records the same lifecycle — the state's entered
//! event, then the work's hand-over, its start and its outcome, then the execution's own
//! terminal event, "numbered sequentially, starting at one" with each event naming the
//! one before it — because the hand-over and the start are written before anything about
//! the call's outcome is known. What the policies decide is the OUTCOME: the run's own
//! error document where they allowed the call, and the invoked service's own
//! access-denied refusal where they did not, in which case nothing was invoked.
//!
//! The rows differ along what the rule generalises over: what the role's policy says
//! about the call — a grant naming the function's own ARN, a grant over every resource,
//! and a policy that grants the invocation nothing at all.
//!
//! This is the three-service reading of the rule `tests/all/task_work_event_families.rs`
//! states over Step Functions and Lambda alone, and it reuses that suite's readings of a
//! history (`numbered`, `detail`) and `lambda_task_control_flow`'s harness
//! (`Chain::with_iam`, `role`, `given_function_of`, `ran_as`); `tests/all.rs` is this
//! binary's crate root, so both are reached through `crate::<suite>`.

use serde_json::{json, Value};

use crate::lambda_task_control_flow::Chain;
use crate::state_machine_lifecycle::string;
use crate::task_work_event_families::{detail, numbered};

/// The handler every row invokes: it raises, so the task's outcome is the run's own error
/// document and the error name is the class the handler raised.
const HANDLER_SOURCE: &str = r#"
def handler(event, context):
    raise RuntimeError("order %s cannot be processed" % event.get("orderId"))
"#;

/// The `errorType` of the document that raise produces, which is the name the failed unit
/// of work and the failed execution both report once the call was made.
const RAISED_ERROR_TYPE: &str = "RuntimeError";

/// The IAM action an invocation is subject to, which is the one the invoked operation
/// PUBLISHES rather than the operation's own name.
const INVOKE_ACTION: &str = "lambda:InvokeFunction";

/// The error a `Task` state reports when the role may not make the call: the invoked
/// service's base SDK exception, which is what an error its operation does not model is
/// reported under.
const REFUSED_ERROR: &str = "Lambda.AWSLambdaException";

/// What the cause of that refusal carries: the code the answer came back under.
const REFUSED_CODE: &str = "Error Code: AccessDeniedException";

/// What one row's policy does to the call, and so what the attempt closes with.
enum Decided {
    /// The call was made: the run's own error closed the attempt.
    Made,
    /// The call was not made: the attempt closes on the invoked service's access-denied
    /// refusal instead.
    Refused,
}

/// The lifecycle a row's work records, as `(type, id, previousEventId)`: the pair the
/// state was entered with around the events the work itself wrote — the SAME events
/// whatever the role's policies decided, because the attempt began either way.
fn expected() -> Vec<(String, i64, i64)> {
    let types: &[&str] = &[
        "ExecutionStarted",
        "TaskStateEntered",
        "TaskScheduled",
        "TaskStarted",
        "TaskFailed",
        "ExecutionFailed",
    ];
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

/// Every attempt records the whole lifecycle, contiguously numbered, whatever the role's
/// policies decided; what they decide is the failure the attempt closes with.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_roles_decision_decides_what_the_attempt_closes_with_not_how_it_is_recorded() {
    let chain = Chain::with_iam();
    let function = chain
        .given_function_of("lifecycle-under-role", HANDLER_SOURCE, None)
        .await;

    // (case, the statements the role's inline policy carries, what they decide)
    let rows: Vec<(&str, Value, Decided)> = vec![
        (
            "a role granted the invocation on the function's own ARN",
            json!([{
                "Effect": "Allow",
                "Action": INVOKE_ACTION,
                "Resource": function.clone(),
            }]),
            Decided::Made,
        ),
        (
            "a role granted it over every resource",
            json!([{ "Effect": "Allow", "Action": INVOKE_ACTION, "Resource": "*" }]),
            Decided::Made,
        ),
        (
            "a role whose policy says nothing about the invocation",
            json!([{
                "Effect": "Allow",
                "Action": "logs:PutLogEvents",
                "Resource": "*",
            }]),
            Decided::Refused,
        ),
    ];

    for (index, (case, statements, decided)) in rows.into_iter().enumerate() {
        let role = chain
            .role(case, &format!("LifecycleRole{index}"), &statements)
            .await;
        let definition = json!({
            "StartAt": "InvokeProcessing",
            "States": {
                "InvokeProcessing": {
                    "Type": "Task",
                    "Resource": "arn:aws:states:::lambda:invoke",
                    "Parameters": { "FunctionName": function.clone(), "Payload.$": "$" },
                    "End": true,
                },
            },
        });
        let described = chain
            .ran_as(
                case,
                &format!("lifecycle-under-role-{index}"),
                &definition,
                &json!({ "orderId": "o-1" }),
                &role,
            )
            .await;

        // Which failure the execution reports is what tells the two paths apart: the
        // run's own error once the call was made, and the invoked service's access-denied
        // refusal when it was not.
        let reported = match decided {
            Decided::Made => RAISED_ERROR_TYPE,
            Decided::Refused => REFUSED_ERROR,
        };
        assert_eq!(
            (
                described.get("status").and_then(Value::as_str),
                described.get("error").and_then(Value::as_str),
            ),
            (Some("FAILED"), Some(reported)),
            "case {case:?}: expected the execution to fail under {reported:?}, got {described}"
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
            .unwrap_or_else(|| panic!("case {case:?}: expected events in {answered}"));
        assert_eq!(
            numbered(&events),
            expected(),
            "case {case:?}: expected the lifecycle of the work the role decided, got {events:?}"
        );

        // The hand-over is written either way, and carries the document the attempt
        // really built — the resolved `FunctionName` and `Payload` of this invocation.
        let scheduled = detail(&events, "TaskScheduled", "taskScheduledEventDetails", case);
        let parameters: Value = scheduled
            .get("parameters")
            .and_then(Value::as_str)
            .map(|text| serde_json::from_str(text).expect("the parameters are JSON"))
            .unwrap_or_else(|| panic!("case {case:?}: expected parameters on {scheduled}"));
        assert_eq!(
            parameters,
            json!({ "FunctionName": function.clone(), "Payload": { "orderId": "o-1" } }),
            "case {case:?}: expected the resolved document, got {parameters}"
        );

        // Every event the work wrote names the work the `Resource` asked for.
        let named: &[(&str, &str)] = &[
            ("TaskStarted", "taskStartedEventDetails"),
            ("TaskFailed", "taskFailedEventDetails"),
        ];
        for (event_type, member) in named {
            let detail = detail(&events, event_type, member, case);
            assert_eq!(
                (
                    detail.get("resourceType").and_then(Value::as_str),
                    detail.get("resource").and_then(Value::as_str),
                ),
                (Some("lambda"), Some("invoke")),
                "case {case:?}: expected {event_type} to name the work, got {detail}"
            );
        }

        // And the failure the task reports is the one the execution repeats: the run's
        // own error document, or the refusal of a call nothing made.
        let failed = detail(&events, "TaskFailed", "taskFailedEventDetails", case);
        let ended = detail(&events, "ExecutionFailed", "executionFailedEventDetails", case);
        assert_eq!(
            (
                failed.get("error").and_then(Value::as_str),
                ended.get("error").and_then(Value::as_str),
            ),
            (Some(reported), Some(reported)),
            "case {case:?}: expected {reported:?} on both failures, got {failed} and {ended}"
        );
        let cause = failed
            .get("cause")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: expected a cause on {failed}"));
        match decided {
            Decided::Made => {
                let document: Value = serde_json::from_str(cause)
                    .expect("the cause of a call that was made is the run's own document");
                assert_eq!(
                    document.get("errorType").and_then(Value::as_str),
                    Some(RAISED_ERROR_TYPE),
                    "case {case:?}: expected the run's error document, got {document}"
                );
            }
            Decided::Refused => assert!(
                cause.contains(REFUSED_CODE) && cause.contains(INVOKE_ACTION),
                "case {case:?}: expected the refusal to name the code and the action, got \
                 {cause:?}"
            ),
        }
    }
}
