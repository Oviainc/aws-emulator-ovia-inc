// SPDX-License-Identifier: CC0-1.0

//! The callback (`.waitForTaskToken`) pattern over a LAMBDA invocation, driven over both
//! services' own wire protocols through the production `MultiServiceRouter` with Step
//! Functions and Lambda mounted on ONE engine — so the function really runs and the
//! `SendTask*` the test asserts on is the one its own handler made.
//!
//! RULE: a callback task's `taskToken` answers that task from the instant the state's
//! integration call CARRIES it, which is before that call has come back — so a
//! `SendTaskSuccess` or `SendTaskFailure` quoting it is accepted while the call is still
//! in flight, and what it reports is the state's own outcome: a success is the result the
//! state's result-side data flow places, a failure is reported with exactly the `error`
//! and `cause` it named. A token nothing has answered leaves the execution `RUNNING`, and
//! is answered just the same once the call has come back.
//!
//! That is how the Lambda callback pattern is written: the function the task hands the
//! token to calls `send_task_success`/`send_task_failure` from inside the very invocation
//! that delivered it, through the AWS SDK its runtime supplies.
//!
//! The rows differ along what the rule generalises over: which `Resource` form names the
//! invocation (the optimized `lambda:invoke` integration and the AWS SDK one), WHEN the
//! answer arrives (from inside the call, and from outside it after the call came back),
//! which `SendTask*` answers it, and whether the state declares a `ResultPath` for the
//! answer to be placed through.
//!
//! The harness is `lambda_task_control_flow`'s `Chain` (Step Functions and Lambda over one
//! engine); `tests/all.rs` is this binary's crate root, so it is reached through
//! `crate::lambda_task_control_flow`.

use serde_json::{json, Value};

use service_com_amazonaws_sfn_awsstepfunctions::state::EVENT_TASK_SUBMITTED;

use crate::lambda_task_control_flow::Chain;
use crate::state_machine_lifecycle::{string, PAST_THE_EXECUTION_DELAY, ROLE_ARN};

/// The handler every row invokes: it is handed the task token in its event and answers
/// the task through the AWS SDK its runtime supplies, exactly as the Lambda callback
/// pattern is written — or holds the token and answers nothing, for a row whose answer
/// comes from outside the invocation.
///
/// It reports the token back in its own return value too, so the row that answers from
/// outside reads the token the invocation really received rather than one this suite
/// assumed.
const HANDLER_SOURCE: &str = r#"
import json

import boto3
import botocore.exceptions


def handler(event, context):
    token = event["token"]
    mode = event["mode"]
    states = boto3.client("stepfunctions")
    if mode in ("success", "twice"):
        states.send_task_success(
            taskToken=token,
            output=json.dumps({"answered": "by the work itself", "mode": mode}),
        )
    elif mode == "failure":
        states.send_task_failure(
            taskToken=token,
            error=event["error"],
            cause=event["cause"],
        )
    second = None
    if mode == "twice":
        try:
            states.send_task_success(taskToken=token, output="{}")
        except botocore.exceptions.ClientError as error:
            second = error.response["Error"]["Code"]
    return {"token": token, "mode": mode, "second": second}
"#;

/// The `error` and `cause` the handler's `send_task_failure` reports, which the execution
/// must carry verbatim.
const REPORTED_ERROR: &str = "LambdaReportedFailure";
/// See [`REPORTED_ERROR`].
const REPORTED_CAUSE: &str = "the function decided the unit of work had failed";

/// The document the handler's `send_task_success` sends as the task's `output`.
fn answered_output(mode: &str) -> Value {
    json!({ "answered": "by the work itself", "mode": mode })
}

/// The modelled error code a second answer to one token is refused with: "the task token
/// has either expired or the task associated with the token has already been closed"
/// (`SendTaskSuccess`'s `TaskTimedOut`).
const CLOSED_TASK_CODE: &str = "TaskTimedOut";

/// The `output` the row that answers from OUTSIDE the invocation sends.
const OUTSIDE_OUTPUT: &str = r#"{"answered":"from outside the invocation"}"#;

/// The execution input every row starts with, which a `ResultPath` row's output must
/// still carry.
fn execution_input() -> Value {
    json!({ "from": "the start" })
}

/// Which `Resource` form names the invocation, and how the function's own answer is
/// reported in the submission a callback task records.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Integration {
    /// `arn:aws:states:::lambda:invoke.waitForTaskToken`: the optimized integration,
    /// whose result is the literal SDK response, so the function's return value is under
    /// `Payload`.
    Optimized,
    /// `arn:aws:states:::aws-sdk:lambda:invoke.waitForTaskToken`: the AWS SDK family,
    /// whose result is the operation's own members — `Payload` among them.
    AwsSdk,
}

