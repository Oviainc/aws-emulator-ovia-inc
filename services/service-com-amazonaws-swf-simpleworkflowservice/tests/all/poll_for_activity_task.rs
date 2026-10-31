// SPDX-License-Identifier: CC0-1.0

//! `PollForActivityTask`, end to end over the service's own wire protocol (`awsJson1_0`,
//! `POST /` with `X-Amz-Target: SimpleWorkflowService.<Operation>`), dispatched through
//! the same `MultiServiceRouter` the emulator mounts in production.
//!
//! The rule every case here asserts: a poll of an activity task list hands the caller
//! the one activity task of that domain's OPEN runs waiting in that list — highest
//! `taskPriority` first, then first-scheduled first — records the hand-over as an
//! `ActivityTaskStarted` event carrying the caller's `identity` and naming the
//! `ActivityTaskScheduled` event it started, and answers entirely from that stored
//! scheduling event: its `activityId`, its `activityType` and the `input` it was
//! scheduled with, plus the new event's id as `startedEventId`, the run it belongs to and
//! an opaque handle on that one task. A task is handed out at most once, a list with
//! nothing waiting is held open (the call is the documented long poll), a task is timed
//! out by the service itself at the instant the earliest of its own durations runs out,
//! and an unregistered domain is the modelled `UnknownResourceFault`.
//!
//! Every row varies what the rule generalises over — where the task list came from, the
//! shape and size of the `activityId`, the `input` payload (absent, text, the modelled
//! maximum, non-ASCII bytes), the identity's size, how many tasks share one list, their
//! priorities and the order they were scheduled in, and which of the four modelled
//! timeout types runs out first — and every expected value is computed by the test from
//! what it seeded, never hardcoded from one recording.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_swf_simpleworkflowservice::ops::types::{
    ACTIVITY_TASK_TIMEOUT_TYPE_VALUES, EVENT_TYPE_VALUES,
};

use super::workflow_execution_lifecycle::{
    error_code, full_defaults, i64_at, merge, seeded, string_at, whole_history, Emulator,
};

/// The workflow type every run in this suite is started with.
const WORKFLOW_TYPE: (&str, &str) = ("activity-wf", "1.0");

/// The `defaultTaskList` that workflow type carries, so every run's decision tasks wait
/// there.
const DECIDER_LIST: &str = "deciders";

/// The activity type every task in this suite is scheduled from.
const ACTIVITY_TYPE: (&str, &str) = ("process-order", "3.1");

/// The `defaultTaskList` the activity type registers, so a `ScheduleActivityTask`
/// decision that names no task list schedules there.
const REGISTERED_LIST: &str = "registered-activities";

/// The modelled `TaskToken` maximum length (`len 1..1024`).
const MAX_TASK_TOKEN_LENGTH: usize = 1024;

/// The modelled `Data` maximum length (`len 0..32768`), which an `input` payload may
/// reach.
const MAX_DATA_LENGTH: usize = 32768;

/// The modelled `ActivityId` maximum length (`len 1..256`).
const MAX_ACTIVITY_ID_LENGTH: usize = 256;

/// The modelled `Identity` maximum length (`len 0..256`).
const MAX_IDENTITY_LENGTH: usize = 256;

/// How long a poll of a task list with nothing waiting is given to prove it is NOT
/// answering. The documented long-poll timeout is 60 seconds, so any wait far below it
/// that produces no answer shows the connection was held open.
const HOLD_OPEN_PROBE: Duration = Duration::from_millis(250);

/// The duration the timeout rows give a task, in whole seconds as every SWF duration
/// member is specified.
const SHORT_TIMEOUT: &str = "1";

/// How long a timeout row waits past [`SHORT_TIMEOUT`] before reading the run back. The
/// sweep is driven by the wall clock, so the wait only has to cross the deadline.
const PAST_SHORT_TIMEOUT: Duration = Duration::from_millis(1_400);

