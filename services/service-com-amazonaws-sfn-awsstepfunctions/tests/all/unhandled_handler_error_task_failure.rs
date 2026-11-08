// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what an optimized `arn:aws:states:::lambda:invoke` `Task` state
//! reports when the HANDLER it invoked ended by RAISING, driven over both services' own
//! wire protocols through the production `MultiServiceRouter` with Step Functions and
//! Lambda mounted on ONE engine — so the function really runs and the task's outcome is
//! that run's own.
//!
//! RULE: an `Invoke` the service SERVED whose answer carries a non-empty `FunctionError`
//! is a FAILED unit of work for the `Task` state that asked for it, never a result: the
//! state's error is the `errorType` the run's error document names and its cause is that
//! document verbatim, and both are carried by the state's `TaskFailed` event (beside the
//! `resourceType` and `resource` the `Resource` names), by the execution's
//! `ExecutionFailed` event and by `DescribeExecution`, with no success event for the
//! state. An `Invoke` whose handler RETURNED carries no such signal, so the same state
//! succeeds and reports what the function answered.
//!
//! The rows differ along what the rule generalises over: the runtime the handler is
//! written for (which also decides the member the document reports its stack under), the
//! class of the value the handler ended with (a custom error class that sets its own
//! `name`, one of the language's own error classes, and a thrown value that is not an
//! error at all), and the order number the input carries, which the raised message is
//! built from — plus the row that returns instead of raising.
//!
//! The harness (`Chain`, `given_function_in`, `ran`, `sfn`) is the one
//! `lambda_task_control_flow` builds for the same two services, and the history's event
//! types are read through `lambda_function_timeout`'s own reading of them; `tests/all.rs`
//! is this binary's crate root, so both are reached through `crate::<suite>`.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::lambda_function_timeout::event_types;
use crate::lambda_task_control_flow::Chain;
use crate::state_machine_lifecycle::string;

/// The node runtime a row's handler is written for, and the file its module lives in.
const NODE_RUNTIME: &str = "nodejs22.x";
/// See [`NODE_RUNTIME`].
const NODE_MODULE: &str = "index.js";

/// The python runtime a row's handler is written for, and the file its module lives in.
const PYTHON_RUNTIME: &str = "python3.12";
/// See [`PYTHON_RUNTIME`].
const PYTHON_MODULE: &str = "index.py";

/// The node handler every node row invokes: what it ends with is decided by the `kind`
/// the EVENT carries, and the message it raises embeds the `order` the event carries, so
/// the error document is the function's and its input's rather than anything fixed here.
const NODE_HANDLER: &str = r#"
class CustomBoomError extends Error {
    constructor(message) {
        super(message);
        this.name = 'CustomBoomError';
    }
}

exports.handler = async (event) => {
    if (event.kind === 'custom') {
        throw new CustomBoomError('order ' + event.order + ' cannot be processed');
    }
    if (event.kind === 'builtin') {
        throw new TypeError('order ' + event.order + ' is not a function');
    }
    if (event.kind === 'bare') {
        throw 'order ' + event.order + ' was rejected';
    }
    return { order: event.order, handled: true };
};
"#;

/// The python handler, written for the same event.
const PYTHON_HANDLER: &str = r#"
class OrderRejected(Exception):
    pass


def handler(event, context):
    if event.get("kind") == "custom":
        raise OrderRejected("order %s cannot be processed" % event["order"])
    return {"order": event["order"], "handled": True}
"#;

/// What the `Resource` of the state under test names, and so what its own events report
/// the work as: the service and the action of the optimized integration
/// (`TaskFailedEventDetails$resourceType` is "the service name of the resource in a task
/// state" and `$resource` "the action of the resource called by a task state").
const RESOURCE: &str = "arn:aws:states:::lambda:invoke";
/// See [`RESOURCE`].
const RESOURCE_TYPE: &str = "lambda";
/// See [`RESOURCE`].
const RESOURCE_ACTION: &str = "invoke";

/// Where the document reports the stack of a run that raised, and what that stack
/// carries — which is the runtime's own reporting rather than Step Functions'.
enum Trace {
    /// A node runtime reports it under `trace`, whose first element is the fixed
    /// "<errorType>: <errorMessage>" line.
    NodeErrorLine,
    /// A value that is not an error carries no stack at all, so `trace` is empty.
    NodeEmpty,
    /// A python runtime reports its frames under `stackTrace`, each naming the file the
    /// frame is in.
    PythonFrames,
}

impl Trace {
    /// The member the document carries the stack under.
    fn member(&self) -> &'static str {
        match self {
            Self::NodeErrorLine | Self::NodeEmpty => "trace",
            Self::PythonFrames => "stackTrace",
        }
    }

    /// Every member the document carries: the class, the message and the stack, plus —
    /// for a python runtime — the invocation the run answered, which that runtime names
    /// in its error document beside them.
    fn members(&self) -> BTreeSet<String> {
        let mut named = vec!["errorType", "errorMessage", self.member()];
        if matches!(self, Self::PythonFrames) {
            named.push("requestId");
        }
        named.iter().map(|member| (*member).to_string()).collect()
    }
}

