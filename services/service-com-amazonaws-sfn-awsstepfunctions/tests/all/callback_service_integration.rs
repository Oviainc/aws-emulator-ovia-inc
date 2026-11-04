// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the callback (`.waitForTaskToken`) service-integration
//! pattern, driven over both services' own wire protocols through the production
//! `MultiServiceRouter` with Step Functions and DynamoDB mounted on ONE engine.
//!
//! The rule under test: a `Task` state whose `Resource` names an integration with the
//! `.waitForTaskToken` suffix MAKES that integration's own call — carrying the
//! `taskToken` the state's parameters read from the context object (`$$.Task.Token`)
//! — and the task is then not finished by that call's answer: the execution reports
//! `RUNNING` until a `SendTaskSuccess` or `SendTaskFailure` quoting that token
//! arrives, or until the state's `TimeoutSeconds`/`HeartbeatSeconds` run out, which
//! fails the state with `States.Timeout`. What the answer reports is the state's own
//! outcome, so a success is placed by the state's result-side data flow and a
//! reported failure is handed to the catcher the state names.
//!
//! Every token asserted here is one that really TRAVELLED: the tests read it back out
//! of the far end of the call — the DynamoDB item the task wrote, the child execution
//! it started — through that service's own API, and answer with exactly that value.
//!
//! The harness is `task_integrations`' `Chain` (Step Functions and DynamoDB over one
//! engine); `tests/all.rs` is this binary's crate root, so it is reached through
//! `crate::task_integrations`.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_sfn_awsstepfunctions::state::{
    asl::{EXPRESS_CALLBACK_UNSUPPORTED, RUNTIME_ERROR, TIMEOUT_ERROR},
    read_task_token, EVENT_TASK_SUBMITTED, EVENT_TASK_SUCCEEDED, EVENT_TASK_TIMED_OUT,
};

use crate::state_machine_lifecycle::{string, PAST_THE_EXECUTION_DELAY, ROLE_ARN};
use crate::task_integrations::Chain;

/// The table every callback task below writes its token into.
const TABLE: &str = "callbacks";

/// The `output` a `SendTaskSuccess` reports for a callback task below.
const REPLY_OUTPUT: &str = r#"{"approved":true,"by":"the worker outside"}"#;

/// The child definition a nested-execution callback starts: it runs to its own end,
/// because what the parent waits for is the TOKEN and not the child.
const CHILD: &str =
    r#"{"StartAt":"Done","States":{"Done":{"Type":"Pass","Result":{"ok":true},"End":true}}}"#;

/// How long a test waits for an outcome a `TimeoutSeconds`/`HeartbeatSeconds` of one
/// second makes due: that second, plus the settle delay the outcome is owed after.
const PAST_A_ONE_SECOND_BOUND: Duration =
    Duration::from_millis(1_400).saturating_add(PAST_THE_EXECUTION_DELAY);

/// The integrations the tests drive the pattern over, and how the token each one
/// carried is read back out of the far end of its call.
///
/// Two of them, so what is asserted is the PATTERN rather than one service's call: a
/// DynamoDB write, whose request carries the token as an item attribute, and a nested
/// Step Functions execution, whose request carries it as the child's input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Integration {
    /// `arn:aws:states:::dynamodb:putItem.waitForTaskToken`: the item the call wrote
    /// holds the token under `token`.
    PutItem,
    /// `arn:aws:states:::states:startExecution.waitForTaskToken`: the child execution
    /// the call started was given the token in its own input.
    StartExecution,
}

impl Integration {
    fn resource(self) -> &'static str {
        match self {
            Self::PutItem => "arn:aws:states:::dynamodb:putItem.waitForTaskToken",
            Self::StartExecution => "arn:aws:states:::states:startExecution.waitForTaskToken",
        }
    }
}

/// The `Parameters` document of a callback task over `integration`, for an execution
/// whose own key is `key`.
///
/// `$$.Task.Token` is how the Amazon States Language hands the token to the work the
/// state calls, so it is a member of the document the payload template builds — and
/// the member it goes in belongs to the INVOKED operation (`PutItem`'s `Item`,
/// `StartExecution`'s `input`).
fn parameters(integration: Integration, key: &str, child_arn: &str) -> Value {
    match integration {
        Integration::PutItem => json!({
            "TableName": TABLE,
            "Item": {
                "pk": { "S": key },
                "token": { "S.$": "$$.Task.Token" },
            },
        }),
        Integration::StartExecution => json!({
            "stateMachineArn": child_arn,
            "name": key,
            "input": { "token.$": "$$.Task.Token" },
        }),
    }
}

