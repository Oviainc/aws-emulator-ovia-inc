// SPDX-License-Identifier: CC0-1.0

//! End-to-end test for the WHOLE activity-worker hand-off an activity client performs:
//! create the activity, run a state machine whose `Task` state names it, long-poll for
//! the task, and then delete the activity under the worker that holds the token —
//! driven over this service's own wire protocol (`awsJson1_0`, `POST /` with
//! `X-Amz-Target: AWSStepFunctions.<Operation>`) through the production
//! `MultiServiceRouter`.
//!
//! The rules under test, in the order one client exercises them:
//!
//! 1. an execution that entered a `Task` state whose `Resource` is an activity ARN has
//!    a task queued on that activity, so the next `GetActivityTask` is handed a
//!    non-empty `taskToken` together with the input the state computed;
//! 2. `DeleteActivity` is accepted while a worker still holds an outstanding token —
//!    its model declares no in-use refusal and no output members, so the answer is an
//!    empty 200 — and the deleted ARN is then reported absent (`ActivityDoesNotExist`,
//!    400) by a describe;
//! 3. the token the worker still holds names a task with no activity to be answered
//!    for, so EVERY call that reports on it is refused with `TaskDoesNotExist` (400);
//! 4. the execution is left exactly as it was by those refusals and reaches its own
//!    terminal `FAILED` on the bound its state declared, rather than hanging.
//!
//! The harness (`Emulator`, `create_machine`, `start_execution`, `describe_execution`)
//! is the one `state_machine_lifecycle` builds, and the activity helpers are
//! `activity_tasks`'; `tests/all.rs` is this binary's crate root, so both are reached
//! through `crate::<module>`.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_sfn_awsstepfunctions::state::{
    asl::TIMEOUT_ERROR, resource_error_message, ACTIVITY_DOES_NOT_EXIST,
};

use crate::activity_tasks::{activity_definition, claimed, create_activity};
use crate::state_machine_lifecycle::{
    create_machine, describe_execution, error_code, start_execution, Emulator,
};

/// The bound the scheduling state declares on its task, in seconds, and long enough
/// that every refusal below is made while the task is still outstanding.
const TASK_BOUND_SECONDS: u32 = 1;

/// Long enough that the one-second bound the scheduling state declared has run out.
const PAST_THE_TASK_BOUND: Duration = Duration::from_millis(1_300);

/// The name the worker polls under, which the service only logs.
const WORKER: &str = "hand-off-worker";

/// One call a holder of the token can make on it, and the request that makes it.
struct Reply {
    case: &'static str,
    operation: &'static str,
    request: fn(&str) -> Value,
}

/// Every call that reports on an outstanding task by its token.
fn replies() -> Vec<Reply> {
    vec![
        Reply {
            case: "SendTaskSuccess, which would carry the task's result",
            operation: "SendTaskSuccess",
            request: |token| json!({ "taskToken": token, "output": r#"{"done":true}"# }),
        },
        Reply {
            case: "SendTaskFailure, which would carry the worker's error",
            operation: "SendTaskFailure",
            request: |token| json!({ "taskToken": token, "error": "WorkerFailed" }),
        },
        Reply {
            case: "SendTaskHeartbeat, which would only report progress",
            operation: "SendTaskHeartbeat",
            request: |token| json!({ "taskToken": token }),
        },
    ]
}

/// The activity-worker pattern end to end: the poll is handed the task the execution
/// scheduled, the activity is deleted under the worker, and every call the worker can
/// still make with the token it holds is refused for the activity that is gone — while
/// the execution reaches its own terminal status on the bound its state declared.
#[tokio::test]
async fn a_worker_is_handed_the_task_and_refused_once_its_activity_is_deleted() {
    let emulator = Emulator::new();

    // One activity, one state machine naming it, and one execution per reply the
    // worker will try: each holds its own token, since a token names one task.
    let mut outstanding: Vec<(&str, &str, Value, String, String)> = Vec::new();
    for (index, reply) in replies().into_iter().enumerate() {
        let Reply {
            case,
            operation,
            request,
        } = reply;
        let activity = create_activity(&emulator, &format!("hand-off-{index}")).await;
        let definition =
            activity_definition(&activity, json!({ "TimeoutSeconds": TASK_BOUND_SECONDS }));
        let machine =
            create_machine(&emulator, &format!("hand-off-machine-{index}"), &definition).await;
        let execution = start_execution(&emulator, &machine, "only", r#"{"k":"v"}"#).await;

        // Rule 1: the running execution queued a task on the activity, so the poll is
        // handed a token and the input the state computed — the execution's own input
        // here, the state applying no data flow to it.
        let (token, scheduled) = claimed(&emulator, case, &activity, Some(WORKER)).await;
        assert_eq!(
            scheduled,
            json!({ "k": "v" }),
            "case {case:?}: expected the input the state scheduled, got {scheduled:?}"
        );

        // Rule 2: the activity is deleted while that task is outstanding. The
        // operation models no in-use refusal, and `DeleteActivityOutput` has no
        // members, so the answer is an empty 200.
        let deleted = emulator
            .ok("DeleteActivity", &json!({ "activityArn": activity }))
            .await;
        assert!(
            deleted
                .as_object()
                .is_some_and(|members| members.is_empty()),
            "case {case:?}: expected an empty DeleteActivity body, got {deleted:?}"
        );

        let (status, answer) = emulator
            .call("DescribeActivity", &json!({ "activityArn": activity }))
            .await;
        assert_eq!(
            (status, error_code(&answer).as_str()),
            (400, "ActivityDoesNotExist"),
            "case {case:?}: expected the deleted activity to be reported absent, got {status} \
             with {answer:?}"
        );
        // The refusal names the ARN the request carried, as this service words a
        // refusal about a resource of the request.
        assert_eq!(
            answer.get("message").and_then(Value::as_str),
            Some(resource_error_message(ACTIVITY_DOES_NOT_EXIST, &activity).as_str()),
            "case {case:?}: expected the refusal to name the deleted activity, got {answer:?}"
        );

        outstanding.push((case, operation, request(&token), activity, execution));
    }

    // Rule 3: every call on the token the worker still holds is refused for the
    // activity that is gone — never answered as a reply nobody can record.
    for (case, operation, body, activity, _execution) in &outstanding {
        let (status, answer) = emulator.call(operation, body).await;
        let code = error_code(&answer);
        assert_eq!(
            (status, code.as_str()),
            (400, "TaskDoesNotExist"),
            "case {case:?}: expected a 400 TaskDoesNotExist for the held token, got {status} \
             {code} with {answer:?}"
        );
        // The refusal names the activity the orphaned task belonged to.
        assert!(
            answer
                .get("message")
                .and_then(Value::as_str)
                .is_some_and(|message| message.contains(activity.as_str())),
            "case {case:?}: expected the refusal to name the deleted activity, got {answer:?}"
        );
    }

    // Rule 4: nothing above moved the execution; the bound its state declared is what
    // ends it, and it ends FAILED with the language's own timeout error rather than
    // waiting for a worker that can no longer answer.
    std::thread::sleep(PAST_THE_TASK_BOUND);
    for (case, _operation, _body, _activity, execution) in &outstanding {
        let described = describe_execution(&emulator, execution).await;
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("FAILED"),
            "case {case:?}: expected the execution to reach its own terminal status, got \
             {described:?}"
        );
        assert_eq!(
            described.get("error").and_then(Value::as_str),
            Some(TIMEOUT_ERROR),
            "case {case:?}: expected the error {TIMEOUT_ERROR}, got {described:?}"
        );
    }
}
