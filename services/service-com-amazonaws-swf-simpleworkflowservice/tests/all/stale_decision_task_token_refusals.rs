// SPDX-License-Identifier: CC0-1.0

//! How a decision `taskToken` that no longer addresses an outstanding decision task is
//! refused, end to end over the service's own wire protocol (`awsJson1_0`, `POST /` with
//! `X-Amz-Target: SimpleWorkflowService.<Operation>`) through the same
//! `MultiServiceRouter` the emulator mounts in production.
//!
//! The rule every case here asserts: the fault NAMES the resource the lookup failed on.
//! A decision task token is a handle on the `DecisionTaskStarted` event it was minted
//! for, so a completion it can no longer be applied to is HTTP 400
//! `UnknownResourceFault` reporting either
//!
//! * the EXECUTION — `Unknown execution: WorkflowExecution=[workflowId=.., runId=..]`,
//!   built from the addressed run's own identifiers — when that run is no longer
//!   available, or
//! * the decision TASK — `Unknown decision task, scheduledEventId = <id>`, the
//!   `scheduledEventId` of the task the token addressed, read out of the stored history
//!   — while the run is still open; a token no started event of any stored run produces
//!   names no scheduling event either, and reports the `EventId` member's own modelled
//!   default of `0`.
//!
//! and that a refused completion records nothing.
//!
//! Rows vary what the rule generalises over: how the handle went stale (the task timed
//! out and was replaced, the task was completed, the run closed by each kind of closing
//! decision, the run was terminated out from under a task still in hand, the token
//! addresses nothing at all) and the shape of the ids the clause is built from (plain,
//! punctuated, non-ASCII, at the modelled maximum length).

use serde_json::{json, Value};

use crate::workflow_execution_lifecycle::{
    error_code, full_defaults, merge, seeded, string_at, whole_history, Emulator,
};

/// The workflow type every run in this suite is started with.
const WORKFLOW_TYPE: (&str, &str) = ("stale-wf", "1.0");

/// The task list every run of this suite schedules its decision tasks on.
const DECIDER_LIST: &str = "stale-deciders";

/// The expected clause for a run that is no longer available — the same form every
/// execution-scoped reader of this service reports a missing run with.
fn unknown_execution(workflow_id: &str, run_id: &str) -> String {
    format!("Unknown execution: WorkflowExecution=[workflowId={workflow_id}, runId={run_id}]")
}

/// The expected clause for a decision task that is no longer outstanding.
fn unknown_decision_task(scheduled_event_id: i64) -> String {
    format!("Unknown decision task, scheduledEventId = {scheduled_event_id}")
}

/// Register the domain and the workflow type this suite starts runs of.
async fn seed(emulator: &Emulator, domain: &str) {
    seeded(
        emulator,
        domain,
        domain,
        WORKFLOW_TYPE,
        full_defaults(DECIDER_LIST, "TERMINATE"),
    )
    .await;
}

/// Start one run, optionally overriding the decision task's own `startToCloseTimeout`.
/// Returns the minted `runId`.
async fn start(
    emulator: &Emulator,
    case: &str,
    domain: &str,
    workflow_id: &str,
    task_timeout: Option<&str>,
) -> String {
    let mut body = json!({
        "domain": domain,
        "workflowId": workflow_id,
        "workflowType": { "name": WORKFLOW_TYPE.0, "version": WORKFLOW_TYPE.1 },
    });
    if let Some(task_timeout) = task_timeout {
        merge(&mut body, json!({ "taskStartToCloseTimeout": task_timeout }));
    }
    let started = emulator.ok(case, "StartWorkflowExecution", &body).await;
    string_at(&started, "/runId")
}

/// Poll one decision task that must be waiting, and return
/// `(its taskToken, the scheduledEventId of the task it started)`.
async fn poll_task(emulator: &Emulator, case: &str, domain: &str) -> (String, i64) {
    let task = emulator
        .ok(
            case,
            "PollForDecisionTask",
            &json!({ "domain": domain, "taskList": { "name": DECIDER_LIST } }),
        )
        .await;
    let token = string_at(&task, "/taskToken");
    assert!(
        !token.is_empty(),
        "case {case:?}: expected a decision task waiting in {DECIDER_LIST:?}, got {task:?}"
    );
    let started_event_id = task
        .get("startedEventId")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("case {case:?}: no startedEventId in {task:?}"));
    let events = task
        .pointer("/events")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let scheduled_event_id = events
        .iter()
        .filter(|event| string_at(event, "/eventType") == "DecisionTaskStarted")
        .find(|event| {
            event
                .get("eventId")
                .and_then(Value::as_i64)
                .is_some_and(|id| id == started_event_id)
        })
        .and_then(|event| {
            event
                .pointer("/decisionTaskStartedEventAttributes/scheduledEventId")
                .and_then(Value::as_i64)
        })
        .unwrap_or_else(|| {
            panic!("case {case:?}: the handed-out task's own started event is in {events:?}")
        });
    (token, scheduled_event_id)
}