/// A one-state definition whose `Task` waits for its token, with `extra` merged into
/// the state (the bounds, a `ResultPath`, a `Catch`).
fn definition(integration: Integration, key: &str, child_arn: &str, extra: &Value) -> Value {
    let mut state = json!({
        "Type": "Task",
        "Resource": integration.resource(),
        "Parameters": parameters(integration, key, child_arn),
        "End": true,
    });
    let members = state.as_object_mut().expect("the state is an object");
    if let Some(extra) = extra.as_object() {
        for (member, value) in extra {
            members.insert(member.clone(), value.clone());
        }
    }
    json!({ "StartAt": "WaitForCallback", "States": { "WaitForCallback": state } })
}

/// The history of `execution_arn`, as the API answers it.
async fn events(chain: &Chain, execution_arn: &str) -> Vec<Value> {
    let answered = chain
        .sfn(
            "GetExecutionHistory",
            &json!({ "executionArn": execution_arn }),
        )
        .await;
    answered
        .get("events")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected events in {answered:?}"))
        .clone()
}

/// The event types the history of `execution_arn` carries, in order.
async fn event_types(chain: &Chain, execution_arn: &str) -> Vec<String> {
    events(chain, execution_arn)
        .await
        .iter()
        .filter_map(|event| event.get("type").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

/// The document the callback task's call ANSWERED, as its `TaskSubmitted` event
/// reports it: "the response from a resource when a task has started".
async fn submitted(chain: &Chain, case: &str, execution_arn: &str) -> Value {
    let event = events(chain, execution_arn)
        .await
        .into_iter()
        .find(|event| event.get("type").and_then(Value::as_str) == Some(EVENT_TASK_SUBMITTED))
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a {EVENT_TASK_SUBMITTED} event in the history")
        });
    let details = event
        .get("taskSubmittedEventDetails")
        .unwrap_or_else(|| panic!("case {case:?}: expected the submission's details in {event:?}"));
    serde_json::from_str(&string(details, "output"))
        .unwrap_or_else(|why| panic!("case {case:?}: the submission's output is JSON: {why}"))
}

/// The token the call of a callback task over `integration` DELIVERED, read back out
/// of the far end of that call.
async fn delivered_token(
    chain: &Chain,
    case: &str,
    integration: Integration,
    key: &str,
    execution_arn: &str,
) -> String {
    match integration {
        Integration::PutItem => {
            let item = chain
                .item(TABLE, key)
                .await
                .unwrap_or_else(|| panic!("case {case:?}: the callback task wrote no item"));
            item.get("token")
                .and_then(|attribute| attribute.get("S"))
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: expected a token in the item {item:?}"))
                .to_string()
        }
        Integration::StartExecution => {
            // The child the call started is named by what that call answered, which
            // the submission event carries: the ARN is read off it rather than built.
            let reported = submitted(chain, case, execution_arn).await;
            let child_arn = string(&reported, "ExecutionArn");
            let described = chain.describe(&child_arn).await;
            let input: Value = serde_json::from_str(&string(&described, "input"))
                .unwrap_or_else(|why| panic!("case {case:?}: the child's input is JSON: {why}"));
            input
                .get("token")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected a token in the child's input {input:?}")
                })
                .to_string()
        }
    }
}

/// One row of the answer table: the case, the integration the pattern runs over, how
/// the token is answered, and what the execution must end as.
struct Answer {
    case: &'static str,
    integration: Integration,
    /// `Some((error, cause))` answers with `SendTaskFailure`; `None` answers with
    /// `SendTaskSuccess`.
    failure: Option<(Option<&'static str>, Option<&'static str>)>,
    status: &'static str,
}

// ---------------------------------------------------------------------------
// The call is made, the token travels, and the answer finishes the task
// ---------------------------------------------------------------------------

