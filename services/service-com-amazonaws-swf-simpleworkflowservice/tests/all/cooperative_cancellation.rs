// SPDX-License-Identifier: CC0-1.0

//! The AWS cooperative-cancellation protocol, end to end over the service's own wire
//! protocol (`awsJson1_0`, `POST /` with `X-Amz-Target: SimpleWorkflowService.<Op>`)
//! through the same `MultiServiceRouter` the emulator mounts in production.
//!
//! The rule every case here asserts: an event a decider has to act on makes the
//! decider's reaction POLLABLE — recording it on an open run schedules a decision task
//! on that run's effective task list unless one is already outstanding, so the next
//! `PollForDecisionTask` of that list hands over a task whose `events` carry the new
//! event. A cancellation request is such an event ("It is up to the decider to take
//! appropriate actions when it receives an execution history with this event"), so the
//! run can then be closed by a `CancelWorkflowExecution` decision — `CLOSED`/`CANCELED`
//! with `WorkflowExecutionCanceled` last in the history.
//!
//! Rows vary what the rule generalises over: which requester delivered the cancellation
//! (the caller's own call with and without a `runId`, another run's
//! `RequestCancelExternalWorkflowExecution` decision), where the run's task list came
//! from (the workflow type's registered default or the start's own override), what the
//! previous decision task was drained with, and whether a decision task was already
//! outstanding when the request arrived.

use serde_json::{json, Value};

use crate::workflow_execution_lifecycle::{
    full_defaults, merge, seeded, string_at, whole_history, Emulator,
};

/// The workflow type every run in this suite is started with.
const WORKFLOW_TYPE: (&str, &str) = ("cancel-wf", "1.0");

/// The `defaultTaskList` the seeded workflow type carries, so a start that names no
/// `taskList` schedules its decision task there.
const DEFAULT_LIST: &str = "registered-cancel-deciders";

/// Register the domain and the one workflow type this suite starts runs of.
async fn seed(emulator: &Emulator, domain: &str) {
    seeded(
        emulator,
        domain,
        domain,
        WORKFLOW_TYPE,
        full_defaults(DEFAULT_LIST, "TERMINATE"),
    )
    .await;
}

/// Start one run, optionally overriding the task list. Returns the minted `runId`.
async fn start(
    emulator: &Emulator,
    case: &str,
    domain: &str,
    workflow_id: &str,
    task_list: Option<&str>,
) -> String {
    let mut body = json!({
        "domain": domain,
        "workflowId": workflow_id,
        "workflowType": { "name": WORKFLOW_TYPE.0, "version": WORKFLOW_TYPE.1 },
    });
    if let Some(task_list) = task_list {
        merge(&mut body, json!({ "taskList": { "name": task_list } }));
    }
    let started = emulator.ok(case, "StartWorkflowExecution", &body).await;
    string_at(&started, "/runId")
}

/// One `PollForDecisionTask` of `task_list`, requiring a 200.
async fn poll(emulator: &Emulator, case: &str, domain: &str, task_list: &str) -> Value {
    emulator
        .ok(
            case,
            "PollForDecisionTask",
            &json!({ "domain": domain, "taskList": { "name": task_list } }),
        )
        .await
}

/// Poll a decision task that MUST be waiting, and return `(the answer, its taskToken)`.
async fn poll_task(emulator: &Emulator, case: &str, domain: &str, task_list: &str) -> (Value, String) {
    let task = poll(emulator, case, domain, task_list).await;
    let token = string_at(&task, "/taskToken");
    assert!(
        !token.is_empty(),
        "case {case:?}: expected a decision task waiting in {task_list:?}, got {task:?}"
    );
    (task, token)
}

/// Complete one decision task with `decisions`, requiring a 200.
async fn respond(emulator: &Emulator, case: &str, token: &str, decisions: Value) {
    emulator
        .ok(
            case,
            "RespondDecisionTaskCompleted",
            &json!({ "taskToken": token, "decisions": decisions }),
        )
        .await;
}

/// How many decision tasks are waiting in one list, as `CountPendingDecisionTasks`
/// reports it.
async fn pending(emulator: &Emulator, case: &str, domain: &str, task_list: &str) -> i64 {
    let body = emulator
        .ok(
            case,
            "CountPendingDecisionTasks",
            &json!({ "domain": domain, "taskList": { "name": task_list } }),
        )
        .await;
    body.get("count")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("case {case:?}: expected a count in {body:?}"))
}

/// The `eventType` of each event of a list, in order.
fn types_of(events: &[Value]) -> Vec<String> {
    events
        .iter()
        .map(|event| string_at(event, "/eventType"))
        .collect()
}