/// Register the domain, the workflow type the runs use and the activity type the tasks
/// are scheduled from.
async fn seed(emulator: &Emulator, domain: &str) {
    seeded(
        emulator,
        domain,
        domain,
        WORKFLOW_TYPE,
        full_defaults(DECIDER_LIST, "TERMINATE"),
    )
    .await;
    emulator
        .ok(
            domain,
            "RegisterActivityType",
            &json!({
                "domain": domain,
                "name": ACTIVITY_TYPE.0,
                "version": ACTIVITY_TYPE.1,
                "defaultTaskList": { "name": REGISTERED_LIST },
                "defaultTaskScheduleToStartTimeout": "60",
                "defaultTaskScheduleToCloseTimeout": "600",
                "defaultTaskStartToCloseTimeout": "300",
            }),
        )
        .await;
}

/// Start one run of the seeded workflow type and return its minted `runId`.
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

/// Drive the run's decider once: poll its decision task and complete it with
/// `decisions`. This is how every activity task in this suite comes to wait in a list —
/// the one producer of an `ActivityTaskScheduled` event.
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
    let answered = emulator
        .ok(
            case,
            "RespondDecisionTaskCompleted",
            &json!({ "taskToken": token, "decisions": decisions }),
        )
        .await;
    assert_eq!(
        answered,
        json!({}),
        "case {case:?}: RespondDecisionTaskCompleted answers with an empty body, got \
         {answered:?}"
    );
}

/// One `ScheduleActivityTask` decision for `activity_id`, with `extra` folded into its
/// attributes (the task list, the payload and the durations a row exercises).
fn schedule_decision(activity_id: &str, extra: Value) -> Value {
    let mut attributes = json!({
        "activityType": { "name": ACTIVITY_TYPE.0, "version": ACTIVITY_TYPE.1 },
        "activityId": activity_id,
    });
    merge(&mut attributes, extra);
    json!({
        "decisionType": "ScheduleActivityTask",
        "scheduleActivityTaskDecisionAttributes": attributes,
    })
}

/// One `PollForActivityTask` call, requiring a 200.
async fn poll(
    emulator: &Emulator,
    case: &str,
    domain: &str,
    task_list: &str,
    identity: Option<&str>,
) -> Value {
    let mut body = json!({ "domain": domain, "taskList": { "name": task_list } });
    if let Some(identity) = identity {
        merge(&mut body, json!({ "identity": identity }));
    }
    emulator.ok(case, "PollForActivityTask", &body).await
}

/// How many activity tasks are waiting in one list, as `CountPendingActivityTasks`
/// reports it.
async fn pending(emulator: &Emulator, case: &str, domain: &str, task_list: &str) -> i64 {
    let body = emulator
        .ok(
            case,
            "CountPendingActivityTasks",
            &json!({ "domain": domain, "taskList": { "name": task_list } }),
        )
        .await;
    i64_at(&body, "/count")
}

/// How many decision tasks are waiting in the decider list, as
/// `CountPendingDecisionTasks` reports it.
async fn pending_decisions(emulator: &Emulator, case: &str, domain: &str) -> i64 {
    let body = emulator
        .ok(
            case,
            "CountPendingDecisionTasks",
            &json!({ "domain": domain, "taskList": { "name": DECIDER_LIST } }),
        )
        .await;
    i64_at(&body, "/count")
}

/// `DescribeWorkflowExecution`'s `openCounts.openActivityTasks` for one run.
async fn open_activity_tasks(
    emulator: &Emulator,
    case: &str,
    domain: &str,
    workflow_id: &str,
    run_id: &str,
) -> i64 {
    let described = emulator
        .ok(
            case,
            "DescribeWorkflowExecution",
            &json!({
                "domain": domain,
                "execution": { "workflowId": workflow_id, "runId": run_id },
            }),
        )
        .await;
    i64_at(&described, "/openCounts/openActivityTasks")
}

/// The events of a history with the given `eventType`, oldest first.
fn events_typed(events: &[Value], event_type: &str) -> Vec<Value> {
    events
        .iter()
        .filter(|event| string_at(event, "/eventType") == event_type)
        .cloned()
        .collect()
}

