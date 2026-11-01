// SPDX-License-Identifier: CC0-1.0

//! The `WorkflowExecutionInfo` record and the open counts every execution reader
//! reports, end to end over the service's own wire protocol (`awsJson1_0`, `POST /` with
//! `X-Amz-Target: SimpleWorkflowService.<Operation>`), through the same
//! `MultiServiceRouter` the emulator mounts in production. The harness is the one
//! `workflow_execution_lifecycle` already builds.
//!
//! Three rules are asserted here.
//!
//! 1. ONE projection of a stored run serves every reader of it, and it reports the close
//!    the run's own history records: `lastClosedEventId` is the `eventId` of the event
//!    that closed the run and `lastClosedTimestamp` the instant that event is stamped at,
//!    while a run that is still open has closed at no event (the member's modelled
//!    default of 0) and at no instant; a run no operation of this service can reopen is at
//!    its first generation (`reopenGenerationId` 0) and not `reopenable`.
//! 2. `cancelRequested` reports a cancellation still outstanding against a RUNNING
//!    execution, so a run that is no longer OPEN reports none whatever was asked of it
//!    while it ran — the cancel-requested event stays in its history.
//! 3. `openCounts` reports the work of the run whose own status is OPEN, and the event
//!    that closes one piece of work is what takes it out of its counter: closing the RUN
//!    retracts nothing, so a decision task scheduled and never answered is still counted
//!    on the response that reports the run closed.
//!
//! The rows vary what rule 1 and rule 2 generalise over: how the run closed (an
//! operator's terminate, each of the three closing decisions a decider can take, and the
//! clock spending the run's own budget), how many events its history holds before the
//! close, whether a cancellation was requested first, and which reader the record is read
//! through (`DescribeWorkflowExecution`, `ListOpenWorkflowExecutions`,
//! `ListClosedWorkflowExecutions`). Every expected value is computed from the run's own
//! history, never hardcoded.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_swf_simpleworkflowservice::ops::types::EVENT_TYPE_VALUES;

use super::workflow_execution_lifecycle::{
    full_defaults, i64_at, merge, seeded, string_at, whole_history, Emulator,
};

/// The workflow type every run in this suite is started with.
const WORKFLOW_TYPE: (&str, &str) = ("record-wf", "1.0");

/// The `defaultTaskList` that type carries, so every run's decision tasks wait there.
const DECIDER_LIST: &str = "record-deciders";

/// `oldestDate` is required on a visibility listing, and this is the oldest instant the
/// documented Unix-time form can name, so it excludes nothing.
const EPOCH_SECONDS: f64 = 0.0;

/// The every-`WorkflowExecutionInfo` members this suite is about, as the wire names them.
const PROJECTED: &[&str] = &[
    "lastClosedEventId",
    "reopenGenerationId",
    "reopenable",
    "cancelRequested",
];

/// Register the domain and the workflow type the runs are started with. `budget` is the
/// `defaultExecutionStartToCloseTimeout` the type registers, in whole seconds.
async fn seed(emulator: &Emulator, domain: &str, budget: &str) {
    let mut defaults = full_defaults(DECIDER_LIST, "TERMINATE");
    merge(
        &mut defaults,
        json!({ "defaultExecutionStartToCloseTimeout": budget }),
    );
    seeded(emulator, domain, domain, WORKFLOW_TYPE, defaults).await;
}

/// Start one run of the seeded type and return its minted `runId`.
async fn start(emulator: &Emulator, case: &str, domain: &str, workflow_id: &str) -> String {
    let started = emulator
        .ok(
            case,
            "StartWorkflowExecution",
            &json!({
                "domain": domain,
                "workflowId": workflow_id,
                "workflowType": { "name": WORKFLOW_TYPE.0, "version": WORKFLOW_TYPE.1 },
            }),
        )
        .await;
    string_at(&started, "/runId")
}