/// What a row's run ends as.
enum Outcome {
    /// The handler RAISED: the class the document names, the message it carries (built
    /// from the row's own input), and what its stack carries.
    Raised {
        error_type: &'static str,
        message: String,
        trace: Trace,
    },
    /// The handler RETURNED, so the task succeeds and reports what it answered.
    Returned,
}

/// One row: the function it invokes, the input the execution carries, and the outcome.
struct Row {
    case: &'static str,
    /// Which of the two functions the state invokes.
    runtime: &'static str,
    /// The `kind` the input carries, which selects what the handler does.
    kind: &'static str,
    /// The `order` the input carries, which the raised message is built from.
    order: u64,
    outcome: Outcome,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a custom error class that sets its own name",
            runtime: NODE_RUNTIME,
            kind: "custom",
            order: 17,
            outcome: Outcome::Raised {
                error_type: "CustomBoomError",
                message: "order 17 cannot be processed".to_string(),
                trace: Trace::NodeErrorLine,
            },
        },
        Row {
            case: "the same class raised over another order",
            runtime: NODE_RUNTIME,
            kind: "custom",
            order: 4096,
            outcome: Outcome::Raised {
                error_type: "CustomBoomError",
                message: "order 4096 cannot be processed".to_string(),
                trace: Trace::NodeErrorLine,
            },
        },
        Row {
            case: "one of the language's own error classes",
            runtime: NODE_RUNTIME,
            kind: "builtin",
            order: 5,
            outcome: Outcome::Raised {
                error_type: "TypeError",
                message: "order 5 is not a function".to_string(),
                trace: Trace::NodeErrorLine,
            },
        },
        Row {
            case: "a thrown value that is not an error at all",
            runtime: NODE_RUNTIME,
            kind: "bare",
            order: 6,
            outcome: Outcome::Raised {
                error_type: "string",
                message: "order 6 was rejected".to_string(),
                trace: Trace::NodeEmpty,
            },
        },
        Row {
            case: "a handler of another runtime raising its own exception class",
            runtime: PYTHON_RUNTIME,
            kind: "custom",
            order: 7,
            outcome: Outcome::Raised {
                error_type: "OrderRejected",
                message: "order 7 cannot be processed".to_string(),
                trace: Trace::PythonFrames,
            },
        },
        Row {
            case: "a handler that returns instead of raising",
            runtime: NODE_RUNTIME,
            kind: "return",
            order: 8,
            outcome: Outcome::Returned,
        },
        Row {
            case: "the same, on the other runtime",
            runtime: PYTHON_RUNTIME,
            kind: "return",
            order: 9,
            outcome: Outcome::Returned,
        },
    ]
}

/// The definition every row runs: ONE `Task` state on the optimized integration, handing
/// the execution's whole input to the function, with no `Retry` and no `Catch` — so the
/// task's outcome is the execution's.
fn definition(function_arn: &str) -> Value {
    json!({
        "StartAt": "Invoke",
        "States": {
            "Invoke": {
                "Type": "Task",
                "Resource": RESOURCE,
                "Parameters": { "FunctionName": function_arn, "Payload.$": "$" },
                "End": true,
            },
        },
    })
}

/// The details of the one event of `kind` a run wrote, under `member`.
fn only_detail(events: &[Value], kind: &str, member: &str, case: &str) -> Value {
    let found: Vec<&Value> = events
        .iter()
        .filter(|event| event.get("type").and_then(Value::as_str) == Some(kind))
        .collect();
    assert_eq!(
        found.len(),
        1,
        "case {case:?}: expected exactly one {kind}, got {:?}",
        event_types(events)
    );
    found[0]
        .get(member)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected {member} on {}", found[0]))
}

/// How many events of `kind` a run wrote.
fn count(events: &[Value], kind: &str) -> usize {
    event_types(events)
        .iter()
        .filter(|written| *written == kind)
        .count()
}

/// The `cause` and `error` a detail carries, as text.
fn failure(detail: &Value, case: &str) -> (String, String) {
    let error = detail
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected an error on {detail}"))
        .to_string();
    let cause = detail
        .get("cause")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a cause on {detail}"))
        .to_string();
    (error, cause)
}