/// Assert a poll of a task list with NOTHING waiting holds the connection open instead
/// of answering.
///
/// The call is a long poll — "the service holds the HTTP connection open and responds as
/// soon as a task becomes available. The maximum time the service holds on to the request
/// before responding is 60 seconds" — so a drained list answers nothing for that long;
/// [`HOLD_OPEN_PROBE`] is far below the timeout, and a poll that answers inside it is the
/// failure.
async fn assert_holds_open(emulator: &Emulator, case: &str, domain: &str, task_list: &str) {
    let answered = tokio::time::timeout(
        HOLD_OPEN_PROBE,
        emulator.call(
            "PollForActivityTask",
            &json!({ "domain": domain, "taskList": { "name": task_list } }),
        ),
    )
    .await;
    assert!(
        answered.is_err(),
        "case {case:?}: a poll of a task list with no activity task waiting holds the \
         connection open rather than answering, got {answered:?}"
    );
}

/// One scheduled-then-polled activity task: where its task list came from, the shape of
/// its id, the payload it was scheduled with and the identity the poll passes.
struct HandoutRow {
    case: &'static str,
    workflow_id: &'static str,
    activity_id: String,
    /// `None` schedules the task with no `input` member at all.
    input: Option<String>,
    /// `None` polls with no `identity` member at all.
    identity: Option<String>,
    /// `None` leaves the task on the activity type's registered `defaultTaskList`.
    task_list: Option<String>,
}

impl HandoutRow {
    /// The list this row's task waits in: the one its decision named, else the activity
    /// type's registered default.
    fn list(&self) -> String {
        self.task_list
            .clone()
            .unwrap_or_else(|| REGISTERED_LIST.to_string())
    }
}

/// The hand-out rows: the payload absent, short, at the modelled `Data` maximum and
/// non-ASCII; the id plain, at the modelled maximum and holding the characters SWF's
/// naming rule allows; the identity absent, empty and at the modelled maximum; and the
/// task list taken from the decision or from the registration.
fn handout_rows() -> Vec<HandoutRow> {
    vec![
        HandoutRow {
            case: "no input, no identity, the registered list",
            workflow_id: "wf-plain",
            activity_id: "order-1".to_string(),
            input: None,
            identity: None,
            task_list: None,
        },
        HandoutRow {
            case: "a text payload and a named worker on an overridden list",
            workflow_id: "wf-text",
            activity_id: "order.2-a_b".to_string(),
            input: Some("{\"orderId\":\"A-1\"}".to_string()),
            identity: Some("worker-1".to_string()),
            task_list: Some("decision-activities".to_string()),
        },
        HandoutRow {
            case: "the longest modelled payload and identity",
            workflow_id: "wf-max",
            activity_id: "o".repeat(MAX_ACTIVITY_ID_LENGTH),
            input: Some("p".repeat(MAX_DATA_LENGTH)),
            identity: Some("w".repeat(MAX_IDENTITY_LENGTH)),
            task_list: Some("max-activities".to_string()),
        },
        HandoutRow {
            case: "a non-ASCII payload and an empty identity",
            workflow_id: "wf-bytes",
            activity_id: "order-\u{1f4e6}".to_string(),
            input: Some("\u{1f4e6}\u{00e9}\u{0416} \t\n{\"b\":\"\u{fffd}\"}".to_string()),
            identity: Some(String::new()),
            task_list: Some("bytes-activities".to_string()),
        },
        HandoutRow {
            case: "an empty payload, which is inside the modelled minimum",
            workflow_id: "wf-empty-input",
            activity_id: "order-5".to_string(),
            input: Some(String::new()),
            identity: Some("worker-5".to_string()),
            task_list: Some("empty-activities".to_string()),
        },
    ]
}

