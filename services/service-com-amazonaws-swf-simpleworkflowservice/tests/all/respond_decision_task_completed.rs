// SPDX-License-Identifier: CC0-1.0

//! `RespondDecisionTaskCompleted`, end to end over the service's own wire protocol
//! (`awsJson1_0`, `POST /` with `X-Amz-Target: SimpleWorkflowService.<Operation>`),
//! dispatched through the same `MultiServiceRouter` the emulator mounts in production.
//!
//! The rule every case here asserts: the call closes the ONE outstanding decision task
//! its `taskToken` addresses — recording a `DecisionTaskCompleted` event that names the
//! task's scheduled and started events and carries the `executionContext` attached to it
//! — and then applies each decision in order to the run's stored state, recording for it
//! the history event that decision's own documentation gives it, or the matching
//! `*Failed` event carrying the modelled cause when the decision cannot be applied to
//! the run as it stands. The answer itself is empty: everything the call produced is read
//! back out of state afterwards, through `GetWorkflowExecutionHistory`,
//! `DescribeWorkflowExecution` and the `Count*` readers.
//!
//! Every row varies what the rule generalises over — which decision was made, whether
//! its target exists, where the values it needs came from (the decision or the
//! registration-time default), how many decisions one call carries, the shape of the ids
//! and the size of the payloads, and the durations (including the published bounds) — and
//! every expected value is computed by the test from what it seeded, never hardcoded
//! from one recording.

use serde_json::{json, Value};
use service_com_amazonaws_swf_simpleworkflowservice::ops::types::{
    CLOSE_STATUS_VALUES, DECISION_TASK_TIMEOUT_TYPE_VALUES, EVENT_TYPE_VALUES,
};
use service_com_amazonaws_swf_simpleworkflowservice::state::MAX_EXECUTIONS_PER_DOMAIN;

use super::workflow_execution_lifecycle::{
    error_code, full_defaults, i64_at, merge, seeded, string_at, whole_history, Emulator,
};

/// The workflow type every run in this suite is started with.
const WORKFLOW_TYPE: (&str, &str) = ("decider-wf", "1.0");

/// The `defaultTaskList` the seeded workflow type carries, so a start that names no
/// `taskList` schedules its decision task there.
const DECIDER_LIST: &str = "deciders";

/// The activity type the `ScheduleActivityTask` rows schedule tasks of.
const ACTIVITY_TYPE: (&str, &str) = ("send-email", "2.0");

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

/// Register one activity type carrying `defaults`.
async fn register_activity_type(
    emulator: &Emulator,
    case: &str,
    domain: &str,
    activity_type: (&str, &str),
    defaults: Value,
) {
    let mut body = json!({
        "domain": domain,
        "name": activity_type.0,
        "version": activity_type.1,
    });
    merge(&mut body, defaults);
    emulator.ok(case, "RegisterActivityType", &body).await;
}

/// Every default an activity type can carry, so a `ScheduleActivityTask` decision needs
/// no overrides at all.
fn activity_defaults(task_list: &str) -> Value {
    json!({
        "defaultTaskList": { "name": task_list },
        "defaultTaskScheduleToStartTimeout": "60",
        "defaultTaskScheduleToCloseTimeout": "600",
        "defaultTaskStartToCloseTimeout": "300",
    })
}

/// Start one run and return the minted `runId`. `extra` carries the optional start
/// members a row exercises (`lambdaRole`, `taskList`, `input`, `tagList`).
async fn start(emulator: &Emulator, case: &str, domain: &str, workflow_id: &str, extra: Value) -> String {
    let mut body = json!({
        "domain": domain,
        "workflowId": workflow_id,
        "workflowType": { "name": WORKFLOW_TYPE.0, "version": WORKFLOW_TYPE.1 },
    });
    merge(&mut body, extra);
    let started = emulator.ok(case, "StartWorkflowExecution", &body).await;
    string_at(&started, "/runId")
}

/// Poll one decision task off `task_list` and return the whole answer.
async fn poll(emulator: &Emulator, case: &str, domain: &str, task_list: &str) -> Value {
    emulator
        .ok(
            case,
            "PollForDecisionTask",
            &json!({ "domain": domain, "taskList": { "name": task_list } }),
        )
        .await
}

/// Poll a decision task and return just its `taskToken`.
async fn poll_token(emulator: &Emulator, case: &str, domain: &str, task_list: &str) -> String {
    let task = poll(emulator, case, domain, task_list).await;
    let token = string_at(&task, "/taskToken");
    assert!(
        !token.is_empty(),
        "case {case:?}: expected a decision task waiting in {task_list:?}, got {task:?}"
    );
    token
}

/// Complete one decision task with `decisions`, requiring a 200 and an EMPTY answer.
async fn respond(emulator: &Emulator, case: &str, token: &str, decisions: Value, extra: Value) {
    let mut body = json!({ "taskToken": token, "decisions": decisions });
    merge(&mut body, extra);
    let answered = emulator
        .ok(case, "RespondDecisionTaskCompleted", &body)
        .await;
    assert_eq!(
        answered,
        json!({}),
        "case {case:?}: RespondDecisionTaskCompleted's output shape is Unit, so the body \
         must be empty, got {answered:?}"
    );
}

/// The `eventType` of every event of a run's history, oldest first.
async fn event_types(emulator: &Emulator, domain: &str, workflow_id: &str, run_id: &str) -> Vec<String> {
    whole_history(emulator, domain, workflow_id, run_id)
        .await
        .iter()
        .map(|event| string_at(event, "/eventType"))
        .collect()
}

/// The one event of `event_type` a run's history holds, or a panic naming what it holds
/// instead.
async fn event_of_type(
    emulator: &Emulator,
    case: &str,
    domain: &str,
    workflow_id: &str,
    run_id: &str,
    event_type: &str,
) -> Value {
    let events = whole_history(emulator, domain, workflow_id, run_id).await;
    let matching: Vec<&Value> = events
        .iter()
        .filter(|event| string_at(event, "/eventType") == event_type)
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "case {case:?}: expected exactly one {event_type} event, got {:?}",
        events
            .iter()
            .map(|event| string_at(event, "/eventType"))
            .collect::<Vec<_>>()
    );
    matching[0].clone()
}

/// `DescribeWorkflowExecution` for one run.
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

/// Assert that every `eventType` this suite expects is one the model declares, so a
/// typo in a row is a failure rather than a silently unmatched name.
fn modelled_event_type(case: &str, event_type: &str) {
    assert!(
        EVENT_TYPE_VALUES.contains(&event_type),
        "case {case:?}: {event_type:?} is not one of the modelled EventType values"
    );
}

// ---------------------------------------------------------------------------
// One decision at a time: the event each decision type records
// ---------------------------------------------------------------------------

/// One decision, and what the run's history and open counts look like afterwards.
struct DecisionCase {
    case: &'static str,
    /// Extra members for the `StartWorkflowExecution` call this row's run is started
    /// with — what makes a Lambda task have a role to assume, for instance.
    start: Value,
    /// Whether this row's domain registers [`ACTIVITY_TYPE`].
    register_activity: bool,
    decisions: Value,
    /// The `eventType`s the history holds AFTER the two events a started run begins with
    /// (`WorkflowExecutionStarted`, `DecisionTaskScheduled`) and the `DecisionTaskStarted`
    /// the poll records.
    expected_tail: &'static [&'static str],
    /// `(openActivityTasks, openTimers, openLambdaFunctions, openDecisionTasks)` as
    /// `DescribeWorkflowExecution` reports them afterwards.
    open_counts: (i64, i64, i64, i64),
    /// The `closeStatus` the run ends with, or `None` when it is still OPEN.
    close_status: Option<&'static str>,
}