/// The members a document carries, which is what pins the error document to the shape the
/// runtime reports and nothing beside it.
fn members(document: &Value) -> BTreeSet<String> {
    document
        .as_object()
        .map(|members| members.keys().cloned().collect())
        .unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_handler_that_raised_fails_the_task_that_invoked_it() {
    let chain = Chain::new();
    let node = chain
        .given_function_in("raiser-node", NODE_RUNTIME, NODE_MODULE, NODE_HANDLER, None)
        .await;
    let python = chain
        .given_function_in(
            "raiser-python",
            PYTHON_RUNTIME,
            PYTHON_MODULE,
            PYTHON_HANDLER,
            None,
        )
        .await;

    for (index, row) in rows().into_iter().enumerate() {
        let Row {
            case,
            runtime,
            kind,
            order,
            outcome,
        } = row;
        let function = if runtime == NODE_RUNTIME { &node } else { &python };
        let described = chain
            .ran(
                case,
                &format!("raised-{index}"),
                &definition(function),
                &json!({ "kind": kind, "order": order }),
            )
            .await;
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

        match outcome {
            Outcome::Raised {
                error_type,
                message,
                trace,
            } => {
                // What a client reads off the execution: the status, the error code and
                // the cause.
                assert_eq!(
                    (
                        described.get("status").and_then(Value::as_str),
                        described.get("error").and_then(Value::as_str),
                    ),
                    (Some("FAILED"), Some(error_type)),
                    "case {case:?}: expected the execution to fail under {error_type:?}, got \
                     {described}"
                );
                let cause = described
                    .get("cause")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| panic!("case {case:?}: expected a cause on {described}"))
                    .to_string();

                // The cause is the run's error document itself, in the shape the
                // function's own runtime reports: the class, the message built from this
                // execution's input, and the stack under the member that runtime names.
                let document: Value = serde_json::from_str(&cause).unwrap_or_else(|error| {
                    panic!(
                        "case {case:?}: expected the cause to be the error document, got {error} \
                         for {cause:?}"
                    )
                });
                assert_eq!(
                    (
                        document.get("errorType").and_then(Value::as_str),
                        document.get("errorMessage").and_then(Value::as_str),
                    ),
                    (Some(error_type), Some(message.as_str())),
                    "case {case:?}: expected the document to name {error_type:?} and \
                     {message:?}, got {document}"
                );
                let expected_members = trace.members();
                assert_eq!(
                    members(&document),
                    expected_members,
                    "case {case:?}: expected exactly {expected_members:?} in {document}"
                );
                let stack: Vec<String> = document
                    .get(trace.member())
                    .and_then(Value::as_array)
                    .map(|lines| {
                        lines
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: expected a stack under {} in {document}", trace.member())
                    });
                match trace {
                    Trace::NodeErrorLine => assert_eq!(
                        stack.first().map(String::as_str),
                        Some(format!("{error_type}: {message}").as_str()),
                        "case {case:?}: expected the stack to open with the error's own line, got \
                         {stack:?}"
                    ),
                    Trace::NodeEmpty => assert!(
                        stack.is_empty(),
                        "case {case:?}: expected no stack for a thrown non-error, got {stack:?}"
                    ),
                    Trace::PythonFrames => assert!(
                        stack
                            .first()
                            .is_some_and(|frame| frame.contains(PYTHON_MODULE)),
                        "case {case:?}: expected the frames to name the handler's module, got \
                         {stack:?}"
                    ),
                }

                // The state's own failure event, naming the work the `Resource` asked
                // for and repeating that very error and cause.
                let detail = only_detail(&events, "TaskFailed", "taskFailedEventDetails", case);
                assert_eq!(
                    (
                        detail.get("resourceType").and_then(Value::as_str),
                        detail.get("resource").and_then(Value::as_str),
                    ),
                    (Some(RESOURCE_TYPE), Some(RESOURCE_ACTION)),
                    "case {case:?}: expected the failure to name {RESOURCE_TYPE}/\
                     {RESOURCE_ACTION}, got {detail}"
                );
                assert_eq!(
                    failure(&detail, case),
                    (error_type.to_string(), cause.clone()),
                    "case {case:?}: expected the task's failure to repeat the execution's, got \
                     {detail}"
                );
                assert_eq!(
                    count(&events, "TaskSucceeded"),
                    0,
                    "case {case:?}: expected no success for the state, got {:?}",
                    event_types(&events)
                );

                // And the execution's own, which repeats it unchanged.
                let ended =
                    only_detail(&events, "ExecutionFailed", "executionFailedEventDetails", case);
                assert_eq!(
                    failure(&ended, case),
                    (error_type.to_string(), cause),
                    "case {case:?}: expected the execution's failure to repeat the task's, got \
                     {ended}"
                );
            }
            Outcome::Returned => {
                assert_eq!(
                    described.get("status").and_then(Value::as_str),
                    Some("SUCCEEDED"),
                    "case {case:?}: expected the execution to succeed, got {described}"
                );
                assert!(
                    described.get("error").is_none() && described.get("cause").is_none(),
                    "case {case:?}: expected no error and no cause, got {described}"
                );
                let output: Value = described
                    .get("output")
                    .and_then(Value::as_str)
                    .map(|text| serde_json::from_str(text).expect("the output is JSON"))
                    .unwrap_or_else(|| panic!("case {case:?}: expected an output in {described}"));
                assert_eq!(
                    output.get("Payload"),
                    Some(&json!({ "order": order, "handled": true })),
                    "case {case:?}: expected the function's own answer as the payload, got \
                     {output}"
                );
                assert_eq!(
                    (
                        count(&events, "TaskSucceeded"),
                        count(&events, "TaskFailed"),
                        count(&events, "ExecutionFailed"),
                    ),
                    (1, 0, 0),
                    "case {case:?}: expected one success and no failure, got {:?}",
                    event_types(&events)
                );
            }
        }
    }
}