/// Read one run back through `DescribeWorkflowExecution`.
async fn describe(
    emulator: &Emulator,
    case: &str,
    domain: &str,
    workflow_id: &str,
    run_id: &str,
) -> Value {
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

/// The `executionInfos` entry a visibility listing reports for `run_id`, or `None`.
async fn listed(
    emulator: &Emulator,
    case: &str,
    operation: &str,
    domain: &str,
    run_id: &str,
) -> Option<Value> {
    let body = emulator
        .ok(
            case,
            operation,
            &json!({ "domain": domain, "startTimeFilter": { "oldestDate": EPOCH_SECONDS } }),
        )
        .await;
    body.pointer("/executionInfos")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no executionInfos in {body:?}"))
        .iter()
        .find(|info| {
            info.pointer("/execution/runId")
                .and_then(Value::as_str)
                .is_some_and(|listed| listed == run_id)
        })
        .cloned()
}

/// Drive the run's decider once: poll its decision task and answer it with `decisions`.
async fn decide(emulator: &Emulator, case: &str, domain: &str, decisions: Value) {
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
    emulator
        .ok(
            case,
            "RespondDecisionTaskCompleted",
            &json!({ "taskToken": token, "decisions": decisions }),
        )
        .await;
}

/// The `eventId` of the one event of `events` whose type is `event_type`.
fn event_id_of(case: &str, events: &[Value], event_type: &str) -> i64 {
    assert!(
        EVENT_TYPE_VALUES.contains(&event_type),
        "case {case:?}: {event_type:?} must be a modelled EventType"
    );
    let matching: Vec<&Value> = events
        .iter()
        .filter(|event| {
            event
                .pointer("/eventType")
                .and_then(Value::as_str)
                .is_some_and(|recorded| recorded == event_type)
        })
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "case {case:?}: exactly one {event_type} event must be recorded, got {events:?}"
    );
    i64_at(matching[0], "/eventId")
}

/// Assert the four every-run members of one `WorkflowExecutionInfo` against the run's own
/// history: `closing_event_type` is `None` for a run that is still open.
fn assert_projection(
    case: &str,
    where_from: &str,
    info: &Value,
    events: &[Value],
    closing_event_type: Option<&str>,
    cancel_requested: bool,
) {
    for member in PROJECTED {
        assert!(
            info.get(member).is_some(),
            "case {case:?} ({where_from}): every WorkflowExecutionInfo reports {member}, \
             got {info:?}"
        );
    }
    let expected_closed_at = closing_event_type.map(|event_type| event_id_of(case, events, event_type));
    assert_eq!(
        i64_at(info, "/lastClosedEventId"),
        expected_closed_at.unwrap_or(0),
        "case {case:?} ({where_from}): lastClosedEventId is the id of the event that \
         closed the run, 0 while it is open, got {info:?}"
    );
    let close_timestamp = info.get("closeTimestamp").and_then(Value::as_f64);
    assert_eq!(
        info.get("lastClosedTimestamp").and_then(Value::as_f64),
        close_timestamp,
        "case {case:?} ({where_from}): lastClosedTimestamp is the instant the run closed, \
         absent while it is open, got {info:?}"
    );
    assert_eq!(
        close_timestamp.is_some(),
        closing_event_type.is_some(),
        "case {case:?} ({where_from}): only a closed run carries a close instant, \
         got {info:?}"
    );
    assert_eq!(
        (
            i64_at(info, "/reopenGenerationId"),
            info.get("reopenable").and_then(Value::as_bool)
        ),
        (0, Some(false)),
        "case {case:?} ({where_from}): no operation of this service reopens a run, so \
         every run is at its first generation and none is reopenable, got {info:?}"
    );
    assert_eq!(
        info.get("cancelRequested").and_then(Value::as_bool),
        Some(cancel_requested),
        "case {case:?} ({where_from}): cancelRequested reports a cancellation still \
         outstanding against a running execution, got {info:?}"
    );
}

/// One run, closed the way this row closes it.
struct CloseCase {
    name: &'static str,
    workflow_id: &'static str,
    /// Whether a cancellation is requested against the run before it closes.
    cancel_first: bool,
    /// The decisions the decider answers its first decision task with; empty for a run
    /// closed without a decider.
    decisions: Value,
    /// Whether the row terminates the run through `TerminateWorkflowExecution`.
    terminate: bool,
    /// The event type the close records, from which the expected `lastClosedEventId` is
    /// read out of the run's own history.
    closing_event_type: &'static str,
}