impl Integration {
    fn resource(self) -> &'static str {
        match self {
            Self::Optimized => "arn:aws:states:::lambda:invoke.waitForTaskToken",
            Self::AwsSdk => "arn:aws:states:::aws-sdk:lambda:invoke.waitForTaskToken",
        }
    }
}

/// Who answers the token, and with what.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Answer {
    /// The handler itself, from inside the invocation that delivered the token.
    HandlerSucceeds,
    /// The same, with `SendTaskFailure`.
    HandlerFails,
    /// The handler answers the token TWICE from inside the invocation: one token answers
    /// one task, so the second answer is refused and the first one still decides the
    /// state.
    HandlerAnswersTwice,
    /// Nobody, during the invocation: the execution stays `RUNNING` and this suite
    /// answers the token afterwards, with the token the invocation reported receiving.
    FromOutside,
}

impl Answer {
    /// The `mode` the handler is handed, which decides what it calls.
    fn mode(self) -> &'static str {
        match self {
            Self::HandlerSucceeds => "success",
            Self::HandlerFails => "failure",
            Self::HandlerAnswersTwice => "twice",
            Self::FromOutside => "hold",
        }
    }
}

/// One row: the integration the pattern runs over, who answers the token, whether the
/// state declares a `ResultPath`, and what the execution must end as.
struct Row {
    case: &'static str,
    integration: Integration,
    answer: Answer,
    result_path: Option<&'static str>,
    status: &'static str,
}

const ROWS: &[Row] = &[
    Row {
        case: "the optimized integration, answered by the handler with a success",
        integration: Integration::Optimized,
        answer: Answer::HandlerSucceeds,
        result_path: None,
        status: "SUCCEEDED",
    },
    Row {
        case: "the optimized integration, answered by the handler with a failure",
        integration: Integration::Optimized,
        answer: Answer::HandlerFails,
        result_path: None,
        status: "FAILED",
    },
    Row {
        case: "the AWS SDK integration, answered by the handler with a success",
        integration: Integration::AwsSdk,
        answer: Answer::HandlerSucceeds,
        result_path: None,
        status: "SUCCEEDED",
    },
    Row {
        case: "a success the handler sends, placed through the state's ResultPath",
        integration: Integration::Optimized,
        answer: Answer::HandlerSucceeds,
        result_path: Some("$.reply"),
        status: "SUCCEEDED",
    },
    Row {
        case: "a token the invocation held, answered from outside it",
        integration: Integration::Optimized,
        answer: Answer::FromOutside,
        result_path: None,
        status: "SUCCEEDED",
    },
    Row {
        case: "a token the handler answers twice: the second answer is refused",
        integration: Integration::Optimized,
        answer: Answer::HandlerAnswersTwice,
        result_path: None,
        status: "SUCCEEDED",
    },
];

/// The one-state definition a row runs: a callback `Task` over `integration`, handing the
/// function the token out of the context object (`$$.Task.Token`) and the mode the row
/// asks for.
fn definition(row: &Row, function_arn: &str) -> Value {
    let mut task = json!({
        "Type": "Task",
        "Resource": row.integration.resource(),
        "Parameters": {
            "FunctionName": function_arn,
            "Payload": {
                "token.$": "$$.Task.Token",
                "mode": row.answer.mode(),
                "error": REPORTED_ERROR,
                "cause": REPORTED_CAUSE,
            },
        },
        "End": true,
    });
    if let Some(result_path) = row.result_path {
        task["ResultPath"] = json!(result_path);
    }
    json!({ "StartAt": "HandOver", "States": { "HandOver": task } })
}

/// The events of `execution`, as `GetExecutionHistory` answers them.
async fn events(chain: &Chain, case: &str, execution: &str) -> Vec<Value> {
    let answered = chain
        .sfn(
            case,
            "GetExecutionHistory",
            &json!({ "executionArn": execution }),
        )
        .await;
    answered
        .get("events")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected events in {answered}"))
        .clone()
}

/// The document the callback task's own call ANSWERED, as its `TaskSubmitted` event
/// reports it ("the response from a resource when a task has started").
async fn submitted(chain: &Chain, case: &str, execution: &str) -> Value {
    let event = events(chain, case, execution)
        .await
        .into_iter()
        .find(|event| event.get("type").and_then(Value::as_str) == Some(EVENT_TASK_SUBMITTED))
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a {EVENT_TASK_SUBMITTED} event in the history")
        });
    let details = event
        .get("taskSubmittedEventDetails")
        .unwrap_or_else(|| panic!("case {case:?}: expected the submission's details in {event}"));
    serde_json::from_str(&string(details, "output"))
        .unwrap_or_else(|why| panic!("case {case:?}: the submission's output is JSON: {why}"))
}