#[tokio::test]
async fn each_decision_records_the_event_its_type_documents() {
    let cases = [
        DecisionCase {
            case: "no decisions at all",
            start: json!({}),
            register_activity: false,
            decisions: json!([]),
            expected_tail: &["DecisionTaskCompleted"],
            open_counts: (0, 0, 0, 0),
            close_status: None,
        },
        DecisionCase {
            case: "ScheduleActivityTask on a registered type",
            start: json!({}),
            register_activity: true,
            decisions: json!([{
                "decisionType": "ScheduleActivityTask",
                "scheduleActivityTaskDecisionAttributes": {
                    "activityType": { "name": ACTIVITY_TYPE.0, "version": ACTIVITY_TYPE.1 },
                    "activityId": "email-1",
                },
            }]),
            expected_tail: &["DecisionTaskCompleted", "ActivityTaskScheduled"],
            open_counts: (1, 0, 0, 0),
            close_status: None,
        },
        DecisionCase {
            case: "ScheduleActivityTask on a type the domain never registered",
            start: json!({}),
            register_activity: false,
            decisions: json!([{
                "decisionType": "ScheduleActivityTask",
                "scheduleActivityTaskDecisionAttributes": {
                    "activityType": { "name": ACTIVITY_TYPE.0, "version": ACTIVITY_TYPE.1 },
                    "activityId": "email-1",
                },
            }]),
            // A decision that failed is an event the decider has to act on, so the
            // service gives it another decision task to act in.
            expected_tail: &[
                "DecisionTaskCompleted",
                "ScheduleActivityTaskFailed",
                "DecisionTaskScheduled",
            ],
            open_counts: (0, 0, 0, 1),
            close_status: None,
        },
        DecisionCase {
            case: "RequestCancelActivityTask with no such task in flight",
            start: json!({}),
            register_activity: true,
            decisions: json!([{
                "decisionType": "RequestCancelActivityTask",
                "requestCancelActivityTaskDecisionAttributes": { "activityId": "never-scheduled" },
            }]),
            expected_tail: &[
                "DecisionTaskCompleted",
                "RequestCancelActivityTaskFailed",
                "DecisionTaskScheduled",
            ],
            open_counts: (0, 0, 0, 1),
            close_status: None,
        },
        DecisionCase {
            case: "RecordMarker",
            start: json!({}),
            register_activity: false,
            decisions: json!([{
                "decisionType": "RecordMarker",
                "recordMarkerDecisionAttributes": {
                    "markerName": "checkpoint",
                    "details": "step 3 of 7",
                },
            }]),
            expected_tail: &["DecisionTaskCompleted", "MarkerRecorded"],
            open_counts: (0, 0, 0, 0),
            close_status: None,
        },
        DecisionCase {
            case: "StartTimer",
            start: json!({}),
            register_activity: false,
            decisions: json!([{
                "decisionType": "StartTimer",
                "startTimerDecisionAttributes": {
                    "timerId": "retry-backoff",
                    "startToFireTimeout": "86400",
                },
            }]),
            expected_tail: &["DecisionTaskCompleted", "TimerStarted"],
            open_counts: (0, 1, 0, 0),
            close_status: None,
        },
        DecisionCase {
            case: "CancelTimer with no such timer running",
            start: json!({}),
            register_activity: false,
            decisions: json!([{
                "decisionType": "CancelTimer",
                "cancelTimerDecisionAttributes": { "timerId": "never-started" },
            }]),
            expected_tail: &[
                "DecisionTaskCompleted",
                "CancelTimerFailed",
                "DecisionTaskScheduled",
            ],
            open_counts: (0, 0, 0, 1),
            close_status: None,
        },
        DecisionCase {
            case: "two decisions in one call, applied in order",
            start: json!({}),
            register_activity: false,
            decisions: json!([
                {
                    "decisionType": "StartTimer",
                    "startTimerDecisionAttributes": {
                        "timerId": "first",
                        "startToFireTimeout": "600",
                    },
                },
                {
                    "decisionType": "RecordMarker",
                    "recordMarkerDecisionAttributes": { "markerName": "after-the-timer" },
                },
            ]),
            expected_tail: &["DecisionTaskCompleted", "TimerStarted", "MarkerRecorded"],
            open_counts: (0, 1, 0, 0),
            close_status: None,
        },
        DecisionCase {
            case: "CompleteWorkflowExecution",
            start: json!({}),
            register_activity: false,
            decisions: json!([{
                "decisionType": "CompleteWorkflowExecution",
                "completeWorkflowExecutionDecisionAttributes": { "result": "{\"ok\":true}" },
            }]),
            expected_tail: &["DecisionTaskCompleted", "WorkflowExecutionCompleted"],
            open_counts: (0, 0, 0, 0),
            close_status: Some("COMPLETED"),
        },
        DecisionCase {
            case: "FailWorkflowExecution",
            start: json!({}),
            register_activity: false,
            decisions: json!([{
                "decisionType": "FailWorkflowExecution",
                "failWorkflowExecutionDecisionAttributes": {
                    "reason": "upstream unavailable",
                    "details": "retried 5 times",
                },
            }]),
            expected_tail: &["DecisionTaskCompleted", "WorkflowExecutionFailed"],
            open_counts: (0, 0, 0, 0),
            close_status: Some("FAILED"),
        },
        DecisionCase {
            case: "CancelWorkflowExecution",
            start: json!({}),
            register_activity: false,
            decisions: json!([{
                "decisionType": "CancelWorkflowExecution",
                "cancelWorkflowExecutionDecisionAttributes": { "details": "asked to stop" },
            }]),
            expected_tail: &["DecisionTaskCompleted", "WorkflowExecutionCanceled"],
            open_counts: (0, 0, 0, 0),
            close_status: Some("CANCELED"),
        },
        DecisionCase {
            case: "a decision after the one that closed the run is not applied",
            start: json!({}),
            register_activity: false,
            decisions: json!([
                {
                    "decisionType": "CompleteWorkflowExecution",
                    "completeWorkflowExecutionDecisionAttributes": {},
                },
                {
                    "decisionType": "RecordMarker",
                    "recordMarkerDecisionAttributes": { "markerName": "too-late" },
                },
            ]),
            expected_tail: &["DecisionTaskCompleted", "WorkflowExecutionCompleted"],
            open_counts: (0, 0, 0, 0),
            close_status: Some("COMPLETED"),
        },
        DecisionCase {
            case: "ScheduleLambdaFunction on a run with no lambdaRole",
            start: json!({}),
            register_activity: false,
            decisions: json!([{
                "decisionType": "ScheduleLambdaFunction",
                "scheduleLambdaFunctionDecisionAttributes": {
                    "id": "resize-1",
                    "name": "resize-image",
                    "input": "{\"width\":64}",
                },
            }]),
            // The task is scheduled, and then cannot start: there is no IAM role
            // attached to the execution for it to be invoked under.
            expected_tail: &[
                "DecisionTaskCompleted",
                "LambdaFunctionScheduled",
                "StartLambdaFunctionFailed",
                "DecisionTaskScheduled",
            ],
            open_counts: (0, 0, 0, 1),
            close_status: None,
        },
        DecisionCase {
            case: "ScheduleLambdaFunction on a run started with a lambdaRole",
            start: json!({ "lambdaRole": "arn:aws:iam::123456789012:role/swf-lambda" }),
            register_activity: false,
            decisions: json!([{
                "decisionType": "ScheduleLambdaFunction",
                "scheduleLambdaFunctionDecisionAttributes": {
                    "id": "resize-1",
                    "name": "resize-image",
                },
            }]),
            expected_tail: &["DecisionTaskCompleted", "LambdaFunctionScheduled"],
            open_counts: (0, 0, 1, 0),
            close_status: None,
        },
        DecisionCase {
            case: "SignalExternalWorkflowExecution naming a run the domain does not hold",
            start: json!({}),
            register_activity: false,
            decisions: json!([{
                "decisionType": "SignalExternalWorkflowExecution",
                "signalExternalWorkflowExecutionDecisionAttributes": {
                    "workflowId": "no-such-run",
                    "signalName": "wake-up",
                },
            }]),
            expected_tail: &[
                "DecisionTaskCompleted",
                "SignalExternalWorkflowExecutionInitiated",
                "SignalExternalWorkflowExecutionFailed",
                "DecisionTaskScheduled",
            ],
            open_counts: (0, 0, 0, 1),
            close_status: None,
        },
        DecisionCase {
            case: "RequestCancelExternalWorkflowExecution naming a run the domain does not hold",
            start: json!({}),
            register_activity: false,
            decisions: json!([{
                "decisionType": "RequestCancelExternalWorkflowExecution",
                "requestCancelExternalWorkflowExecutionDecisionAttributes": {
                    "workflowId": "no-such-run",
                },
            }]),
            expected_tail: &[
                "DecisionTaskCompleted",
                "RequestCancelExternalWorkflowExecutionInitiated",
                "RequestCancelExternalWorkflowExecutionFailed",
                "DecisionTaskScheduled",
            ],
            open_counts: (0, 0, 0, 1),
            close_status: None,
        },
        DecisionCase {
            case: "StartChildWorkflowExecution on a type the domain never registered",
            start: json!({}),
            register_activity: false,
            decisions: json!([{
                "decisionType": "StartChildWorkflowExecution",
                "startChildWorkflowExecutionDecisionAttributes": {
                    "workflowType": { "name": "never-registered", "version": "9.9" },
                    "workflowId": "child-1",
                },
            }]),
            expected_tail: &[
                "DecisionTaskCompleted",
                "StartChildWorkflowExecutionFailed",
                "DecisionTaskScheduled",
            ],
            open_counts: (0, 0, 0, 1),
            close_status: None,
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        let emulator = Emulator::new();
        let domain = format!("decisions-{index}");
        seed(&emulator, &domain).await;
        if case.register_activity {
            register_activity_type(
                &emulator,
                case.case,
                &domain,
                ACTIVITY_TYPE,
                activity_defaults(&format!("{domain}-activities")),
            )
            .await;
        }
        let workflow_id = format!("run-{index}");
        let run_id = start(&emulator, case.case, &domain, &workflow_id, case.start.clone()).await;
        let task = poll(&emulator, case.case, &domain, DECIDER_LIST).await;
        let token = string_at(&task, "/taskToken");
        let started_event_id = i64_at(&task, "/startedEventId");

        respond(&emulator, case.case, &token, case.decisions.clone(), json!({})).await;

        let seen = event_types(&emulator, &domain, &workflow_id, &run_id).await;
        let mut expected = vec![
            "WorkflowExecutionStarted".to_string(),
            "DecisionTaskScheduled".to_string(),
            "DecisionTaskStarted".to_string(),
        ];
        for event_type in case.expected_tail {
            modelled_event_type(case.case, event_type);
            expected.push((*event_type).to_string());
        }
        assert_eq!(
            seen, expected,
            "case {:?}: expected the history {expected:?}, got {seen:?}",
            case.case
        );

        // The completion names the task it closed, out of the poll's own answer.
        let completed = event_of_type(
            &emulator,
            case.case,
            &domain,
            &workflow_id,
            &run_id,
            "DecisionTaskCompleted",
        )
        .await;
        assert_eq!(
            i64_at(&completed, "/decisionTaskCompletedEventAttributes/startedEventId"),
            started_event_id,
            "case {:?}: the completion must name the started event the poll reported, \
             got {completed:?}",
            case.case
        );
        assert_eq!(
            i64_at(
                &completed,
                "/decisionTaskCompletedEventAttributes/scheduledEventId"
            ),
            2,
            "case {:?}: the completion must name the DecisionTaskScheduled event of the \
             task it closed, got {completed:?}",
            case.case
        );

        let described = describe(&emulator, case.case, &domain, &workflow_id, &run_id).await;
        let counts = (
            i64_at(&described, "/openCounts/openActivityTasks"),
            i64_at(&described, "/openCounts/openTimers"),
            i64_at(&described, "/openCounts/openLambdaFunctions"),
            i64_at(&described, "/openCounts/openDecisionTasks"),
        );
        assert_eq!(
            counts, case.open_counts,
            "case {:?}: expected (activities, timers, lambdas, decisions) = {:?}, got \
             {counts:?} in {described:?}",
            case.case, case.open_counts
        );
        let status = string_at(&described, "/executionInfo/executionStatus");
        match case.close_status {
            Some(close_status) => {
                assert!(
                    CLOSE_STATUS_VALUES.contains(&close_status),
                    "case {:?}: {close_status:?} is not a modelled CloseStatus",
                    case.case
                );
                assert_eq!(
                    (
                        status.as_str(),
                        string_at(&described, "/executionInfo/closeStatus").as_str()
                    ),
                    ("CLOSED", close_status),
                    "case {:?}: expected the run CLOSED/{close_status}, got {described:?}",
                    case.case
                );
            }
            None => assert_eq!(
                status, "OPEN",
                "case {:?}: expected the run to stay OPEN, got {described:?}",
                case.case
            ),
        }

        // The token addressed ONE task: completing it twice is refused, so a decision
        // task cannot be answered after it is closed.
        emulator
            .refused(
                case.case,
                "RespondDecisionTaskCompleted",
                &json!({ "taskToken": token }),
                "UnknownResourceFault",
            )
            .await;
    }
}

// ---------------------------------------------------------------------------
// ScheduleActivityTask: where each value of the scheduled task came from
// ---------------------------------------------------------------------------

/// Which side supplies the task list and the three timeouts a scheduled activity task
/// must have: the decision itself, or the activity type's registration-time defaults.
struct ActivityCase {
    case: &'static str,
    overrides: Value,
    expected_task_list: &'static str,
    expected_schedule_to_start: &'static str,
    expected_schedule_to_close: &'static str,
    expected_start_to_close: &'static str,
    expected_heartbeat: Option<&'static str>,
}

#[tokio::test]
async fn a_scheduled_activity_task_takes_each_value_from_the_decision_or_the_registration() {
    // The activity type below registers four of the five, so a row can take any of them
    // from either side; `defaultTaskHeartbeatTimeout` is left unregistered because
    // `heartbeatTimeout` is the one timeout a task may carry none of at all ("If set,
    // specifies the maximum time before which a worker ... must report progress").
    let cases = [
        ActivityCase {
            case: "every value from the registration",
            overrides: json!({}),
            expected_task_list: "registered-activities",
            expected_schedule_to_start: "60",
            expected_schedule_to_close: "600",
            expected_start_to_close: "300",
            expected_heartbeat: None,
        },
        ActivityCase {
            case: "every value overridden by the decision",
            overrides: json!({
                "taskList": { "name": "decision-activities" },
                "scheduleToStartTimeout": "0",
                "scheduleToCloseTimeout": "31536000",
                "startToCloseTimeout": "NONE",
                "heartbeatTimeout": "45",
            }),
            expected_task_list: "decision-activities",
            // The published lower bound ("an integer greater than or equal to 0"), the
            // one-year upper bound a run can last, and the unlimited sentinel.
            expected_schedule_to_start: "0",
            expected_schedule_to_close: "31536000",
            expected_start_to_close: "NONE",
            expected_heartbeat: Some("45"),
        },
        ActivityCase {
            case: "the task list from the decision, the timeouts from the registration",
            overrides: json!({ "taskList": { "name": "other-activities" } }),
            expected_task_list: "other-activities",
            expected_schedule_to_start: "60",
            expected_schedule_to_close: "600",
            expected_start_to_close: "300",
            expected_heartbeat: None,
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        let emulator = Emulator::new();
        let domain = format!("activities-{index}");
        seed(&emulator, &domain).await;
        register_activity_type(
            &emulator,
            case.case,
            &domain,
            ACTIVITY_TYPE,
            activity_defaults("registered-activities"),
        )
        .await;
        let run_id = start(&emulator, case.case, &domain, "scheduler", json!({})).await;
        let token = poll_token(&emulator, case.case, &domain, DECIDER_LIST).await;

        let activity_id = format!("task-{index}");
        let mut attributes = json!({
            "activityType": { "name": ACTIVITY_TYPE.0, "version": ACTIVITY_TYPE.1 },
            "activityId": activity_id,
            "input": "{\"to\":\"nobody@example.invalid\"}",
            "control": "attempt 1",
        });
        merge(&mut attributes, case.overrides.clone());
        respond(
            &emulator,
            case.case,
            &token,
            json!([{
                "decisionType": "ScheduleActivityTask",
                "scheduleActivityTaskDecisionAttributes": attributes,
            }]),
            json!({}),
        )
        .await;

        let scheduled = event_of_type(
            &emulator,
            case.case,
            &domain,
            "scheduler",
            &run_id,
            "ActivityTaskScheduled",
        )
        .await;
        let at = |pointer: &str| {
            string_at(
                &scheduled,
                &format!("/activityTaskScheduledEventAttributes{pointer}"),
            )
        };
        assert_eq!(
            (
                at("/taskList/name"),
                at("/scheduleToStartTimeout"),
                at("/scheduleToCloseTimeout"),
                at("/startToCloseTimeout"),
            ),
            (
                case.expected_task_list.to_string(),
                case.expected_schedule_to_start.to_string(),
                case.expected_schedule_to_close.to_string(),
                case.expected_start_to_close.to_string(),
            ),
            "case {:?}: the scheduled task must carry the resolved task list and timeouts, \
             got {scheduled:?}",
            case.case
        );
        assert_eq!(
            scheduled
                .pointer("/activityTaskScheduledEventAttributes/heartbeatTimeout")
                .and_then(Value::as_str),
            case.expected_heartbeat,
            "case {:?}: the scheduled task must carry only the heartbeat timeout one side \
             supplied, got {scheduled:?}",
            case.case
        );
        assert_eq!(
            at("/activityId"),
            activity_id,
            "case {:?}: the scheduled task must carry the decision's activityId, got \
             {scheduled:?}",
            case.case
        );

        // The task now waits in the list it was scheduled on, and in no other.
        for (task_list, expected) in [
            (case.expected_task_list, 1),
            ("registered-activities", i64::from(case.expected_task_list == "registered-activities")),
        ] {
            let counted = emulator
                .ok(
                    case.case,
                    "CountPendingActivityTasks",
                    &json!({ "domain": domain, "taskList": { "name": task_list } }),
                )
                .await;
            assert_eq!(
                i64_at(&counted, "/count"),
                expected,
                "case {:?}: expected {expected} activity task(s) waiting in {task_list:?}, \
                 got {counted:?}",
                case.case
            );
        }

        // The run was given a new decision task for nothing: scheduling work is what
        // the decider is now waiting on, so it decides again when that work reports.
        let counted = emulator
            .ok(
                case.case,
                "CountPendingDecisionTasks",
                &json!({ "domain": domain, "taskList": { "name": DECIDER_LIST } }),
            )
            .await;
        assert_eq!(
            i64_at(&counted, "/count"),
            0,
            "case {:?}: a run whose decider only scheduled work has no further decision \
             task, got {counted:?}",
            case.case
        );

        // A second schedule under the same `activityId` collides with the task in
        // flight; a signal is what gives the decider the task to make it in.
        emulator
            .ok(
                case.case,
                "SignalWorkflowExecution",
                &json!({ "domain": domain, "workflowId": "scheduler", "signalName": "again" }),
            )
            .await;
        let token = poll_token(&emulator, case.case, &domain, DECIDER_LIST).await;
        respond(
            &emulator,
            case.case,
            &token,
            json!([{
                "decisionType": "ScheduleActivityTask",
                "scheduleActivityTaskDecisionAttributes": {
                    "activityType": { "name": ACTIVITY_TYPE.0, "version": ACTIVITY_TYPE.1 },
                    "activityId": activity_id,
                },
            }]),
            json!({}),
        )
        .await;
        let failed = event_of_type(
            &emulator,
            case.case,
            &domain,
            "scheduler",
            &run_id,
            "ScheduleActivityTaskFailed",
        )
        .await;
        assert_eq!(
            string_at(&failed, "/scheduleActivityTaskFailedEventAttributes/cause"),
            "ACTIVITY_ID_ALREADY_IN_USE",
            "case {:?}: a second schedule of an activityId in flight must fail on it, got \
             {failed:?}",
            case.case
        );
    }
}

// ---------------------------------------------------------------------------
// Timers: started by a decision, fired by the clock, cancelled by a decision
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_timer_fires_after_its_own_delay_and_a_cancelled_one_never_does() {
    let emulator = Emulator::new();
    let domain = "timers";
    seed(&emulator, domain).await;
    let run_id = start(&emulator, "timers", domain, "waiter", json!({})).await;
    let token = poll_token(&emulator, "timers", domain, DECIDER_LIST).await;

    // The published lower bound of `startToFireTimeout` ("an integer greater than or
    // equal to 0") is due the instant it is started; a day away is not.
    respond(
        &emulator,
        "timers",
        &token,
        json!([
            {
                "decisionType": "StartTimer",
                "startTimerDecisionAttributes": { "timerId": "now", "startToFireTimeout": "0" },
            },
            {
                "decisionType": "StartTimer",
                "startTimerDecisionAttributes": {
                    "timerId": "tomorrow",
                    "startToFireTimeout": "86400",
                    "control": "the long one",
                },
            },
        ]),
        json!({}),
    )
    .await;

    // Reading the run materialises what the clock already owed it: the zero-delay timer
    // has fired, and the decider is given a decision task to act on it.
    let seen = event_types(&emulator, domain, "waiter", &run_id).await;
    assert_eq!(
        seen,
        vec![
            "WorkflowExecutionStarted",
            "DecisionTaskScheduled",
            "DecisionTaskStarted",
            "DecisionTaskCompleted",
            "TimerStarted",
            "TimerStarted",
            "TimerFired",
            "DecisionTaskScheduled",
        ],
        "case \"timers\": the due timer fires and the decider is told, got {seen:?}"
    );
    let fired = event_of_type(&emulator, "timers", domain, "waiter", &run_id, "TimerFired").await;
    assert_eq!(
        string_at(&fired, "/timerFiredEventAttributes/timerId"),
        "now",
        "case \"timers\": only the timer whose delay ran out fires, got {fired:?}"
    );
    let described = describe(&emulator, "timers", domain, "waiter", &run_id).await;
    assert_eq!(
        i64_at(&described, "/openCounts/openTimers"),
        1,
        "case \"timers\": the timer that has not fired is still the only open one, got \
         {described:?}"
    );

    // The decider cancels the remaining timer, which takes it out of flight.
    let token = poll_token(&emulator, "timers", domain, DECIDER_LIST).await;
    respond(
        &emulator,
        "timers",
        &token,
        json!([{
            "decisionType": "CancelTimer",
            "cancelTimerDecisionAttributes": { "timerId": "tomorrow" },
        }]),
        json!({}),
    )
    .await;
    let canceled = event_of_type(&emulator, "timers", domain, "waiter", &run_id, "TimerCanceled")
        .await;
    assert_eq!(
        string_at(&canceled, "/timerCanceledEventAttributes/timerId"),
        "tomorrow",
        "case \"timers\": the cancelled timer is the one the decision named, got {canceled:?}"
    );
    let described = describe(&emulator, "timers", domain, "waiter", &run_id).await;
    assert_eq!(
        i64_at(&described, "/openCounts/openTimers"),
        0,
        "case \"timers\": a cancelled timer is no longer open, got {described:?}"
    );
}

// ---------------------------------------------------------------------------
// ContinueAsNewWorkflowExecution: one run closes, the next one carries on
// ---------------------------------------------------------------------------

#[tokio::test]
async fn continue_as_new_closes_this_run_and_starts_the_next_one() {
    // Two rows: one that takes everything from the workflow type's registered defaults,
    // one that overrides the task list, the input and the tag list.
    let cases = [
        ("from the registered defaults", json!({}), DECIDER_LIST),
        (
            "with the next run's own task list",
            json!({
                "taskList": { "name": "next-deciders" },
                "input": "{\"generation\":2}",
                "tagList": ["carried", "over"],
                "executionStartToCloseTimeout": "600",
            }),
            "next-deciders",
        ),
    ];

    for (index, (case, attributes, expected_list)) in cases.iter().enumerate() {
        let emulator = Emulator::new();
        let domain = format!("continue-{index}");
        seed(&emulator, &domain).await;
        let first = start(&emulator, case, &domain, "looping", json!({})).await;
        let token = poll_token(&emulator, case, &domain, DECIDER_LIST).await;

        respond(
            &emulator,
            case,
            &token,
            json!([{
                "decisionType": "ContinueAsNewWorkflowExecution",
                "continueAsNewWorkflowExecutionDecisionAttributes": attributes,
            }]),
            json!({}),
        )
        .await;

        let continued = event_of_type(
            &emulator,
            case,
            &domain,
            "looping",
            &first,
            "WorkflowExecutionContinuedAsNew",
        )
        .await;
        let second = string_at(
            &continued,
            "/workflowExecutionContinuedAsNewEventAttributes/newExecutionRunId",
        );
        assert_ne!(
            second, first,
            "case {case:?}: the run that carries on is a NEW run, got {continued:?}"
        );
        assert_eq!(
            string_at(
                &continued,
                "/workflowExecutionContinuedAsNewEventAttributes/taskList/name"
            ),
            *expected_list,
            "case {case:?}: the closing event records the next run's resolved task list, \
             got {continued:?}"
        );

        let described = describe(&emulator, case, &domain, "looping", &first).await;
        assert_eq!(
            (
                string_at(&described, "/executionInfo/executionStatus"),
                string_at(&described, "/executionInfo/closeStatus"),
            ),
            ("CLOSED".to_string(), "CONTINUED_AS_NEW".to_string()),
            "case {case:?}: the run that continued as new is closed as such, got {described:?}"
        );

        // The new run is a run of its own, started with its first decision task waiting
        // on the list it resolved, and it names the run it continues.
        let started = event_of_type(
            &emulator,
            case,
            &domain,
            "looping",
            &second,
            "WorkflowExecutionStarted",
        )
        .await;
        assert_eq!(
            string_at(
                &started,
                "/workflowExecutionStartedEventAttributes/continuedExecutionRunId"
            ),
            first,
            "case {case:?}: the new run names the run it continues, got {started:?}"
        );
        let task = poll(&emulator, case, &domain, expected_list).await;
        assert_eq!(
            string_at(&task, "/workflowExecution/runId"),
            second,
            "case {case:?}: the next decision task belongs to the run that carried on, \
             got {task:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// StartChildWorkflowExecution: a real run of its own, reporting back
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_child_run_is_a_run_of_its_own_and_reports_back_when_it_closes() {
    let emulator = Emulator::new();
    let domain = "children";
    seed(&emulator, domain).await;
    let parent = start(&emulator, "children", domain, "parent", json!({})).await;
    let token = poll_token(&emulator, "children", domain, DECIDER_LIST).await;

    respond(
        &emulator,
        "children",
        &token,
        json!([{
            "decisionType": "StartChildWorkflowExecution",
            "startChildWorkflowExecutionDecisionAttributes": {
                "workflowType": { "name": WORKFLOW_TYPE.0, "version": WORKFLOW_TYPE.1 },
                "workflowId": "child",
                "input": "{\"part\":1}",
                "taskList": { "name": "child-deciders" },
                "control": "the only child",
            },
        }]),
        json!({}),
    )
    .await;

    let started = event_of_type(
        &emulator,
        "children",
        domain,
        "parent",
        &parent,
        "ChildWorkflowExecutionStarted",
    )
    .await;
    let child = string_at(
        &started,
        "/childWorkflowExecutionStartedEventAttributes/workflowExecution/runId",
    );
    let described = describe(&emulator, "children", domain, "parent", &parent).await;
    assert_eq!(
        i64_at(&described, "/openCounts/openChildWorkflowExecutions"),
        1,
        "case \"children\": the parent has its child in flight, got {described:?}"
    );

    // The child is a separate run with its own history and its own decision task,
    // waiting on the list the decision named, and it knows its parent.
    let child_described = describe(&emulator, "children", domain, "child", &child).await;
    assert_eq!(
        (
            string_at(&child_described, "/executionInfo/parent/workflowId"),
            string_at(&child_described, "/executionInfo/parent/runId"),
        ),
        ("parent".to_string(), parent.clone()),
        "case \"children\": the child reports the run that started it, got {child_described:?}"
    );
    let child_task = poll(&emulator, "children", domain, "child-deciders").await;
    assert_eq!(
        string_at(&child_task, "/workflowExecution/runId"),
        child,
        "case \"children\": the child's decision task waits on the list the decision \
         named, got {child_task:?}"
    );

    // When the child closes, what became of it is an event of the PARENT's history, and
    // the parent is given a decision task to act on it.
    respond(
        &emulator,
        "children",
        &string_at(&child_task, "/taskToken"),
        json!([{
            "decisionType": "CompleteWorkflowExecution",
            "completeWorkflowExecutionDecisionAttributes": { "result": "part 1 done" },
        }]),
        json!({}),
    )
    .await;
    let reported = event_of_type(
        &emulator,
        "children",
        domain,
        "parent",
        &parent,
        "ChildWorkflowExecutionCompleted",
    )
    .await;
    assert_eq!(
        (
            string_at(
                &reported,
                "/childWorkflowExecutionCompletedEventAttributes/result"
            ),
            string_at(
                &reported,
                "/childWorkflowExecutionCompletedEventAttributes/workflowExecution/runId"
            ),
            i64_at(
                &reported,
                "/childWorkflowExecutionCompletedEventAttributes/initiatedEventId"
            ),
        ),
        (
            "part 1 done".to_string(),
            child.clone(),
            i64_at(
                &started,
                "/childWorkflowExecutionStartedEventAttributes/initiatedEventId"
            ),
        ),
        "case \"children\": the parent is told what its child ended with, got {reported:?}"
    );
    let described = describe(&emulator, "children", domain, "parent", &parent).await;
    assert_eq!(
        (
            i64_at(&described, "/openCounts/openChildWorkflowExecutions"),
            i64_at(&described, "/openCounts/openDecisionTasks"),
        ),
        (0, 1),
        "case \"children\": a closed child is no longer in flight and the parent decides \
         again, got {described:?}"
    );

    // A child `workflowId` that is already running is the collision the cause names.
    let token = poll_token(&emulator, "children", domain, DECIDER_LIST).await;
    let second = start(&emulator, "children", domain, "sibling", json!({})).await;
    assert!(!second.is_empty(), "case \"children\": the sibling run started");
    respond(
        &emulator,
        "children",
        &token,
        json!([{
            "decisionType": "StartChildWorkflowExecution",
            "startChildWorkflowExecutionDecisionAttributes": {
                "workflowType": { "name": WORKFLOW_TYPE.0, "version": WORKFLOW_TYPE.1 },
                "workflowId": "sibling",
            },
        }]),
        json!({}),
    )
    .await;
    let failed = event_of_type(
        &emulator,
        "children",
        domain,
        "parent",
        &parent,
        "StartChildWorkflowExecutionFailed",
    )
    .await;
    assert_eq!(
        string_at(
            &failed,
            "/startChildWorkflowExecutionFailedEventAttributes/cause"
        ),
        "WORKFLOW_ALREADY_RUNNING",
        "case \"children\": a child id already running fails on it, got {failed:?}"
    );
}

#[tokio::test]
async fn the_child_policy_reaches_the_children_of_a_terminated_parent_only() {
    // `childPolicy` "specifies the policy to use for the child workflow executions of
    // this workflow execution IF IT IS TERMINATED, by calling the
    // TerminateWorkflowExecution action explicitly or due to an expired timeout" — so
    // the rows below span the three policies under a terminate, and a parent that closed
    // by its own decision instead, whose children keep running.
    let cases = [
        ("TERMINATE", true, Some("CLOSED"), Some("TERMINATED")),
        ("REQUEST_CANCEL", true, Some("OPEN"), None),
        ("ABANDON", true, Some("OPEN"), None),
        ("TERMINATE", false, Some("OPEN"), None),
    ];

    for (index, (child_policy, terminate_parent, child_status, child_close_status)) in
        cases.iter().enumerate()
    {
        let case = format!("{child_policy} parent {}", if *terminate_parent { "terminated" } else { "completed" });
        let emulator = Emulator::new();
        let domain = format!("child-policy-{index}");
        seeded(
            &emulator,
            &case,
            &domain,
            WORKFLOW_TYPE,
            full_defaults(DECIDER_LIST, child_policy),
        )
        .await;
        let parent = start(&emulator, &case, &domain, "parent", json!({})).await;
        let token = poll_token(&emulator, &case, &domain, DECIDER_LIST).await;
        respond(
            &emulator,
            &case,
            &token,
            json!([{
                "decisionType": "StartChildWorkflowExecution",
                "startChildWorkflowExecutionDecisionAttributes": {
                    "workflowType": { "name": WORKFLOW_TYPE.0, "version": WORKFLOW_TYPE.1 },
                    "workflowId": "child",
                    "taskList": { "name": "child-deciders" },
                },
            }]),
            json!({}),
        )
        .await;
        let started = event_of_type(
            &emulator,
            &case,
            &domain,
            "parent",
            &parent,
            "ChildWorkflowExecutionStarted",
        )
        .await;
        let child = string_at(
            &started,
            "/childWorkflowExecutionStartedEventAttributes/workflowExecution/runId",
        );

        if *terminate_parent {
            emulator
                .ok(
                    &case,
                    "TerminateWorkflowExecution",
                    &json!({ "domain": domain, "workflowId": "parent", "runId": parent }),
                )
                .await;
        } else {
            // The child start gave the parent no new decision task — it is waiting on
            // its child — so a signal is what gives it the task it closes itself in.
            emulator
                .ok(
                    &case,
                    "SignalWorkflowExecution",
                    &json!({ "domain": domain, "workflowId": "parent", "signalName": "wrap-up" }),
                )
                .await;
            let token = poll_token(&emulator, &case, &domain, DECIDER_LIST).await;
            respond(
                &emulator,
                &case,
                &token,
                json!([{
                    "decisionType": "CompleteWorkflowExecution",
                    "completeWorkflowExecutionDecisionAttributes": {},
                }]),
                json!({}),
            )
            .await;
        }

        let described = describe(&emulator, &case, &domain, "child", &child).await;
        assert_eq!(
            Some(string_at(&described, "/executionInfo/executionStatus")).as_deref(),
            *child_status,
            "case {case:?}: expected the child {child_status:?}, got {described:?}"
        );
        assert_eq!(
            described
                .pointer("/executionInfo/closeStatus")
                .and_then(Value::as_str),
            *child_close_status,
            "case {case:?}: expected the child's closeStatus {child_close_status:?}, got \
             {described:?}"
        );

        let child_events = event_types(&emulator, &domain, "child", &child).await;
        match *child_policy {
            "TERMINATE" if *terminate_parent => {
                let terminated = event_of_type(
                    &emulator,
                    &case,
                    &domain,
                    "child",
                    &child,
                    "WorkflowExecutionTerminated",
                )
                .await;
                assert_eq!(
                    string_at(&terminated, "/workflowExecutionTerminatedEventAttributes/cause"),
                    "CHILD_POLICY_APPLIED",
                    "case {case:?}: the service terminated the child under the parent's \
                     policy, got {terminated:?}"
                );
            }
            "REQUEST_CANCEL" => {
                let requested = event_of_type(
                    &emulator,
                    &case,
                    &domain,
                    "child",
                    &child,
                    "WorkflowExecutionCancelRequested",
                )
                .await;
                assert_eq!(
                    string_at(
                        &requested,
                        "/workflowExecutionCancelRequestedEventAttributes/cause"
                    ),
                    "CHILD_POLICY_APPLIED",
                    "case {case:?}: the child is asked to cancel itself, got {requested:?}"
                );
            }
            _ => assert!(
                !child_events
                    .iter()
                    .any(|event| event == "WorkflowExecutionTerminated"
                        || event == "WorkflowExecutionCancelRequested"),
                "case {case:?}: nothing reaches this child, got {child_events:?}"
            ),
        }
    }
}

#[tokio::test]
async fn continue_as_new_past_the_per_domain_run_bound_fails_and_leaves_the_run_open() {
    // A closed run stays stored — its history is what the readers answer from — so every
    // run a decision starts adds to the same bounded store. Past that bound the decision
    // fails with the modelled cause and the run it would have replaced stays open,
    // rather than the store growing for as long as a decider keeps looping.
    let emulator = Emulator::new();
    let domain = "full-domain";
    seed(&emulator, domain).await;
    const LOOPING_LIST: &str = "looping-deciders";
    let run_id = start(
        &emulator,
        "bound",
        domain,
        "looping",
        json!({ "taskList": { "name": LOOPING_LIST } }),
    )
    .await;
    // Fill the domain to its bound with runs of their own, each deciding elsewhere so a
    // poll of the looping run's list answers with its task and no other.
    for index in 1..MAX_EXECUTIONS_PER_DOMAIN {
        start(&emulator, "bound", domain, &format!("filler-{index}"), json!({})).await;
    }

    let token = poll_token(&emulator, "bound", domain, LOOPING_LIST).await;
    respond(
        &emulator,
        "bound",
        &token,
        json!([{
            "decisionType": "ContinueAsNewWorkflowExecution",
            "continueAsNewWorkflowExecutionDecisionAttributes": {},
        }]),
        json!({}),
    )
    .await;

    let failed = event_of_type(
        &emulator,
        "bound",
        domain,
        "looping",
        &run_id,
        "ContinueAsNewWorkflowExecutionFailed",
    )
    .await;
    assert_eq!(
        string_at(
            &failed,
            "/continueAsNewWorkflowExecutionFailedEventAttributes/cause"
        ),
        "CONTINUE_AS_NEW_WORKFLOW_EXECUTION_RATE_EXCEEDED",
        "case \"bound\": a domain that cannot hold the next run fails the decision, got \
         {failed:?}"
    );
    let described = describe(&emulator, "bound", domain, "looping", &run_id).await;
    assert_eq!(
        string_at(&described, "/executionInfo/executionStatus"),
        "OPEN",
        "case \"bound\": the run that could not be continued stays open, got {described:?}"
    );
}

#[tokio::test]
async fn a_deep_chain_of_child_runs_is_terminated_without_recursing_on_the_chain() {
    // The TERMINATE child policy cascades down the whole parent/child chain, and a
    // decider can build that chain as deep as the domain holds runs. Closing the root
    // must cost the service queue entries rather than stack frames, so this row builds a
    // chain far deeper than any one close could nest and terminates the root of it.
    const DEPTH: usize = 200;
    let emulator = Emulator::new();
    let domain = "deep-chain";
    seed(&emulator, domain).await;
    let root = start(&emulator, "deep chain", domain, "root", json!({})).await;

    // At every step exactly one decision task is waiting: the one the newly started
    // child begins with, since starting a child gives its parent no new task. The
    // child's `runId` is read off the parent that started it, which is the only place
    // the service reports it.
    let mut deepest = ("root".to_string(), root.clone());
    for depth in 1..=DEPTH {
        let token = poll_token(&emulator, "deep chain", domain, DECIDER_LIST).await;
        let child = format!("child-{depth}");
        respond(
            &emulator,
            "deep chain",
            &token,
            json!([{
                "decisionType": "StartChildWorkflowExecution",
                "startChildWorkflowExecutionDecisionAttributes": {
                    "workflowType": { "name": WORKFLOW_TYPE.0, "version": WORKFLOW_TYPE.1 },
                    "workflowId": child,
                },
            }]),
            json!({}),
        )
        .await;
        let started = event_of_type(
            &emulator,
            "deep chain",
            domain,
            &deepest.0,
            &deepest.1,
            "ChildWorkflowExecutionStarted",
        )
        .await;
        let child_run = string_at(
            &started,
            "/childWorkflowExecutionStartedEventAttributes/workflowExecution/runId",
        );
        deepest = (child, child_run);
    }

    let described = describe(&emulator, "deep chain", domain, &deepest.0, &deepest.1).await;
    assert_eq!(
        string_at(&described, "/executionInfo/executionStatus"),
        "OPEN",
        "case \"deep chain\": the whole chain is open before the root is terminated, got \
         {described:?}"
    );

    emulator
        .ok(
            "deep chain",
            "TerminateWorkflowExecution",
            &json!({ "domain": domain, "workflowId": "root", "runId": root }),
        )
        .await;

    let described = describe(&emulator, "deep chain", domain, &deepest.0, &deepest.1).await;
    assert_eq!(
        (
            string_at(&described, "/executionInfo/executionStatus"),
            string_at(&described, "/executionInfo/closeStatus"),
        ),
        ("CLOSED".to_string(), "TERMINATED".to_string()),
        "case \"deep chain\": the policy reaches the deepest child of the chain, got \
         {described:?}"
    );
    let terminated = event_of_type(
        &emulator,
        "deep chain",
        domain,
        &deepest.0,
        &deepest.1,
        "WorkflowExecutionTerminated",
    )
    .await;
    assert_eq!(
        string_at(&terminated, "/workflowExecutionTerminatedEventAttributes/cause"),
        "CHILD_POLICY_APPLIED",
        "case \"deep chain\": the service terminated it under the root's policy, got \
         {terminated:?}"
    );
}

// ---------------------------------------------------------------------------
// SignalExternalWorkflowExecution: the signal reaches the other run
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_external_signal_decision_delivers_the_signal_to_the_run_it_names() {
    // Two rows: the target addressed by `workflowId` alone (its one open run) and by the
    // `(workflowId, runId)` pair. The two runs decide on task lists of their own, so a
    // poll names which run's task it hands over.
    const TARGET_LIST: &str = "target-deciders";
    for (index, case) in ["by workflowId", "by workflowId and runId"].iter().enumerate() {
        let emulator = Emulator::new();
        let domain = format!("signals-{index}");
        seed(&emulator, &domain).await;
        let sender = start(&emulator, case, &domain, "sender", json!({})).await;
        let target = start(
            &emulator,
            case,
            &domain,
            "target",
            json!({ "taskList": { "name": TARGET_LIST } }),
        )
        .await;

        // The target's own first decision task is answered first, so the signal below is
        // what gives it its next one.
        let target_token = poll_token(&emulator, case, &domain, TARGET_LIST).await;
        respond(&emulator, case, &target_token, json!([]), json!({})).await;
        let sender_token = poll_token(&emulator, case, &domain, DECIDER_LIST).await;

        let mut attributes = json!({
            "workflowId": "target",
            "signalName": "wake-up",
            "input": "{\"at\":\"once\"}",
            "control": "sent by the sender",
        });
        if index == 1 {
            merge(&mut attributes, json!({ "runId": target }));
        }
        respond(
            &emulator,
            case,
            &sender_token,
            json!([{
                "decisionType": "SignalExternalWorkflowExecution",
                "signalExternalWorkflowExecutionDecisionAttributes": attributes,
            }]),
            json!({}),
        )
        .await;

        // The target holds the signal and the decision task it has to act on it in.
        let signaled = event_of_type(&emulator, case, &domain, "target", &target, "WorkflowExecutionSignaled")
            .await;
        assert_eq!(
            (
                string_at(&signaled, "/workflowExecutionSignaledEventAttributes/signalName"),
                string_at(
                    &signaled,
                    "/workflowExecutionSignaledEventAttributes/externalWorkflowExecution/runId"
                ),
            ),
            ("wake-up".to_string(), sender.clone()),
            "case {case:?}: the target's signal names the run that sent it, got {signaled:?}"
        );
        let target_events = event_types(&emulator, &domain, "target", &target).await;
        assert_eq!(
            target_events.last().map(String::as_str),
            Some("DecisionTaskScheduled"),
            "case {case:?}: a signalled run gets a decision task, got {target_events:?}"
        );
        let waiting = poll(&emulator, case, &domain, TARGET_LIST).await;
        assert_eq!(
            string_at(&waiting, "/workflowExecution/runId"),
            target,
            "case {case:?}: that task waits on the target's own list, got {waiting:?}"
        );

        // The sender holds the request and the acknowledgement that it was delivered.
        let sender_events = event_types(&emulator, &domain, "sender", &sender).await;
        assert_eq!(
            sender_events,
            vec![
                "WorkflowExecutionStarted",
                "DecisionTaskScheduled",
                "DecisionTaskStarted",
                "DecisionTaskCompleted",
                "SignalExternalWorkflowExecutionInitiated",
                "ExternalWorkflowExecutionSignaled",
                "DecisionTaskScheduled",
            ],
            "case {case:?}: the sender records the request and its delivery, got \
             {sender_events:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// A closing decision taken without the newest events
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_closing_decision_is_refused_when_the_decider_had_not_seen_the_newest_events() {
    // One row per closing decision, each with the `*Failed` event the service records
    // for it instead of closing the run.
    let cases = [
        (
            "CompleteWorkflowExecution",
            json!({ "result": "done" }),
            "completeWorkflowExecutionDecisionAttributes",
            "CompleteWorkflowExecutionFailed",
            "completeWorkflowExecutionFailedEventAttributes",
        ),
        (
            "FailWorkflowExecution",
            json!({ "reason": "give up" }),
            "failWorkflowExecutionDecisionAttributes",
            "FailWorkflowExecutionFailed",
            "failWorkflowExecutionFailedEventAttributes",
        ),
        (
            "CancelWorkflowExecution",
            json!({}),
            "cancelWorkflowExecutionDecisionAttributes",
            "CancelWorkflowExecutionFailed",
            "cancelWorkflowExecutionFailedEventAttributes",
        ),
        (
            "ContinueAsNewWorkflowExecution",
            json!({}),
            "continueAsNewWorkflowExecutionDecisionAttributes",
            "ContinueAsNewWorkflowExecutionFailed",
            "continueAsNewWorkflowExecutionFailedEventAttributes",
        ),
    ];

    for (index, (decision_type, attributes, member, failed_event, failed_member)) in
        cases.iter().enumerate()
    {
        let emulator = Emulator::new();
        let domain = format!("unhandled-{index}");
        seed(&emulator, &domain).await;
        let run_id = start(&emulator, decision_type, &domain, "racing", json!({})).await;
        let token = poll_token(&emulator, decision_type, &domain, DECIDER_LIST).await;

        // The attributes member a decision carries is named by its own type, so the row
        // builds the decision object rather than naming one member in the literal.
        let mut decision = serde_json::Map::new();
        decision.insert("decisionType".to_string(), json!(decision_type));
        decision.insert((*member).to_string(), attributes.clone());
        let decisions = Value::Array(vec![Value::Object(decision)]);

        // A signal arrives while the decider is deciding: the history the decider was
        // handed is no longer the whole history.
        emulator
            .ok(
                decision_type,
                "SignalWorkflowExecution",
                &json!({
                    "domain": domain,
                    "workflowId": "racing",
                    "signalName": "stop-the-press",
                }),
            )
            .await;

        respond(&emulator, decision_type, &token, decisions.clone(), json!({})).await;

        let failed = event_of_type(
            &emulator,
            decision_type,
            &domain,
            "racing",
            &run_id,
            failed_event,
        )
        .await;
        assert_eq!(
            string_at(&failed, &format!("/{failed_member}/cause")),
            "UNHANDLED_DECISION",
            "case {decision_type:?}: a closing decision taken without the newest events \
             fails on them, got {failed:?}"
        );

        let described = describe(&emulator, decision_type, &domain, "racing", &run_id).await;
        assert_eq!(
            (
                string_at(&described, "/executionInfo/executionStatus"),
                i64_at(&described, "/openCounts/openDecisionTasks"),
            ),
            ("OPEN".to_string(), 1),
            "case {decision_type:?}: the run stays open and the decider is given another \
             decision task, got {described:?}"
        );

        // That new decision task carries the signal the first decider missed, and the
        // same decision taken with it in hand closes the run.
        let token = poll_token(&emulator, decision_type, &domain, DECIDER_LIST).await;
        respond(&emulator, decision_type, &token, decisions, json!({})).await;
        let described = describe(&emulator, decision_type, &domain, "racing", &run_id).await;
        assert_eq!(
            string_at(&described, "/executionInfo/executionStatus"),
            "CLOSED",
            "case {decision_type:?}: the retried decision closes the run, got {described:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The task list override
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_task_list_override_moves_the_runs_future_decision_tasks() {
    let emulator = Emulator::new();
    let domain = "overrides";
    seed(&emulator, domain).await;
    let run_id = start(&emulator, "permanent", domain, "moved", json!({})).await;
    let token = poll_token(&emulator, "permanent", domain, DECIDER_LIST).await;

    // No timeout: "when this parameter is missing, the task list override is permanent".
    respond(
        &emulator,
        "permanent",
        &token,
        json!([]),
        json!({
            "taskList": { "name": "other-deciders" },
            "executionContext": "{\"cursor\":17}",
        }),
    )
    .await;

    let described = describe(&emulator, "permanent", domain, "moved", &run_id).await;
    assert_eq!(
        (
            string_at(&described, "/executionConfiguration/taskList/name"),
            string_at(&described, "/latestExecutionContext"),
        ),
        ("other-deciders".to_string(), "{\"cursor\":17}".to_string()),
        "case \"permanent\": the run reports the overridden list and the context the \
         decider attached, got {described:?}"
    );
    let completed = event_of_type(
        &emulator,
        "permanent",
        domain,
        "moved",
        &run_id,
        "DecisionTaskCompleted",
    )
    .await;
    assert_eq!(
        string_at(
            &completed,
            "/decisionTaskCompletedEventAttributes/taskList/name"
        ),
        "other-deciders",
        "case \"permanent\": the completion records the override it was asked for, got \
         {completed:?}"
    );

    // The run's NEXT decision task waits on the overridden list, and not on the original.
    emulator
        .ok(
            "permanent",
            "SignalWorkflowExecution",
            &json!({ "domain": domain, "workflowId": "moved", "signalName": "again" }),
        )
        .await;
    for (task_list, expected) in [("other-deciders", 1), (DECIDER_LIST, 0)] {
        let counted = emulator
            .ok(
                "permanent",
                "CountPendingDecisionTasks",
                &json!({ "domain": domain, "taskList": { "name": task_list } }),
            )
            .await;
        assert_eq!(
            i64_at(&counted, "/count"),
            expected,
            "case \"permanent\": expected {expected} decision task(s) waiting in \
             {task_list:?}, got {counted:?}"
        );
    }
}

#[tokio::test]
async fn a_task_list_override_with_a_timeout_is_reverted_when_nobody_starts_its_task() {
    let emulator = Emulator::new();
    let domain = "temporary-overrides";
    seed(&emulator, domain).await;
    let run_id = start(&emulator, "temporary", domain, "reverted", json!({})).await;
    let token = poll_token(&emulator, "temporary", domain, DECIDER_LIST).await;

    // "If a decision task scheduled on the override task list is not started within the
    // timeout, the decision task will time out. Amazon SWF will revert the override and
    // schedule a new decision task to the original task list." The published lower bound
    // of the duration is due the instant the task is scheduled.
    respond(
        &emulator,
        "temporary",
        &token,
        json!([{
            "decisionType": "RecordMarker",
            "recordMarkerDecisionAttributes": { "markerName": "before-the-override" },
        }]),
        json!({
            "taskList": { "name": "briefly-other" },
            "taskListScheduleToStartTimeout": "0",
        }),
    )
    .await;

    // The decision the completion recorded gave the run no new task, so the override is
    // first seen by the task a signal schedules on it.
    emulator
        .ok(
            "temporary",
            "SignalWorkflowExecution",
            &json!({ "domain": domain, "workflowId": "reverted", "signalName": "go" }),
        )
        .await;

    // Counting the decision tasks of a list materialises the decision-task timeouts the
    // clock already owed the domain: the task on the override list timed out waiting for
    // a decider, so the override is reverted and the replacement waits on the original
    // list.
    for (task_list, expected) in [("briefly-other", 0), (DECIDER_LIST, 1)] {
        let counted = emulator
            .ok(
                "temporary",
                "CountPendingDecisionTasks",
                &json!({ "domain": domain, "taskList": { "name": task_list } }),
            )
            .await;
        assert_eq!(
            i64_at(&counted, "/count"),
            expected,
            "case \"temporary\": expected {expected} decision task(s) waiting in \
             {task_list:?}, got {counted:?}"
        );
    }

    let seen = event_types(&emulator, domain, "reverted", &run_id).await;
    assert!(
        seen.contains(&"DecisionTaskTimedOut".to_string()),
        "case \"temporary\": the task on the override list times out, got {seen:?}"
    );
    let timed_out = event_of_type(
        &emulator,
        "temporary",
        domain,
        "reverted",
        &run_id,
        "DecisionTaskTimedOut",
    )
    .await;
    let timeout_type = string_at(
        &timed_out,
        "/decisionTaskTimedOutEventAttributes/timeoutType",
    );
    assert!(
        DECISION_TASK_TIMEOUT_TYPE_VALUES.contains(&timeout_type.as_str()),
        "case \"temporary\": {timeout_type:?} is not a modelled DecisionTaskTimeoutType"
    );
    assert_eq!(
        timeout_type, "SCHEDULE_TO_START",
        "case \"temporary\": a task nobody started times out waiting, got {timed_out:?}"
    );

    let described = describe(&emulator, "temporary", domain, "reverted", &run_id).await;
    assert_eq!(
        string_at(&described, "/executionConfiguration/taskList/name"),
        DECIDER_LIST,
        "case \"temporary\": the override is reverted to the run's original list, got \
         {described:?}"
    );
    let task = poll(&emulator, "temporary", domain, DECIDER_LIST).await;
    assert_eq!(
        string_at(&task, "/workflowExecution/runId"),
        run_id,
        "case \"temporary\": the replacement task waits on the original list, got {task:?}"
    );
}

// ---------------------------------------------------------------------------
// Refusals: a token that addresses nothing, and a decision this service cannot read
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_task_token_that_addresses_no_outstanding_task_is_an_unknown_resource() {
    let emulator = Emulator::new();
    let domain = "tokens";
    seed(&emulator, domain).await;
    let run_id = start(&emulator, "tokens", domain, "once", json!({})).await;
    let live = poll_token(&emulator, "tokens", domain, DECIDER_LIST).await;

    // Rows: a token no task ever had, one of the right shape but of nothing, the live
    // token with a character changed, and (below) the live token after it was used.
    let mut rotated: Vec<char> = live.chars().collect();
    rotated[0] = if rotated[0] == 'a' { 'b' } else { 'a' };
    let rotated: String = rotated.into_iter().collect();
    for (case, token) in [
        ("a token of a single character", "x".to_string()),
        ("a token-shaped string of nothing", "0".repeat(live.len())),
        ("the live token with one character changed", rotated),
    ] {
        emulator
            .refused(
                case,
                "RespondDecisionTaskCompleted",
                &json!({ "taskToken": token }),
                "UnknownResourceFault",
            )
            .await;
    }

    // None of those refusals touched the run, so the live token still works — once.
    respond(&emulator, "tokens", &live, json!([]), json!({})).await;
    emulator
        .refused(
            "the token of a task already completed",
            "RespondDecisionTaskCompleted",
            &json!({ "taskToken": live }),
            "UnknownResourceFault",
        )
        .await;
    let seen = event_types(&emulator, domain, "once", &run_id).await;
    assert_eq!(
        seen,
        vec![
            "WorkflowExecutionStarted",
            "DecisionTaskScheduled",
            "DecisionTaskStarted",
            "DecisionTaskCompleted",
        ],
        "case \"tokens\": only the one honoured completion is recorded, got {seen:?}"
    );
}

#[tokio::test]
async fn a_decision_this_service_cannot_read_is_refused_and_records_nothing() {
    let emulator = Emulator::new();
    let domain = "bad-decisions";
    seed(&emulator, domain).await;
    let run_id = start(&emulator, "bad", domain, "careful", json!({})).await;
    let token = poll_token(&emulator, "bad", domain, DECIDER_LIST).await;

    let rows = [
        (
            "a decisionType outside the modelled enumeration",
            json!([{ "decisionType": "DoSomethingElse" }]),
            json!({}),
        ),
        (
            "a decision carrying no attributes for its own type",
            json!([{ "decisionType": "StartTimer" }]),
            json!({}),
        ),
        (
            "a timerId SWF's naming rule rejects",
            json!([{
                "decisionType": "StartTimer",
                "startTimerDecisionAttributes": {
                    "timerId": "nope:colon",
                    "startToFireTimeout": "60",
                },
            }]),
            json!({}),
        ),
        (
            "a duration that is not a number of seconds",
            json!([{
                "decisionType": "StartTimer",
                "startTimerDecisionAttributes": { "timerId": "t", "startToFireTimeout": "soon" },
            }]),
            json!({}),
        ),
        (
            "a bad decision after a good one",
            json!([
                {
                    "decisionType": "RecordMarker",
                    "recordMarkerDecisionAttributes": { "markerName": "fine" },
                },
                {
                    "decisionType": "CancelTimer",
                    "cancelTimerDecisionAttributes": { "timerId": "bad|bar" },
                },
            ]),
            json!({}),
        ),
        (
            "a task list override SWF's naming rule rejects",
            json!([]),
            json!({ "taskList": { "name": "arn" } }),
        ),
        (
            "a task list override timeout that is not a number of seconds",
            json!([]),
            json!({
                "taskList": { "name": "elsewhere" },
                "taskListScheduleToStartTimeout": "half an hour",
            }),
        ),
    ];

    for (case, decisions, extra) in rows {
        let mut body = json!({ "taskToken": token, "decisions": decisions });
        merge(&mut body, extra);
        let (status, parsed) = emulator.call("RespondDecisionTaskCompleted", &body).await;
        let code = error_code(&parsed);
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} {code} with \
             {parsed:?}"
        );
    }

    // Nothing above was recorded: the decision task is still outstanding, so the decider
    // can answer it properly.
    let seen = event_types(&emulator, domain, "careful", &run_id).await;
    assert_eq!(
        seen,
        vec![
            "WorkflowExecutionStarted",
            "DecisionTaskScheduled",
            "DecisionTaskStarted",
        ],
        "case \"bad\": a refused request leaves the history untouched, got {seen:?}"
    );
    respond(
        &emulator,
        "bad",
        &token,
        json!([{
            "decisionType": "RecordMarker",
            "recordMarkerDecisionAttributes": { "markerName": "fine" },
        }]),
        json!({}),
    )
    .await;
    let seen = event_types(&emulator, domain, "careful", &run_id).await;
    assert_eq!(
        seen.last().map(String::as_str),
        Some("MarkerRecorded"),
        "case \"bad\": the still-outstanding task is answerable, got {seen:?}"
    );
}