#[tokio::test]
async fn every_execution_record_reports_the_close_its_own_history_recorded() {
    let emulator = Emulator::new();
    let domain = "closes";
    // A budget far longer than this suite takes, so no row below is closed by the clock;
    // the clock's own close has its own test at the end of this suite.
    seed(&emulator, domain, "300").await;

    // One row per way a run can leave the OPEN status under a caller's control: the
    // operator's terminate and each of the three closing decisions a decider can take.
    // `ContinueAsNewWorkflowExecution` closes a run too, but it starts a replacement run
    // of the same `workflowId`, which `list_closed_workflow_executions` covers.
    let cases = [
        CloseCase {
            name: "terminated by an operator",
            workflow_id: "wf-terminated",
            cancel_first: false,
            decisions: json!([]),
            terminate: true,
            closing_event_type: "WorkflowExecutionTerminated",
        },
        CloseCase {
            name: "terminated after a cancellation was requested",
            workflow_id: "wf-cancel-then-terminate",
            cancel_first: true,
            decisions: json!([]),
            terminate: true,
            closing_event_type: "WorkflowExecutionTerminated",
        },
        CloseCase {
            name: "completed by its decider",
            workflow_id: "wf-completed",
            cancel_first: false,
            decisions: json!([{
                "decisionType": "CompleteWorkflowExecution",
                "completeWorkflowExecutionDecisionAttributes": { "result": "done" },
            }]),
            terminate: false,
            closing_event_type: "WorkflowExecutionCompleted",
        },
        CloseCase {
            name: "failed by its decider",
            workflow_id: "wf-failed",
            cancel_first: false,
            decisions: json!([{
                "decisionType": "FailWorkflowExecution",
                "failWorkflowExecutionDecisionAttributes": {
                    "reason": "nope",
                    "details": "the decider gave up",
                },
            }]),
            terminate: false,
            closing_event_type: "WorkflowExecutionFailed",
        },
        CloseCase {
            name: "cancelled by its decider after a cancellation was requested",
            workflow_id: "wf-canceled",
            cancel_first: true,
            decisions: json!([{
                "decisionType": "CancelWorkflowExecution",
                "cancelWorkflowExecutionDecisionAttributes": { "details": "as asked" },
            }]),
            terminate: false,
            closing_event_type: "WorkflowExecutionCanceled",
        },
    ];

    for case in &cases {
        let run_id = start(&emulator, case.name, domain, case.workflow_id).await;

        // While the run is OPEN: it has closed at no event, and the listing of OPEN
        // executions reports the same record the describe does.
        let open_events = whole_history(&emulator, domain, case.workflow_id, &run_id).await;
        let described = describe(&emulator, case.name, domain, case.workflow_id, &run_id).await;
        assert_projection(
            case.name,
            "DescribeWorkflowExecution, open",
            described
                .pointer("/executionInfo")
                .unwrap_or_else(|| panic!("case {:?}: no executionInfo", case.name)),
            &open_events,
            None,
            false,
        );
        let open_listed = listed(
            &emulator,
            case.name,
            "ListOpenWorkflowExecutions",
            domain,
            &run_id,
        )
        .await
        .unwrap_or_else(|| panic!("case {:?}: the open run must be listed", case.name));
        assert_projection(
            case.name,
            "ListOpenWorkflowExecutions",
            &open_listed,
            &open_events,
            None,
            false,
        );

        if case.cancel_first {
            emulator
                .ok(
                    case.name,
                    "RequestCancelWorkflowExecution",
                    &json!({
                        "domain": domain,
                        "workflowId": case.workflow_id,
                        "runId": run_id,
                    }),
                )
                .await;
            let requested =
                describe(&emulator, case.name, domain, case.workflow_id, &run_id).await;
            assert_eq!(
                requested
                    .pointer("/executionInfo/cancelRequested")
                    .and_then(Value::as_bool),
                Some(true),
                "case {:?}: the request is outstanding while the run is open, got \
                 {requested:?}",
                case.name
            );
        }

        if case.decisions.as_array().is_some_and(|rows| !rows.is_empty()) {
            decide(&emulator, case.name, domain, case.decisions.clone()).await;
        }
        if case.terminate {
            emulator
                .ok(
                    case.name,
                    "TerminateWorkflowExecution",
                    &json!({
                        "domain": domain,
                        "workflowId": case.workflow_id,
                        "runId": run_id,
                    }),
                )
                .await;
        }

        // Once CLOSED: the same four members, now reporting the closing event of this
        // run's own history, through both readers that can see a closed run.
        let closed_events = whole_history(&emulator, domain, case.workflow_id, &run_id).await;
        let described = describe(&emulator, case.name, domain, case.workflow_id, &run_id).await;
        let info = described
            .pointer("/executionInfo")
            .unwrap_or_else(|| panic!("case {:?}: no executionInfo", case.name));
        assert_eq!(
            string_at(info, "/executionStatus"),
            "CLOSED",
            "case {:?}: the row closes the run, got {described:?}",
            case.name
        );
        assert_projection(
            case.name,
            "DescribeWorkflowExecution, closed",
            info,
            &closed_events,
            Some(case.closing_event_type),
            false,
        );
        let closed_listed = listed(
            &emulator,
            case.name,
            "ListClosedWorkflowExecutions",
            domain,
            &run_id,
        )
        .await
        .unwrap_or_else(|| panic!("case {:?}: the closed run must be listed", case.name));
        assert_projection(
            case.name,
            "ListClosedWorkflowExecutions",
            &closed_listed,
            &closed_events,
            Some(case.closing_event_type),
            false,
        );

        if case.cancel_first {
            // What the run was asked is history, and closing it changed no event.
            let cancel_requested_events = closed_events
                .iter()
                .filter(|event| {
                    event
                        .pointer("/eventType")
                        .and_then(Value::as_str)
                        .is_some_and(|event_type| event_type == "WorkflowExecutionCancelRequested")
                })
                .count();
            assert_eq!(
                cancel_requested_events, 1,
                "case {:?}: the cancel-requested event stays in the closed run's \
                 history, got {closed_events:?}",
                case.name
            );
        }
    }
}