/// The token the invocation really received, read back out of the function's OWN return
/// value: both `Resource` forms report the invocation's answer with the function's return
/// value under `Payload`.
async fn received_token(chain: &Chain, case: &str, execution: &str) -> String {
    let reported = submitted(chain, case, execution).await;
    let payload = reported
        .get("Payload")
        .unwrap_or_else(|| panic!("case {case:?}: expected the invocation's Payload in {reported}"));
    payload
        .get("token")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected the token the handler saw in {payload}"))
        .to_string()
}

/// Every row: the token answers the task whoever quotes it and whenever, and the answer is
/// the state's own outcome.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lambda_callback_task_is_finished_by_the_answer_to_its_token() {
    let chain = Chain::new();
    let function_arn = chain
        .given_function_of("callback-worker", HANDLER_SOURCE, Some(20))
        .await;

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let created = chain
            .sfn(
                case,
                "CreateStateMachine",
                &json!({
                    "name": format!("callback-{index}"),
                    "definition": definition(row, &function_arn).to_string(),
                    "roleArn": ROLE_ARN,
                }),
            )
            .await;
        let machine = string(&created, "stateMachineArn");
        let started = chain
            .sfn(
                case,
                "StartExecution",
                &json!({
                    "stateMachineArn": machine,
                    "name": "only",
                    "input": execution_input().to_string(),
                }),
            )
            .await;
        let execution = string(&started, "executionArn");

        // A row whose token nothing answered during the invocation is a task still
        // outstanding: the execution is RUNNING past the delay its own outcome would have
        // settled after, and the answer this suite then sends finishes it.
        if row.answer == Answer::FromOutside {
            std::thread::sleep(PAST_THE_EXECUTION_DELAY);
            let waiting = chain
                .sfn(
                    case,
                    "DescribeExecution",
                    &json!({ "executionArn": execution }),
                )
                .await;
            assert_eq!(
                waiting.get("status").and_then(Value::as_str),
                Some("RUNNING"),
                "case {case:?}: expected RUNNING while the token is unanswered, got {waiting}"
            );
            let token = received_token(&chain, case, &execution).await;
            chain
                .sfn(
                    case,
                    "SendTaskSuccess",
                    &json!({ "taskToken": token, "output": OUTSIDE_OUTPUT }),
                )
                .await;
        }

        let settled = chain.settled(case, &execution).await;
        assert_eq!(
            settled.get("status").and_then(Value::as_str),
            Some(row.status),
            "case {case:?}: expected {}, got {settled}",
            row.status
        );
        match row.answer {
            Answer::HandlerFails => {
                assert_eq!(
                    settled.get("error").and_then(Value::as_str),
                    Some(REPORTED_ERROR),
                    "case {case:?}: expected the error the answer named, got {settled}"
                );
                assert_eq!(
                    settled.get("cause").and_then(Value::as_str),
                    Some(REPORTED_CAUSE),
                    "case {case:?}: expected the cause the answer named, got {settled}"
                );
            }
            Answer::HandlerSucceeds | Answer::HandlerAnswersTwice | Answer::FromOutside => {
                assert!(
                    settled.get("error").is_none(),
                    "case {case:?}: an answered success reports no error, got {settled}"
                );
                let output: Value = serde_json::from_str(&string(&settled, "output"))
                    .unwrap_or_else(|why| panic!("case {case:?}: the output is JSON: {why}"));
                let answered = match row.answer {
                    Answer::FromOutside => serde_json::from_str(OUTSIDE_OUTPUT)
                        .expect("the output this suite sends is JSON"),
                    _ => answered_output(row.answer.mode()),
                };
                // The state's own result-side data flow places the answer: a state with no
                // `ResultPath` reports the answer itself, and one declaring a path reports
                // the state's input with the answer AT that path.
                let expected = match row.result_path {
                    None => answered,
                    Some("$.reply") => {
                        let mut merged = execution_input();
                        merged["reply"] = answered;
                        merged
                    }
                    Some(other) => panic!("case {case:?}: no expectation for ResultPath {other}"),
                };
                assert_eq!(
                    output, expected,
                    "case {case:?}: expected the answer the token was finished with, got {output}"
                );
            }
        }
        // One token answers one task: the handler that answered twice had its SECOND
        // answer refused, with the modelled code for a task already closed, and the row
        // above has just asserted that the first one still decided the state.
        if row.answer == Answer::HandlerAnswersTwice {
            let reported = submitted(&chain, case, &execution).await;
            let payload = reported.get("Payload").unwrap_or_else(|| {
                panic!("case {case:?}: expected the invocation's Payload in {reported}")
            });
            assert_eq!(
                payload.get("second").and_then(Value::as_str),
                Some(CLOSED_TASK_CODE),
                "case {case:?}: expected the second answer to be refused with \
                 {CLOSED_TASK_CODE}, got {payload}"
            );
        }
    }
}