/// Complete one decision task, requiring a 200.
async fn respond(emulator: &Emulator, case: &str, token: &str, decisions: Value) {
    emulator
        .ok(
            case,
            "RespondDecisionTaskCompleted",
            &json!({ "taskToken": token, "decisions": decisions }),
        )
        .await;
}

/// Replay `token` and require HTTP 400 `UnknownResourceFault` with exactly
/// `expected_message`.
async fn refused_with(emulator: &Emulator, case: &str, token: &str, expected_message: &str) {
    let (status, parsed) = emulator
        .call(
            "RespondDecisionTaskCompleted",
            &json!({ "taskToken": token }),
        )
        .await;
    assert_eq!(
        (
            status,
            error_code(&parsed).as_str(),
            parsed.get("message").and_then(Value::as_str),
        ),
        (400, "UnknownResourceFault", Some(expected_message)),
        "case {case:?}: expected 400 UnknownResourceFault naming \
         {expected_message:?}, got {status} with {parsed:?}"
    );
}

/// The one closing decision a row answers its task with, as the decision list on the
/// wire, with the `closeStatus` it leaves the run in.
fn closing_decision(kind: &str) -> (Value, &'static str) {
    match kind {
        "CompleteWorkflowExecution" => (
            json!([{
                "decisionType": "CompleteWorkflowExecution",
                "completeWorkflowExecutionDecisionAttributes": { "result": "done" },
            }]),
            "COMPLETED",
        ),
        "FailWorkflowExecution" => (
            json!([{
                "decisionType": "FailWorkflowExecution",
                "failWorkflowExecutionDecisionAttributes": { "reason": "nope" },
            }]),
            "FAILED",
        ),
        "CancelWorkflowExecution" => (
            json!([{
                "decisionType": "CancelWorkflowExecution",
                "cancelWorkflowExecutionDecisionAttributes": {},
            }]),
            "CANCELED",
        ),
        other => panic!("{other} is not one of this suite's closing decisions"),
    }
}

/// One way a token can address a run that is no longer available: the closing decision
/// the decider answered its own task with, and the `workflowId` shape the clause is then
/// built from.
struct ClosedRow {
    case: &'static str,
    workflow_id: String,
    decision: &'static str,
}

#[tokio::test]
async fn a_token_whose_run_has_closed_names_the_execution() {
    let rows = [
        ClosedRow {
            case: "completed, a plain workflowId",
            workflow_id: "wf-completed".to_string(),
            decision: "CompleteWorkflowExecution",
        },
        ClosedRow {
            case: "failed, a punctuated workflowId",
            workflow_id: "order.42_v1-a".to_string(),
            decision: "FailWorkflowExecution",
        },
        ClosedRow {
            case: "canceled, a non-ASCII workflowId",
            workflow_id: "注文-ワークフロー#1".to_string(),
            decision: "CancelWorkflowExecution",
        },
        ClosedRow {
            case: "completed, a workflowId at the modelled maximum length",
            workflow_id: "w".repeat(256),
            decision: "CompleteWorkflowExecution",
        },
    ];

    let emulator = Emulator::new();
    let domain = "stale-closed";
    seed(&emulator, domain).await;
    for row in &rows {
        let case = row.case;
        let run_id = start(&emulator, case, domain, &row.workflow_id, None).await;
        let (token, _) = poll_task(&emulator, case, domain).await;
        let (decisions, close_status) = closing_decision(row.decision);
        respond(&emulator, case, &token, decisions).await;

        let before = whole_history(&emulator, domain, &row.workflow_id, &run_id).await;
        refused_with(
            &emulator,
            case,
            &token,
            &unknown_execution(&row.workflow_id, &run_id),
        )
        .await;
        let after = whole_history(&emulator, domain, &row.workflow_id, &run_id).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused replay records nothing, got {after:?}"
        );
        let described = emulator
            .ok(
                case,
                "DescribeWorkflowExecution",
                &json!({
                    "domain": domain,
                    "execution": { "workflowId": row.workflow_id, "runId": run_id },
                }),
            )
            .await;
        assert_eq!(
            (
                string_at(&described, "/executionInfo/executionStatus"),
                string_at(&described, "/executionInfo/closeStatus"),
            ),
            ("CLOSED".to_string(), close_status.to_string()),
            "case {case:?}: the run keeps the status its decider left it in, got {described:?}"
        );
    }
}