/// A callback task makes its call, suspends at the token that call carried, and is
/// finished by the `SendTask*` that quotes it.
///
/// Each row asserts the whole pattern: the far end of the call really holds the token
/// (so the call was made and the token was delivered), the token is one this service
/// issued and names the execution waiting at the state, the execution is `RUNNING`
/// while nothing has answered, the history records the submission, and the answer is
/// the state's own outcome — a success placed as the state's result, a failure
/// reported with exactly the `error` and `cause` the answer carried. The rows differ
/// along the integration the pattern runs over and along what the answer reports,
/// including an answer that names neither member.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_callback_task_is_finished_by_the_answer_to_the_token_it_handed_out() {
    let rows = [
        Answer {
            case: "a DynamoDB write answered with a success",
            integration: Integration::PutItem,
            failure: None,
            status: "SUCCEEDED",
        },
        Answer {
            case: "a nested execution answered with a success",
            integration: Integration::StartExecution,
            failure: None,
            status: "SUCCEEDED",
        },
        Answer {
            case: "a DynamoDB write answered with a failure naming both members",
            integration: Integration::PutItem,
            failure: Some((Some("WorkerReportedFailure"), Some("the worker said no"))),
            status: "FAILED",
        },
        Answer {
            case: "a nested execution answered with a failure naming neither member",
            integration: Integration::StartExecution,
            failure: Some((None, None)),
            status: "FAILED",
        },
    ];

    let chain = Chain::new();
    chain.create_table(TABLE).await;
    let child_machine = chain
        .create(
            "callback-child",
            &serde_json::from_str::<Value>(CHILD).expect("the child definition is JSON"),
        )
        .await;

    for (index, row) in rows.iter().enumerate() {
        let Answer {
            case,
            integration,
            failure,
            status,
        } = row;
        let key = format!("call-{index}");
        let machine = chain
            .create(
                &format!("callback-{index}"),
                &definition(*integration, &key, &child_machine, &json!({})),
            )
            .await;
        let execution = chain
            .start(&machine, &key, &json!({ "from": "the start" }))
            .await;

        // The call was made and the token went with it: the far end holds a token
        // this service issued, and it names the execution waiting at the state.
        let token = delivered_token(&chain, case, *integration, &key, &execution).await;
        let named = read_task_token(&token).unwrap_or_else(|| {
            panic!("case {case:?}: expected a token this service issued, got {token:?}")
        });
        assert_eq!(
            named, execution,
            "case {case:?}: expected the token to name {execution}, got {named}"
        );

        // Nothing has answered yet, so the task is not finished: the execution is
        // still RUNNING, past the delay its own outcome would have settled after, and
        // the history records what the call answered as the task's SUBMISSION.
        std::thread::sleep(PAST_THE_EXECUTION_DELAY);
        let waiting = chain.describe(&execution).await;
        assert_eq!(
            waiting.get("status").and_then(Value::as_str),
            Some("RUNNING"),
            "case {case:?}: expected RUNNING while the token is unanswered, got {waiting:?}"
        );
        let present = event_types(&chain, &execution).await;
        assert!(
            present.iter().any(|event| event == EVENT_TASK_SUBMITTED),
            "case {case:?}: expected a {EVENT_TASK_SUBMITTED} event, got {present:?}"
        );
        assert!(
            !present.iter().any(|event| event == EVENT_TASK_SUCCEEDED),
            "case {case:?}: an unanswered task has not succeeded, got {present:?}"
        );

        // The answer finishes the task, and what it reported is the state's outcome.
        match failure {
            None => {
                chain
                    .sfn(
                        "SendTaskSuccess",
                        &json!({ "taskToken": token, "output": REPLY_OUTPUT }),
                    )
                    .await;
            }
            Some((error, cause)) => {
                let mut request = json!({ "taskToken": token });
                let members = request.as_object_mut().expect("the request is an object");
                if let Some(error) = error {
                    members.insert("error".to_string(), json!(error));
                }
                if let Some(cause) = cause {
                    members.insert("cause".to_string(), json!(cause));
                }
                chain.sfn("SendTaskFailure", &request).await;
            }
        }
        std::thread::sleep(PAST_THE_EXECUTION_DELAY);
        let settled = chain.describe(&execution).await;
        assert_eq!(
            settled.get("status").and_then(Value::as_str),
            Some(*status),
            "case {case:?}: expected {status}, got {settled:?}"
        );
        match failure {
            None => {
                let output: Value = serde_json::from_str(&string(&settled, "output"))
                    .unwrap_or_else(|why| panic!("case {case:?}: the output is JSON: {why}"));
                assert_eq!(
                    output.get("approved").and_then(Value::as_bool),
                    Some(true),
                    "case {case:?}: expected the answer's own document as the result, got \
                     {output:?}"
                );
            }
            Some((error, cause)) => {
                assert_eq!(
                    settled.get("error").and_then(Value::as_str),
                    *error,
                    "case {case:?}: expected the error the answer named, got {settled:?}"
                );
                assert_eq!(
                    settled.get("cause").and_then(Value::as_str),
                    *cause,
                    "case {case:?}: expected the cause the answer named, got {settled:?}"
                );
            }
        }
        // One token answers one task: a second answer resolves nothing.
        let (_status, code) = chain
            .sfn_err(
                "SendTaskSuccess",
                &json!({ "taskToken": token, "output": "{}" }),
            )
            .await;
        assert_eq!(
            code, "TaskTimedOut",
            "case {case:?}: expected a closed task to refuse a second answer, got {code}"
        );
    }
}