#[tokio::test]
async fn a_run_the_clock_closed_reports_the_timeout_event_it_recorded() {
    let emulator = Emulator::new();
    let domain = "clock";
    // The member's own published lower bound ("an integer greater than or equal to 0"),
    // so the service's own start-to-close sweep is what closes this run.
    seed(&emulator, domain, "0").await;
    let run_id = start(&emulator, "clock", domain, "wf-timed-out").await;

    // The sweep is driven by the wall clock and materialised by the next read, so the
    // wait only has to cross the deadline.
    tokio::time::sleep(Duration::from_millis(900)).await;

    let described = describe(&emulator, "clock", domain, "wf-timed-out", &run_id).await;
    let events = whole_history(&emulator, domain, "wf-timed-out", &run_id).await;
    let info = described
        .pointer("/executionInfo")
        .unwrap_or_else(|| panic!("case \"clock\": no executionInfo in {described:?}"));
    assert_eq!(
        (
            string_at(info, "/executionStatus"),
            string_at(info, "/closeStatus")
        ),
        ("CLOSED".to_string(), "TIMED_OUT".to_string()),
        "case \"clock\": an unattended run is closed by the service, got {described:?}"
    );
    assert_projection(
        "clock",
        "DescribeWorkflowExecution, timed out",
        info,
        &events,
        Some("WorkflowExecutionTimedOut"),
        false,
    );
}

#[tokio::test]
async fn the_open_counts_report_the_work_each_closing_event_left_outstanding() {
    let emulator = Emulator::new();
    let domain = "counts";
    seed(&emulator, domain, "300").await;

    // A run is started with one decision task scheduled. Terminating it answers no
    // decision task, so that task is still outstanding at the instant the run closes.
    let terminated = start(&emulator, "unanswered", domain, "wf-unanswered").await;
    let before = describe(&emulator, "unanswered", domain, "wf-unanswered", &terminated).await;
    assert_eq!(
        i64_at(&before, "/openCounts/openDecisionTasks"),
        1,
        "case \"unanswered\": a started run has its first decision task open, got \
         {before:?}"
    );
    emulator
        .ok(
            "unanswered",
            "TerminateWorkflowExecution",
            &json!({ "domain": domain, "workflowId": "wf-unanswered", "runId": terminated }),
        )
        .await;
    let after = describe(&emulator, "unanswered", domain, "wf-unanswered", &terminated).await;
    assert_eq!(
        (
            string_at(&after, "/executionInfo/executionStatus"),
            i64_at(&after, "/openCounts/openDecisionTasks")
        ),
        ("CLOSED".to_string(), 1),
        "case \"unanswered\": the response that reports the run closed still counts the \
         decision task nothing answered, got {after:?}"
    );

    // A run whose decider DID answer its decision task has none outstanding: the
    // `DecisionTaskCompleted` event is what took it out of the counter, not the close.
    let answered = start(&emulator, "answered", domain, "wf-answered").await;
    decide(
        &emulator,
        "answered",
        domain,
        json!([{
            "decisionType": "CompleteWorkflowExecution",
            "completeWorkflowExecutionDecisionAttributes": { "result": "done" },
        }]),
    )
    .await;
    let closed = describe(&emulator, "answered", domain, "wf-answered", &answered).await;
    assert_eq!(
        (
            string_at(&closed, "/executionInfo/closeStatus"),
            i64_at(&closed, "/openCounts/openDecisionTasks")
        ),
        ("COMPLETED".to_string(), 0),
        "case \"answered\": the decision task its decider completed is no longer open, \
         got {closed:?}"
    );
    for counter in [
        "openActivityTasks",
        "openTimers",
        "openChildWorkflowExecutions",
        "openLambdaFunctions",
    ] {
        assert_eq!(
            i64_at(&closed, &format!("/openCounts/{counter}")),
            0,
            "case \"answered\": this run never had any {counter}, got {closed:?}"
        );
    }
}