/// Describe one run.
async fn describe(emulator: &Emulator, case: &str, domain: &str, workflow_id: &str, run_id: &str) -> Value {
    emulator
        .ok(
            case,
            "DescribeWorkflowExecution",
            &json!({
                "domain": domain,
                "execution": { "workflowId": workflow_id, "runId": run_id },
            }),
        )
        .await
}

/// One run that reaches the cancel request with NO decision task outstanding.
struct IdleRow {
    case: &'static str,
    workflow_id: &'static str,
    /// The task list the start names, or `None` to take the type's registered default.
    task_list: Option<&'static str>,
    /// Whether the cancel request names the `runId` or lets the service resolve it.
    pass_run_id: bool,
    /// The decisions the run's FIRST decision task is drained with, before the request.
    drain_with: Value,
}

#[tokio::test]
async fn a_cancel_request_on_an_idle_run_schedules_the_deciders_decision_task() {
    let rows = [
        IdleRow {
            case: "the registered default list, explicit runId",
            workflow_id: "wf-default-list",
            task_list: None,
            pass_run_id: true,
            drain_with: json!([]),
        },
        IdleRow {
            case: "a list named at start, runId resolved from the open run",
            workflow_id: "wf-override-list",
            task_list: Some("start-cancel-deciders"),
            pass_run_id: false,
            drain_with: json!([]),
        },
        IdleRow {
            case: "the previous task drained with a marker decision",
            workflow_id: "wf-marker-drained",
            task_list: Some("marker-cancel-deciders"),
            pass_run_id: true,
            drain_with: json!([{
                "decisionType": "RecordMarker",
                "recordMarkerDecisionAttributes": { "markerName": "checkpoint" },
            }]),
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let emulator = Emulator::new();
        let domain = format!("cancel-idle-{index}");
        seed(&emulator, &domain).await;
        let list = row.task_list.unwrap_or(DEFAULT_LIST);
        let run_id = start(&emulator, case, &domain, row.workflow_id, row.task_list).await;

        // The decider drains the decision task the start scheduled, which is what leaves
        // the run with none outstanding.
        let (_, first_token) = poll_task(&emulator, case, &domain, list).await;
        respond(&emulator, case, &first_token, row.drain_with.clone()).await;
        let idle = pending(&emulator, case, &domain, list).await;
        assert_eq!(
            idle, 0,
            "case {case:?}: the drained run holds no waiting decision task before the \
             cancel request, got {idle}"
        );

        let mut cancel = json!({ "domain": domain, "workflowId": row.workflow_id });
        if row.pass_run_id {
            merge(&mut cancel, json!({ "runId": run_id }));
        }
        emulator
            .ok(case, "RequestCancelWorkflowExecution", &cancel)
            .await;

        // The latch is recorded and the run stays OPEN — it is the decider that closes
        // it — and the decider's reaction is now waiting in the run's own task list.
        let described = describe(&emulator, case, &domain, row.workflow_id, &run_id).await;
        assert_eq!(
            (
                string_at(&described, "/executionInfo/executionStatus"),
                described
                    .pointer("/executionInfo/cancelRequested")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            ),
            ("OPEN".to_string(), true),
            "case {case:?}: a cancel request latches on the open run, got {described:?}"
        );
        let waiting = pending(&emulator, case, &domain, list).await;
        assert_eq!(
            waiting, 1,
            "case {case:?}: the cancel request schedules exactly one decision task on \
             {list:?}, got {waiting}"
        );

        let (task, cancel_token) = poll_task(&emulator, case, &domain, list).await;
        let handed = task
            .pointer("/events")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: no events in {task:?}"));
        assert!(
            types_of(&handed).contains(&"WorkflowExecutionCancelRequested".to_string()),
            "case {case:?}: the decision task the request scheduled carries the \
             cancel-requested event, got {:?}",
            types_of(&handed)
        );
        assert_eq!(
            string_at(&task, "/workflowExecution/runId"),
            run_id,
            "case {case:?}: the task belongs to the run the request addressed, got {task:?}"
        );

        // The decider answers it the documented way, and the run closes CANCELED.
        respond(
            &emulator,
            case,
            &cancel_token,
            json!([{
                "decisionType": "CancelWorkflowExecution",
                "cancelWorkflowExecutionDecisionAttributes": { "details": "asked to stop" },
            }]),
        )
        .await;
        let closed = describe(&emulator, case, &domain, row.workflow_id, &run_id).await;
        assert_eq!(
            (
                string_at(&closed, "/executionInfo/executionStatus"),
                string_at(&closed, "/executionInfo/closeStatus"),
            ),
            ("CLOSED".to_string(), "CANCELED".to_string()),
            "case {case:?}: the decider's cancel decision closes the run, got {closed:?}"
        );
        let history = whole_history(&emulator, &domain, row.workflow_id, &run_id).await;
        let kinds = types_of(&history);
        assert_eq!(
            kinds.last().map(String::as_str),
            Some("WorkflowExecutionCanceled"),
            "case {case:?}: the cancellation is the last event of the run, got {kinds:?}"
        );
        assert_eq!(
            kinds
                .iter()
                .filter(|kind| *kind == "WorkflowExecutionCancelRequested")
                .count(),
            1,
            "case {case:?}: the request is a latch, so the run carries one of them, got \
             {kinds:?}"
        );
    }
}

#[tokio::test]
async fn a_cancel_request_never_makes_a_second_decision_task_outstanding() {
    // SWF keeps at most one decision task per execution: the two ways a run can already
    // have one — waiting in its list, or in a decider's hands — and in both the event the
    // request records reaches the decider through the task that already exists.
    let emulator = Emulator::new();
    let domain = "cancel-busy";
    seed(&emulator, domain).await;
    let list = DEFAULT_LIST;

    for (case, poll_first) in [
        ("a task still waiting in the list", false),
        ("a task already in a decider's hands", true),
    ] {
        let workflow_id = if poll_first { "wf-in-hand" } else { "wf-waiting" };
        let run_id = start(&emulator, case, domain, workflow_id, None).await;
        let mut held: Option<String> = None;
        if poll_first {
            let (_, token) = poll_task(&emulator, case, domain, list).await;
            held = Some(token);
        }

        emulator
            .ok(
                case,
                "RequestCancelWorkflowExecution",
                &json!({ "domain": domain, "workflowId": workflow_id, "runId": run_id }),
            )
            .await;

        let history = whole_history(&emulator, domain, workflow_id, &run_id).await;
        let scheduled = types_of(&history)
            .iter()
            .filter(|kind| *kind == "DecisionTaskScheduled")
            .count();
        assert_eq!(
            scheduled, 1,
            "case {case:?}: a run that already has a decision task gets no second, got {:?}",
            types_of(&history)
        );

        match held {
            // The decider holding the task has not seen the cancel request, so completing
            // its task schedules the next one, which carries it.
            Some(token) => {
                respond(&emulator, case, &token, json!([])).await;
                let (task, _) = poll_task(&emulator, case, domain, list).await;
                let handed = task
                    .pointer("/events")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                assert!(
                    types_of(&handed).contains(&"WorkflowExecutionCancelRequested".to_string()),
                    "case {case:?}: the next decision task carries the request the decider \
                     had not seen, got {:?}",
                    types_of(&handed)
                );
            }
            // The task was still in the list: the one task there is the one that carries
            // the request.
            None => {
                let (task, _) = poll_task(&emulator, case, domain, list).await;
                let handed = task
                    .pointer("/events")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                assert!(
                    types_of(&handed).contains(&"WorkflowExecutionCancelRequested".to_string()),
                    "case {case:?}: the waiting task carries the request, got {:?}",
                    types_of(&handed)
                );
            }
        }
    }
}

#[tokio::test]
async fn a_cancel_delivered_by_another_runs_decision_also_schedules_the_targets_task() {
    // The same rule, reached through the other requester the service models: a
    // `RequestCancelExternalWorkflowExecution` decision of one run delivers the event to
    // the target, which must likewise become pollable for the target's decider.
    let emulator = Emulator::new();
    let domain = "cancel-external";
    seed(&emulator, domain).await;
    let requester_list = "requester-deciders";
    let target_list = "target-deciders";
    let target_run = start(&emulator, "external", domain, "wf-target", Some(target_list)).await;
    let _requester_run = start(&emulator, "external", domain, "wf-requester", Some(requester_list)).await;

    // The target's own first decision task is drained, so the delivery is what schedules
    // the next one.
    let (_, target_token) = poll_task(&emulator, "external", domain, target_list).await;
    respond(&emulator, "external", &target_token, json!([])).await;

    let (_, requester_token) = poll_task(&emulator, "external", domain, requester_list).await;
    respond(
        &emulator,
        "external",
        &requester_token,
        json!([{
            "decisionType": "RequestCancelExternalWorkflowExecution",
            "requestCancelExternalWorkflowExecutionDecisionAttributes": {
                "workflowId": "wf-target",
                "runId": target_run,
            },
        }]),
    )
    .await;

    let waiting = pending(&emulator, "external", domain, target_list).await;
    assert_eq!(
        waiting, 1,
        "case \"external\": the delivered request schedules the target's decision task, \
         got {waiting}"
    );
    let (task, _) = poll_task(&emulator, "external", domain, target_list).await;
    let handed = task
        .pointer("/events")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert!(
        types_of(&handed).contains(&"WorkflowExecutionCancelRequested".to_string()),
        "case \"external\": the target's new task carries the cancel-requested event, got {:?}",
        types_of(&handed)
    );
}