// ---------------------------------------------------------------------------
// The state's own data flow and its Catch
// ---------------------------------------------------------------------------

/// A callback state's `Retry` is the STATE's budget, not one answer's: each reported
/// failure a retrier answers for makes the call again with a token of its own, and the
/// state fails once that retrier has spent its `MaxAttempts`.
///
/// The budget has to survive the suspension — the walk that continues from an answer
/// is a different walk from the one that suspended — so this drives the whole cycle:
/// the first answer is retried (a SECOND, different token is delivered and the
/// execution is still RUNNING), and the answer to that one exhausts the retrier and
/// fails the execution with exactly the error the answer reported. The rows differ
/// along how many attempts the retrier allows, so what is pinned is that the state
/// makes `MaxAttempts` further calls and then no more.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_callback_states_retry_spends_one_budget_across_the_answers() {
    // (case, the retrier's MaxAttempts)
    let rows: [(&str, u64); 2] = [
        ("a retrier allowing one further attempt", 1),
        ("a retrier allowing two", 2),
    ];
    // The error the answers report and the retrier names; the retrier waits no time,
    // so each attempt's outcome is due as soon as the answer is recorded.
    const REPORTED: &str = "WorkerSaidNo";

    let chain = Chain::new();
    chain.create_table(TABLE).await;

    for (index, (case, max_attempts)) in rows.into_iter().enumerate() {
        let key = format!("retry-{index}");
        let machine = chain
            .create(
                &format!("retry-{index}"),
                &definition(
                    Integration::PutItem,
                    &key,
                    "",
                    &json!({
                        "Retry": [{
                            "ErrorEquals": [REPORTED],
                            "MaxAttempts": max_attempts,
                            "IntervalSeconds": 0,
                        }],
                    }),
                ),
            )
            .await;
        let execution = chain.start(&machine, &key, &json!({})).await;

        // Every attempt the retrier allows: the answer is retried, so the state makes
        // its call AGAIN — a token of its own, delivered to the same far end — and the
        // execution is still RUNNING.
        let mut token = delivered_token(&chain, case, Integration::PutItem, &key, &execution).await;
        for attempt in 0..max_attempts {
            chain
                .sfn(
                    "SendTaskFailure",
                    &json!({ "taskToken": token, "error": REPORTED }),
                )
                .await;
            std::thread::sleep(PAST_THE_EXECUTION_DELAY);
            let running = chain.describe(&execution).await;
            assert_eq!(
                running.get("status").and_then(Value::as_str),
                Some("RUNNING"),
                "case {case:?}: expected RUNNING while attempt {attempt} is retried, got \
                 {running:?}"
            );
            let retried =
                delivered_token(&chain, case, Integration::PutItem, &key, &execution).await;
            assert_ne!(
                retried, token,
                "case {case:?}: attempt {attempt} must hand out a token of its own"
            );
            token = retried;
        }

        // The retrier has spent its attempts, so the next reported failure is the
        // state's own and the execution fails with exactly what the answer reported.
        chain
            .sfn(
                "SendTaskFailure",
                &json!({ "taskToken": token, "error": REPORTED, "cause": "no more attempts" }),
            )
            .await;
        std::thread::sleep(PAST_THE_EXECUTION_DELAY);
        let settled = chain.describe(&execution).await;
        assert_eq!(
            settled.get("status").and_then(Value::as_str),
            Some("FAILED"),
            "case {case:?}: expected FAILED once the retrier is spent, got {settled:?}"
        );
        assert_eq!(
            settled.get("error").and_then(Value::as_str),
            Some(REPORTED),
            "case {case:?}: expected the error the answer named, got {settled:?}"
        );
        assert_eq!(
            settled.get("cause").and_then(Value::as_str),
            Some("no more attempts"),
            "case {case:?}: expected the cause the answer named, got {settled:?}"
        );
    }
}