#[tokio::test]
async fn a_token_whose_run_was_terminated_names_the_execution() {
    // The run can also go out from under a task still in a decider's hands, which is the
    // same missing resource: the execution.
    let emulator = Emulator::new();
    let domain = "stale-terminated";
    seed(&emulator, domain).await;
    let workflow_id = "wf-terminated";
    let run_id = start(&emulator, "terminated", domain, workflow_id, None).await;
    let (token, _) = poll_task(&emulator, "terminated", domain).await;
    emulator
        .ok(
            "terminated",
            "TerminateWorkflowExecution",
            &json!({ "domain": domain, "workflowId": workflow_id, "runId": run_id }),
        )
        .await;
    refused_with(
        &emulator,
        "terminated",
        &token,
        &unknown_execution(workflow_id, &run_id),
    )
    .await;
}

#[tokio::test]
async fn a_token_whose_task_is_over_on_an_open_run_names_the_decision_task() {
    let emulator = Emulator::new();

    // Row 1: the task was COMPLETED and the run stays open, so the handle is a handle on
    // a task that is over.
    let domain = "stale-completed-task";
    seed(&emulator, domain).await;
    let workflow_id = "wf-answered";
    let run_id = start(&emulator, "answered", domain, workflow_id, None).await;
    let (token, scheduled_event_id) = poll_task(&emulator, "answered", domain).await;
    respond(&emulator, "answered", &token, json!([])).await;
    let before = whole_history(&emulator, domain, workflow_id, &run_id).await;
    refused_with(
        &emulator,
        "the token of a task already completed",
        &token,
        &unknown_decision_task(scheduled_event_id),
    )
    .await;
    let after = whole_history(&emulator, domain, workflow_id, &run_id).await;
    assert_eq!(
        after, before,
        "case \"the token of a task already completed\": a refused replay records nothing, \
         got {after:?}"
    );

    // Row 2: the task TIMED OUT and the service replaced it, so a different task of the
    // same open run is now outstanding and the stale handle names its own scheduling
    // event — not the live one's.
    let domain = "stale-timed-out-task";
    seed(&emulator, domain).await;
    let workflow_id = "wf-timed-out";
    // `startToCloseTimeout` is "The maximum duration for this decision task ... an
    // integer greater than or equal to 0", so the published lower bound is out the moment
    // the task starts.
    let _run_id = start(&emulator, "timed out", domain, workflow_id, Some("0")).await;
    let (stale, stale_scheduled) = poll_task(&emulator, "timed out", domain).await;
    let (live, live_scheduled) = poll_task(&emulator, "timed out", domain).await;
    assert_ne!(
        (stale.as_str(), stale_scheduled),
        (live.as_str(), live_scheduled),
        "case \"timed out\": the replacement task is a different task with its own handle"
    );
    refused_with(
        &emulator,
        "the token of a task the clock timed out",
        &stale,
        &unknown_decision_task(stale_scheduled),
    )
    .await;
}

#[tokio::test]
async fn a_token_that_addresses_no_started_task_at_all_names_no_scheduling_event() {
    let emulator = Emulator::new();
    let domain = "stale-nothing";
    seed(&emulator, domain).await;
    let _run_id = start(&emulator, "nothing", domain, "wf-live", None).await;
    let (live, _) = poll_task(&emulator, "nothing", domain).await;

    let mut rotated: Vec<char> = live.chars().collect();
    rotated[0] = if rotated[0] == 'a' { 'b' } else { 'a' };
    let rotated: String = rotated.into_iter().collect();
    for (case, token) in [
        ("a token of a single character", "x".to_string()),
        ("a token-shaped string of nothing", "0".repeat(live.len())),
        ("the live token with one character changed", rotated),
    ] {
        refused_with(&emulator, case, &token, &unknown_decision_task(0)).await;
    }

    // None of those refusals touched the live task, which still completes.
    respond(&emulator, "nothing", &live, json!([])).await;
}