#[tokio::test]
async fn a_poll_hands_out_the_waiting_task_and_records_the_hand_over() {
    let emulator = Emulator::new();
    let domain = "handout";
    seed(&emulator, domain).await;

    let mut tokens: BTreeSet<String> = BTreeSet::new();
    for row in handout_rows() {
        let case = row.case;
        let list = row.list();
        let run_id = start(&emulator, case, domain, row.workflow_id).await;

        let mut extra = json!({});
        if let Some(task_list) = &row.task_list {
            merge(&mut extra, json!({ "taskList": { "name": task_list } }));
        }
        if let Some(input) = &row.input {
            merge(&mut extra, json!({ "input": input }));
        }
        decide(
            &emulator,
            case,
            domain,
            json!([schedule_decision(&row.activity_id, extra)]),
        )
        .await;

        let waiting = pending(&emulator, case, domain, &list).await;
        assert_eq!(
            waiting, 1,
            "case {case:?}: the scheduled task waits in {list:?} before the poll, got \
             {waiting}"
        );

        let answer = poll(&emulator, case, domain, &list, row.identity.as_deref()).await;

        // Every member of the answer is the scheduling event's, read back out of state.
        let stored = whole_history(&emulator, domain, row.workflow_id, &run_id).await;
        let scheduled = events_typed(&stored, "ActivityTaskScheduled");
        assert_eq!(
            scheduled.len(),
            1,
            "case {case:?}: the decision scheduled exactly one activity task, got \
             {stored:?}"
        );
        let scheduled_event_id = i64_at(&scheduled[0], "/eventId");
        assert_eq!(
            (
                string_at(&answer, "/activityId"),
                string_at(&answer, "/activityType/name"),
                string_at(&answer, "/activityType/version"),
                string_at(&answer, "/workflowExecution/workflowId"),
                string_at(&answer, "/workflowExecution/runId"),
            ),
            (
                row.activity_id.clone(),
                ACTIVITY_TYPE.0.to_string(),
                ACTIVITY_TYPE.1.to_string(),
                row.workflow_id.to_string(),
                run_id.clone(),
            ),
            "case {case:?}: the answer identifies the scheduled task and its run, got \
             {answer:?}"
        );
        assert_eq!(
            answer.get("input").and_then(Value::as_str),
            row.input.as_deref(),
            "case {case:?}: the answer carries the input the task was scheduled with, got \
             {answer:?}"
        );

        // The hand-over is one ActivityTaskStarted event: its id is the answer's
        // startedEventId, it names the scheduling event it started, and it carries the
        // identity the poll passed (and no identity member when none was passed).
        let started = events_typed(&stored, "ActivityTaskStarted");
        assert_eq!(
            started.len(),
            1,
            "case {case:?}: the poll records exactly one ActivityTaskStarted event, got \
             {stored:?}"
        );
        let started_event_id = i64_at(&started[0], "/eventId");
        assert_eq!(
            (
                i64_at(&answer, "/startedEventId"),
                i64_at(
                    &started[0],
                    "/activityTaskStartedEventAttributes/scheduledEventId"
                ),
            ),
            (started_event_id, scheduled_event_id),
            "case {case:?}: the recorded hand-over is the answer's startedEventId and \
             names the task it started, got {started:?} for {answer:?}"
        );
        let recorded_identity = started[0]
            .pointer("/activityTaskStartedEventAttributes/identity")
            .and_then(Value::as_str);
        let expected_identity = row.identity.as_deref().filter(|value| !value.is_empty());
        assert_eq!(
            recorded_identity, expected_identity,
            "case {case:?}: the hand-over records the worker's identity, got {started:?}"
        );
        assert!(
            started_event_id > scheduled_event_id,
            "case {case:?}: the hand-over is recorded after the scheduling it names, got \
             {started_event_id} after {scheduled_event_id}"
        );

        // The token is an opaque handle inside the modelled length, and no two tasks
        // share one.
        let token = string_at(&answer, "/taskToken");
        assert!(
            !token.is_empty() && token.len() <= MAX_TASK_TOKEN_LENGTH,
            "case {case:?}: the answered token is non-empty and inside the modelled \
             maximum of {MAX_TASK_TOKEN_LENGTH}, got {} characters",
            token.len()
        );
        assert!(
            !token.contains(&row.activity_id) && !token.contains(&run_id),
            "case {case:?}: the token is opaque, so it spells out neither the activityId \
             nor the runId, got {token:?}"
        );
        assert!(
            tokens.insert(token.clone()),
            "case {case:?}: each task gets its own handle, got the token {token:?} twice"
        );

        // The task left its list and is still in flight with its worker, and the
        // run's history holds no event type the model does not name.
        let left = pending(&emulator, case, domain, &list).await;
        let open = open_activity_tasks(&emulator, case, domain, row.workflow_id, &run_id).await;
        assert_eq!(
            (left, open),
            (0, 1),
            "case {case:?}: the handed-out task waits in no list and is still open with \
             its worker, got {left} waiting and {open} open"
        );
        for event in &stored {
            let event_type = string_at(event, "/eventType");
            assert!(
                EVENT_TYPE_VALUES.contains(&event_type.as_str()),
                "case {case:?}: {event_type:?} is not in the model's EventType catalogue"
            );
        }

        // Nothing else is waiting in that list, so a second poll is held open.
        assert_holds_open(&emulator, case, domain, &list).await;
    }
}