/// What the answer reports is the STATE's result and the STATE's failure: a success is
/// placed by the state's result-side data flow, and a reported failure is handed to
/// the catcher the state names.
///
/// The rows differ along which member of the state decides what happens to the answer
/// — a `ResultPath` that merges the result into the data the state was entered with,
/// an `OutputPath` that selects out of that, and a `Catch` on the reported error — so
/// what is pinned is that the answer re-enters the state's own data flow rather than
/// being reported as the execution's output directly.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_answer_re_enters_the_states_own_data_flow() {
    // (case, the extra members of the state, the error a failure reports, the output
    // the execution must end with)
    let rows: [(&str, Value, Option<&str>, Value); 3] = [
        (
            "a ResultPath merging the result into the state's input",
            json!({ "ResultPath": "$.callback" }),
            None,
            json!({
                "from": "the start",
                "callback": { "approved": true, "by": "the worker outside" },
            }),
        ),
        (
            "an OutputPath selecting out of the result",
            json!({ "ResultPath": "$.callback", "OutputPath": "$.callback.by" }),
            None,
            json!("the worker outside"),
        ),
        (
            "a Catch answering for the error the failure reported",
            json!({
                "Catch": [{
                    "ErrorEquals": ["WorkerSaidNo"],
                    "Next": "Recovered",
                    "ResultPath": "$.why",
                }],
            }),
            Some("WorkerSaidNo"),
            json!({ "from": "the start", "why": { "Error": "WorkerSaidNo" } }),
        ),
    ];

    let chain = Chain::new();
    chain.create_table(TABLE).await;

    for (index, (case, extra, reported, expected)) in rows.into_iter().enumerate() {
        let key = format!("flow-{index}");
        let mut machine_definition = definition(Integration::PutItem, &key, "", &extra);
        // A `Catch` transitions to a state of its own, so the definition carries the
        // catcher's target beside the task — and the task then names a `Next` instead
        // of ending the execution itself.
        if extra.get("Catch").is_some() {
            let states = machine_definition
                .get_mut("States")
                .and_then(Value::as_object_mut)
                .expect("the definition carries States");
            states.insert(
                "Recovered".to_string(),
                json!({ "Type": "Pass", "End": true }),
            );
            let task = states
                .get_mut("WaitForCallback")
                .and_then(Value::as_object_mut)
                .expect("the definition carries the task");
            task.remove("End");
            task.insert("Next".to_string(), json!("Recovered"));
        }
        let machine = chain
            .create(&format!("flow-{index}"), &machine_definition)
            .await;
        let execution = chain
            .start(&machine, &key, &json!({ "from": "the start" }))
            .await;
        let token = delivered_token(&chain, case, Integration::PutItem, &key, &execution).await;

        match reported {
            None => {
                chain
                    .sfn(
                        "SendTaskSuccess",
                        &json!({ "taskToken": token, "output": REPLY_OUTPUT }),
                    )
                    .await;
            }
            Some(error) => {
                chain
                    .sfn(
                        "SendTaskFailure",
                        &json!({ "taskToken": token, "error": error }),
                    )
                    .await;
            }
        }
        std::thread::sleep(PAST_THE_EXECUTION_DELAY);
        let settled = chain.describe(&execution).await;
        assert_eq!(
            settled.get("status").and_then(Value::as_str),
            Some("SUCCEEDED"),
            "case {case:?}: expected SUCCEEDED, got {settled:?}"
        );
        let output: Value = serde_json::from_str(&string(&settled, "output"))
            .unwrap_or_else(|why| panic!("case {case:?}: the output is JSON: {why}"));
        assert_eq!(
            output, expected,
            "case {case:?}: expected {expected:?}, got {output:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The bounds a callback task is answered inside
// ---------------------------------------------------------------------------

/// A callback task whose token is NOT answered in time fails the state with ASL's own
/// `States.Timeout`, naming the bound that ran out — and a token answered inside its
/// bounds finishes the task, heartbeats included.
///
/// The rows differ along which bound the state declares: the whole task's
/// `TimeoutSeconds`, the gap between heartbeats its `HeartbeatSeconds` allows, and a
/// state declaring both whose token is answered inside them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_callback_token_that_is_not_answered_in_time_times_the_task_out() {
    // (case, the bounds the state declares, whether the token is answered)
    let rows: [(&str, Value, bool); 3] = [
        (
            "the whole task's TimeoutSeconds",
            json!({ "TimeoutSeconds": 1 }),
            false,
        ),
        (
            "the gap its HeartbeatSeconds allows between heartbeats",
            json!({ "HeartbeatSeconds": 1 }),
            false,
        ),
        (
            "both bounds, with the token answered inside them",
            json!({ "TimeoutSeconds": 30, "HeartbeatSeconds": 30 }),
            true,
        ),
    ];

    let chain = Chain::new();
    chain.create_table(TABLE).await;

    for (index, (case, bounds, answered)) in rows.into_iter().enumerate() {
        let key = format!("bound-{index}");
        let machine = chain
            .create(
                &format!("bound-{index}"),
                &definition(Integration::PutItem, &key, "", &bounds),
            )
            .await;
        let execution = chain.start(&machine, &key, &json!({})).await;
        let token = delivered_token(&chain, case, Integration::PutItem, &key, &execution).await;

        if answered {
            // "This action resets the Heartbeat clock": a heartbeat is accepted for a
            // callback task's token exactly as it is for an activity's.
            chain
                .sfn("SendTaskHeartbeat", &json!({ "taskToken": token }))
                .await;
            chain
                .sfn(
                    "SendTaskSuccess",
                    &json!({ "taskToken": token, "output": REPLY_OUTPUT }),
                )
                .await;
            std::thread::sleep(PAST_THE_EXECUTION_DELAY);
            let settled = chain.describe(&execution).await;
            assert_eq!(
                settled.get("status").and_then(Value::as_str),
                Some("SUCCEEDED"),
                "case {case:?}: expected SUCCEEDED, got {settled:?}"
            );
            continue;
        }

        std::thread::sleep(PAST_A_ONE_SECOND_BOUND);
        let settled = chain.describe(&execution).await;
        assert_eq!(
            settled.get("status").and_then(Value::as_str),
            Some("FAILED"),
            "case {case:?}: expected FAILED, got {settled:?}"
        );
        assert_eq!(
            settled.get("error").and_then(Value::as_str),
            Some(TIMEOUT_ERROR),
            "case {case:?}: expected {TIMEOUT_ERROR}, got {settled:?}"
        );
        // The cause names the bound that ran out and the state that declared it, both
        // read from the definition.
        let bound = match bounds.get("HeartbeatSeconds") {
            Some(_) => "HeartbeatSeconds",
            None => "TimeoutSeconds",
        };
        let cause = settled
            .get("cause")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            cause.contains(bound) && cause.contains("WaitForCallback"),
            "case {case:?}: expected a cause naming {bound} and the state, got {cause:?}"
        );
        let present = event_types(&chain, &execution).await;
        assert!(
            present.iter().any(|event| event == EVENT_TASK_TIMED_OUT),
            "case {case:?}: expected a {EVENT_TASK_TIMED_OUT} event, got {present:?}"
        );
        // A token whose task the clock closed answers nothing afterwards.
        let (_status, code) = chain
            .sfn_err(
                "SendTaskSuccess",
                &json!({ "taskToken": token, "output": "{}" }),
            )
            .await;
        assert_eq!(
            code, "TaskTimedOut",
            "case {case:?}: expected a closed task to refuse an answer, got {code}"
        );
    }
}

// ---------------------------------------------------------------------------
// Where the execution cannot wait for a token
// ---------------------------------------------------------------------------

/// A callback task the execution cannot WAIT at is not run: the state fails with
/// ASL's `States.Runtime` and a cause that says what was not run, and no call is made
/// for it.
///
/// The rows are the places an execution has nowhere to wait: an EXPRESS workflow,
/// which waits for no answer from outside, and a `Map` iteration or a `Parallel`
/// branch, each of which is walked to its own end inside the enclosing state's turn.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_callback_task_the_execution_cannot_wait_at_is_not_run() {
    let inner = json!({
        "Type": "Task",
        "Resource": "arn:aws:states:::dynamodb:putItem.waitForTaskToken",
        "Parameters": {
            "TableName": TABLE,
            "Item": { "pk": { "S": "nested" }, "token": { "S.$": "$$.Task.Token" } },
        },
        "End": true,
    });
    // (case, the state machine type, the definition, the key no item may appear under,
    //  what the cause must state)
    let rows: [(&str, &str, Value, &str, &str); 3] = [
        (
            "an EXPRESS workflow, which waits for no answer from outside",
            "EXPRESS",
            definition(Integration::PutItem, "express", "", &json!({})),
            "express",
            EXPRESS_CALLBACK_UNSUPPORTED,
        ),
        (
            "a Map iteration, which is walked to its own end",
            "STANDARD",
            json!({
                "StartAt": "Each",
                "States": {
                    "Each": {
                        "Type": "Map",
                        "ItemsPath": "$.items",
                        "ItemProcessor": { "StartAt": "Inner", "States": { "Inner": inner } },
                        "End": true,
                    }
                }
            }),
            "nested",
            "nothing was run for it",
        ),
        (
            "a Parallel branch, which is walked to its own end",
            "STANDARD",
            json!({
                "StartAt": "Both",
                "States": {
                    "Both": {
                        "Type": "Parallel",
                        "Branches": [{ "StartAt": "Inner", "States": { "Inner": inner } }],
                        "End": true,
                    }
                }
            }),
            "nested",
            "nothing was run for it",
        ),
    ];

    let chain = Chain::new();
    chain.create_table(TABLE).await;

    for (index, (case, machine_type, machine_definition, key, stated)) in
        rows.into_iter().enumerate()
    {
        let created = chain
            .sfn(
                "CreateStateMachine",
                &json!({
                    "name": format!("unwaitable-{index}"),
                    "definition": machine_definition.to_string(),
                    "roleArn": ROLE_ARN,
                    "type": machine_type,
                }),
            )
            .await;
        let machine = string(&created, "stateMachineArn");

        // An EXPRESS execution is reported by neither `DescribeExecution` nor
        // `GetExecutionHistory`, so it is started SYNCHRONOUSLY: that call answers the
        // outcome itself. A STANDARD one is started and then described.
        let outcome = if machine_type == "EXPRESS" {
            chain
                .sfn(
                    "StartSyncExecution",
                    &json!({
                        "stateMachineArn": machine,
                        "name": format!("unwaitable-{index}"),
                        "input": json!({ "items": [1] }).to_string(),
                    }),
                )
                .await
        } else {
            let execution = chain
                .start(&machine, &format!("unwaitable-{index}"), &json!({ "items": [1] }))
                .await;
            std::thread::sleep(PAST_THE_EXECUTION_DELAY);
            chain.describe(&execution).await
        };
        assert_eq!(
            outcome.get("status").and_then(Value::as_str),
            Some("FAILED"),
            "case {case:?}: expected FAILED, got {outcome:?}"
        );
        assert_eq!(
            outcome.get("error").and_then(Value::as_str),
            Some(RUNTIME_ERROR),
            "case {case:?}: expected {RUNTIME_ERROR}, got {outcome:?}"
        );
        let cause = outcome
            .get("cause")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            cause.contains(stated),
            "case {case:?}: expected a cause stating {stated:?}, got {cause:?}"
        );
        assert!(
            chain.item(TABLE, key).await.is_none(),
            "case {case:?}: no call may be made for a task nothing can wait at"
        );
    }
}