/// One task of the dispatch-order case: which run scheduled it, its `taskPriority` and
/// the order it was scheduled in.
struct OrderedTask {
    activity_id: &'static str,
    workflow_id: &'static str,
    /// `None` schedules the task with no `taskPriority`, which SWF dispatches at 0.
    priority: Option<&'static str>,
}

#[tokio::test]
async fn tasks_are_handed_out_highest_priority_first_then_first_scheduled_first() {
    let emulator = Emulator::new();
    let domain = "ordering";
    let list = "ordered-activities";
    seed(&emulator, domain).await;

    // Two runs sharing one list, scheduling in a different order from the dispatch
    // order: priorities above, below and at the default, plus two tasks that tie on
    // priority and are separated only by when they were scheduled.
    let tasks = [
        OrderedTask {
            activity_id: "a-default",
            workflow_id: "wf-first",
            priority: None,
        },
        OrderedTask {
            activity_id: "b-high",
            workflow_id: "wf-first",
            priority: Some("10"),
        },
        OrderedTask {
            activity_id: "c-low",
            workflow_id: "wf-second",
            priority: Some("-5"),
        },
        OrderedTask {
            activity_id: "d-high",
            workflow_id: "wf-second",
            priority: Some("10"),
        },
        OrderedTask {
            activity_id: "e-zero",
            workflow_id: "wf-second",
            priority: Some("0"),
        },
    ];

    // Each run's decider schedules its own tasks, in the order the table lists them.
    for workflow_id in ["wf-first", "wf-second"] {
        start(&emulator, workflow_id, domain, workflow_id).await;
        let decisions: Vec<Value> = tasks
            .iter()
            .filter(|task| task.workflow_id == workflow_id)
            .map(|task| {
                let mut extra = json!({ "taskList": { "name": list } });
                if let Some(priority) = task.priority {
                    merge(&mut extra, json!({ "taskPriority": priority }));
                }
                schedule_decision(task.activity_id, extra)
            })
            .collect();
        decide(&emulator, workflow_id, domain, Value::Array(decisions)).await;
    }

    // The rule, applied by the TEST: higher priority first, then first-scheduled first.
    // `wf-first` scheduled before `wf-second`, and within a run the table's order is the
    // scheduling order.
    let mut expected: Vec<&OrderedTask> = tasks.iter().collect();
    expected.sort_by_key(|task| {
        let priority: i64 = task.priority.unwrap_or("0").parse().expect("a priority");
        let scheduled_after = tasks
            .iter()
            .position(|other| other.activity_id == task.activity_id)
            .expect("the task is in the table");
        let run_order = usize::from(task.workflow_id == "wf-second");
        (-priority, run_order, scheduled_after)
    });
    let expected: Vec<&str> = expected.iter().map(|task| task.activity_id).collect();

    let mut handed_out = Vec::new();
    for position in 0..tasks.len() {
        let case = format!("task {} of {}", position + 1, tasks.len());
        let answer = poll(&emulator, &case, domain, list, Some("worker")).await;
        handed_out.push(string_at(&answer, "/activityId"));
    }
    assert_eq!(
        handed_out, expected,
        "the list hands its tasks out highest priority first, then first scheduled \
         first; got {handed_out:?}"
    );

    // Every task was handed out exactly once and the list is now empty, so a further
    // poll waits instead of handing one out again.
    let waiting = pending(&emulator, "drained", domain, list).await;
    assert_eq!(
        waiting, 0,
        "every task was handed out, so none is still waiting, got {waiting}"
    );
    assert_holds_open(&emulator, "drained", domain, list).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_pollers_of_one_list_are_each_handed_a_different_task() {
    // A task is handed out at most once, however many workers ask at the same time: the
    // hand-over is recorded inside the one state lock, so two pollers cannot be given
    // the same `ActivityTaskScheduled` event.
    let emulator = std::sync::Arc::new(Emulator::new());
    let domain = "concurrent";
    let list = "concurrent-activities";
    seed(&emulator, domain).await;

    // Rows: as many waiting tasks as there are pollers, scheduled by one decider.
    let activity_ids: Vec<String> = (0..4).map(|index| format!("order-{index}")).collect();
    start(&emulator, "concurrent", domain, "wf-concurrent").await;
    let decisions: Vec<Value> = activity_ids
        .iter()
        .map(|activity_id| {
            schedule_decision(activity_id, json!({ "taskList": { "name": list } }))
        })
        .collect();
    decide(&emulator, "concurrent", domain, Value::Array(decisions)).await;

    let polling: Vec<tokio::task::JoinHandle<Value>> = (0..activity_ids.len())
        .map(|index| {
            let emulator = std::sync::Arc::clone(&emulator);
            let domain = domain.to_string();
            let list = list.to_string();
            tokio::spawn(async move {
                let identity = format!("worker-{index}");
                poll(&emulator, "concurrent", &domain, &list, Some(&identity)).await
            })
        })
        .collect();

    let mut handed_out: BTreeSet<String> = BTreeSet::new();
    let mut tokens: BTreeSet<String> = BTreeSet::new();
    for handle in polling {
        let answer = tokio::time::timeout(Duration::from_secs(30), handle)
            .await
            .expect("each poller is handed one of the waiting tasks")
            .expect("the polling task did not panic");
        handed_out.insert(string_at(&answer, "/activityId"));
        tokens.insert(string_at(&answer, "/taskToken"));
    }
    let expected: BTreeSet<String> = activity_ids.iter().cloned().collect();
    assert_eq!(
        (handed_out.clone(), tokens.len()),
        (expected, activity_ids.len()),
        "case \"concurrent\": every waiting task went to exactly one poller, each with \
         its own handle, got {handed_out:?}"
    );
    let waiting = pending(&emulator, "concurrent", domain, list).await;
    assert_eq!(
        waiting, 0,
        "case \"concurrent\": every task was handed out, so none is still waiting, got \
         {waiting}"
    );
}

#[tokio::test]
async fn an_unknown_domain_or_a_task_list_name_outside_the_naming_rule_is_refused() {
    let emulator = Emulator::new();
    let domain = "refusals";
    seed(&emulator, domain).await;

    // A task list lives inside a domain, so a domain that was never registered — or one
    // differing only in case — addresses no task list at all.
    for unknown in ["never-registered", "REFUSALS", "refusals-2"] {
        emulator
            .refused(
                unknown,
                "PollForActivityTask",
                &json!({ "domain": unknown, "taskList": { "name": REGISTERED_LIST } }),
                "UnknownResourceFault",
            )
            .await;
    }

    // The naming rule stated on every user-supplied SWF name, `taskList.name` included:
    // "The specified string must not start or end with whitespace. It must not contain a
    // : (colon), / (slash), | (vertical bar), or any control characters
    // (\u0000-\u001f | \u007f-\u009f). Also, it must not be the literal string arn."
    let rejected = [
        ("leading whitespace", " activities"),
        ("trailing whitespace", "activities "),
        ("a colon", "activities:1"),
        ("a slash", "activities/1"),
        ("a vertical bar", "activities|1"),
        ("a control character", "activities\u{1}"),
        ("a high control character", "activities\u{7f}"),
        ("the literal arn", "arn"),
    ];
    for (case, task_list) in rejected {
        emulator
            .refused(
                case,
                "PollForActivityTask",
                &json!({ "domain": domain, "taskList": { "name": task_list } }),
                "ValidationException",
            )
            .await;
    }

    // A refused poll records nothing: the run that was never polled keeps its history.
    let run_id = start(&emulator, "untouched", domain, "wf-untouched").await;
    let before = whole_history(&emulator, domain, "wf-untouched", &run_id).await;
    let (status, body) = emulator
        .call(
            "PollForActivityTask",
            &json!({ "domain": "never-registered", "taskList": { "name": REGISTERED_LIST } }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "UnknownResourceFault"),
        "a poll of an unregistered domain is 400 UnknownResourceFault, got {body:?}"
    );
    let after = whole_history(&emulator, domain, "wf-untouched", &run_id).await;
    assert_eq!(
        before, after,
        "case \"untouched\": a refused poll changes no run's history, got {after:?}"
    );
}

/// One task whose own durations either have or have not run out by the time the run is
/// read back.
struct TimeoutRow {
    case: &'static str,
    workflow_id: &'static str,
    /// The durations the `ScheduleActivityTask` decision carries.
    schedule_to_start: &'static str,
    schedule_to_close: &'static str,
    start_to_close: &'static str,
    heartbeat: Option<&'static str>,
    /// Whether a worker is handed the task before the wait.
    polled: bool,
    /// The `timeoutType` the service must record, or `None` when nothing is due.
    expected: Option<&'static str>,
}

#[tokio::test]
async fn a_task_times_out_under_the_earliest_of_its_own_durations() {
    // One row per modelled `ActivityTaskTimeoutType`, plus a row whose every duration is
    // the published unlimited sentinel so nothing is due. Each duration is the task's
    // own, so the row that runs out is the row whose duration this test shortened.
    let rows = [
        TimeoutRow {
            case: "nobody takes it before its schedule-to-start runs out",
            workflow_id: "wf-schedule-to-start",
            schedule_to_start: SHORT_TIMEOUT,
            schedule_to_close: "NONE",
            start_to_close: "NONE",
            heartbeat: None,
            polled: false,
            expected: Some("SCHEDULE_TO_START"),
        },
        TimeoutRow {
            case: "the whole task's budget runs out while it waits",
            workflow_id: "wf-schedule-to-close",
            schedule_to_start: "NONE",
            schedule_to_close: SHORT_TIMEOUT,
            start_to_close: "NONE",
            heartbeat: None,
            polled: false,
            expected: Some("SCHEDULE_TO_CLOSE"),
        },
        TimeoutRow {
            case: "a worker holds it past its start-to-close",
            workflow_id: "wf-start-to-close",
            schedule_to_start: "NONE",
            schedule_to_close: "NONE",
            start_to_close: SHORT_TIMEOUT,
            heartbeat: None,
            polled: true,
            expected: Some("START_TO_CLOSE"),
        },
        TimeoutRow {
            case: "a worker reports no progress inside its heartbeat",
            workflow_id: "wf-heartbeat",
            schedule_to_start: "NONE",
            schedule_to_close: "NONE",
            start_to_close: "NONE",
            heartbeat: Some(SHORT_TIMEOUT),
            polled: true,
            expected: Some("HEARTBEAT"),
        },
        TimeoutRow {
            case: "every duration unlimited, so nothing is due",
            workflow_id: "wf-unlimited",
            schedule_to_start: "NONE",
            schedule_to_close: "NONE",
            start_to_close: "NONE",
            heartbeat: None,
            polled: false,
            expected: None,
        },
    ];

    for row in rows {
        let emulator = Emulator::new();
        let domain = "timeouts";
        let list = format!("activities-{}", row.workflow_id);
        seed(&emulator, domain).await;
        let run_id = start(&emulator, row.case, domain, row.workflow_id).await;

        let mut extra = json!({
            "taskList": { "name": list },
            "scheduleToStartTimeout": row.schedule_to_start,
            "scheduleToCloseTimeout": row.schedule_to_close,
            "startToCloseTimeout": row.start_to_close,
        });
        if let Some(heartbeat) = row.heartbeat {
            merge(&mut extra, json!({ "heartbeatTimeout": heartbeat }));
        }
        decide(
            &emulator,
            row.case,
            domain,
            json!([schedule_decision(row.workflow_id, extra)]),
        )
        .await;

        let started_event_id = if row.polled {
            let answer = poll(&emulator, row.case, domain, &list, Some("worker")).await;
            i64_at(&answer, "/startedEventId")
        } else {
            0
        };

        tokio::time::sleep(PAST_SHORT_TIMEOUT).await;
        // Reading the run back is what materialises what the clock already owed it.
        let stored = whole_history(&emulator, domain, row.workflow_id, &run_id).await;
        let timed_out = events_typed(&stored, "ActivityTaskTimedOut");

        let Some(expected) = row.expected else {
            assert!(
                timed_out.is_empty(),
                "case {:?}: a task with no duration to run out times nothing out, got \
                 {stored:?}",
                row.case
            );
            let still_there = if row.polled {
                open_activity_tasks(&emulator, row.case, domain, row.workflow_id, &run_id).await
            } else {
                pending(&emulator, row.case, domain, &list).await
            };
            assert_eq!(
                still_there, 1,
                "case {:?}: the task is still in flight, got {still_there}",
                row.case
            );
            continue;
        };

        assert_eq!(
            timed_out.len(),
            1,
            "case {:?}: exactly one timeout is recorded for one task, got {stored:?}",
            row.case
        );
        let attributes = "/activityTaskTimedOutEventAttributes";
        let recorded = string_at(&timed_out[0], &format!("{attributes}/timeoutType"));
        assert!(
            ACTIVITY_TASK_TIMEOUT_TYPE_VALUES.contains(&recorded.as_str()),
            "case {:?}: {recorded:?} is not in the model's ActivityTaskTimeoutType \
             catalogue",
            row.case
        );
        let scheduled = events_typed(&stored, "ActivityTaskScheduled");
        assert_eq!(
            (
                recorded.as_str(),
                i64_at(&timed_out[0], &format!("{attributes}/scheduledEventId")),
                i64_at(&timed_out[0], &format!("{attributes}/startedEventId")),
            ),
            (
                expected,
                i64_at(&scheduled[0], "/eventId"),
                started_event_id,
            ),
            "case {:?}: the timeout names the duration that ran out, the task it ended \
             and the hand-over it closed, got {timed_out:?}",
            row.case
        );
        assert!(
            timed_out[0]
                .pointer(&format!("{attributes}/details"))
                .is_none(),
            "case {:?}: a task whose worker reported no progress carries no heartbeat \
             details, got {timed_out:?}",
            row.case
        );

        // A timed-out task is out of flight and out of its list, and the decider is told
        // by the decision task the timeout scheduled.
        let waiting = pending(&emulator, row.case, domain, &list).await;
        let open = open_activity_tasks(&emulator, row.case, domain, row.workflow_id, &run_id).await;
        let decisions = pending_decisions(&emulator, row.case, domain).await;
        assert_eq!(
            (waiting, open, decisions),
            (0, 0, 1),
            "case {:?}: a timed-out task waits nowhere, is out of flight and gets the \
             decider a decision task, got {waiting} waiting, {open} open and {decisions} \
             decision task(s)",
            row.case
        );

        // The list is empty, so a poll arriving after the timeout is held open rather
        // than handed the task that is over.
        assert_holds_open(&emulator, row.case, domain, &list).await;
    }
}
