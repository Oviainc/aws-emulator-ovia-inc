// SPDX-License-Identifier: CC0-1.0

//! One execution at a time per managed device, end to end over HTTP.
//!
//! The rule under test: a managed device performs ONE task execution at a time
//! (`CreateTask`'s `command` member — "The task to be performed. Only one task is
//! executed on a device at a time", `smithy_query(query="operation",operation="CreateTask")`),
//! so the executions a device accumulates form a queue: at most one of them is
//! `IN_PROGRESS` at any instant, the rest report `QUEUED`, and each leaves the queue —
//! and applies its command — only once the one ahead of it is terminal. The queue is per
//! DEVICE, so work on one device never holds up another device's.
//!
//! Rows differ along what the rule generalises over: how many tasks are stacked on one
//! device, the commands they carry (which is what decides the effect on the device), and
//! whether the last of them also targets a device that is idle. The expected states and
//! instants are COMPUTED from the queue each execution landed in, so no row is fitted to
//! one request.
//!
//! Every case goes through the router, so an answer that is not computed from persisted
//! state cannot pass.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_snowdevicemanagement_snowdevicemanagement::state::{
    EXECUTION_FINISH_SPAN, EXECUTION_START_SPAN,
};
use smithy_server_runtime::MultiServiceRouter;

use super::device_task_lifecycle::{
    create_task, describe_device, describe_execution, describe_task, emulator, ids_of, ok, string,
    whole_inventory,
};

/// Slack added to a span before reading the state the clock owes past it, so a loaded
/// host cannot read a record before the due instant it stores has passed.
const SETTLE_SLACK: Duration = Duration::from_millis(80);

/// How often the drain below reads the queue.
const POLL_EVERY: Duration = Duration::from_millis(40);

/// How long past the queue's own length the drain waits before giving up on it.
const POLL_GRACE: Duration = Duration::from_secs(10);

/// How far two instants reported as epoch seconds may differ and still count as the
/// same instant: the two ends of one hand-off are the SAME stored instant, rendered
/// through JSON on either side of it.
const TOLERANCE: f64 = 1e-3;

/// `ExecutionState`'s terminal values, from
/// `smithy_query(query="shape",shape="ExecutionState")`: the states no further change is
/// owed from.
const TERMINAL_STATES: &[&str] = &["SUCCEEDED", "FAILED", "CANCELED", "REJECTED", "TIMED_OUT"];

/// One row: the commands of the tasks stacked on ONE device, in creation order, and
/// whether the last of them also targets a second device that is idle.
struct Row {
    case: &'static str,
    commands: &'static [&'static str],
    also_an_idle_device: bool,
}

/// How far along the queue a state is: queued, running, or done with. The queue may only
/// ever move forward along this order, and never past the execution ahead of it.
fn rank(state: &str) -> u8 {
    match state {
        "QUEUED" => 0,
        "IN_PROGRESS" => 1,
        _ => 2,
    }
}

/// The complaint the queue invariant has about one reading of a device's executions, in
/// the order they were created, or `None` when the reading satisfies it.
fn queue_violation(states: &[String]) -> Option<String> {
    let running = states
        .iter()
        .filter(|state| state.as_str() == "IN_PROGRESS")
        .count();
    if running > 1 {
        return Some(format!(
            "{running} of the device's executions report IN_PROGRESS at once ({states:?})"
        ));
    }
    for (index, state) in states.iter().enumerate().skip(1) {
        if rank(state) > rank(&states[index - 1]) {
            return Some(format!(
                "execution {index} reports {state:?} while the one ahead of it in the queue \
                 reports {:?} ({states:?})",
                states[index - 1]
            ));
        }
    }
    None
}

/// A required instant member, as the epoch seconds the wire carries it as.
fn epoch(body: &Value, member: &str, case: &str) -> f64 {
    body[member]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: expected an instant {member}, got {body}"))
}

/// The `state` of one execution, read back through `DescribeExecution`.
async fn execution_state(
    router: &MultiServiceRouter,
    task_id: &str,
    device: &str,
    case: &str,
) -> String {
    let body = ok(&describe_execution(router, task_id, device).await, case);
    string(&body, "state", case)
}

/// The states of one device's executions, one per task, in the order the tasks were
/// created.
async fn queue_states(
    router: &MultiServiceRouter,
    tasks: &[String],
    device: &str,
    case: &str,
) -> Vec<String> {
    let mut states = Vec::with_capacity(tasks.len());
    for task_id in tasks {
        states.push(execution_state(router, task_id, device, case).await);
    }
    states
}

#[tokio::test]
async fn a_device_performs_one_execution_at_a_time_in_the_order_they_were_created() {
    let rows = [
        Row {
            case: "two tasks on one device",
            commands: &["unlock", "reboot"],
            also_an_idle_device: false,
        },
        Row {
            case: "three tasks on one device",
            commands: &["reboot", "unlock", "reboot"],
            also_an_idle_device: false,
        },
        Row {
            case: "two reboots on one device",
            commands: &["reboot", "reboot"],
            also_an_idle_device: false,
        },
        Row {
            case: "two tasks on one device, the second also targeting an idle device",
            commands: &["reboot", "unlock"],
            also_an_idle_device: true,
        },
    ];

    for row in rows {
        let case = row.case;
        let router = emulator();
        let inventory = ids_of(&whole_inventory(&router, case).await, case);
        let busy = inventory[0].clone();
        let idle = inventory[1].clone();
        let reached_out_before = epoch(
            &ok(&describe_device(&router, &busy).await, case),
            "lastReachedOutAt",
            case,
        );

        // The tasks, created back to back: every one of them targets the same device.
        let mut tasks: Vec<String> = Vec::new();
        for (index, command) in row.commands.iter().enumerate() {
            let last = index + 1 == row.commands.len();
            let targets = if last && row.also_an_idle_device {
                vec![busy.clone(), idle.clone()]
            } else {
                vec![busy.clone()]
            };
            let created = ok(
                &create_task(
                    &router,
                    json!({ "targets": targets, "command": { *command: {} } }),
                )
                .await,
                case,
            );
            tasks.push(string(&created, "taskId", case));
        }

        // Nothing has been handed to the device the instant the creates return.
        let states = queue_states(&router, &tasks, &busy, case).await;
        for (index, state) in states.iter().enumerate() {
            assert_eq!(
                state, "QUEUED",
                "case {case:?}: execution {index} has not been dispatched yet, got {state:?}"
            );
        }

        // Past the dispatch span the device is performing exactly ONE of them, the one
        // at the head of the queue; every other one is still waiting for it.
        tokio::time::sleep(EXECUTION_START_SPAN + SETTLE_SLACK).await;
        let states = queue_states(&router, &tasks, &busy, case).await;
        assert_eq!(
            states[0], "IN_PROGRESS",
            "case {case:?}: past the dispatch span the device performs the first execution, \
             got {states:?}"
        );
        for (index, state) in states.iter().enumerate().skip(1) {
            assert_eq!(
                state, "QUEUED",
                "case {case:?}: a device performs one execution at a time, so execution \
                 {index} must still be QUEUED while the first one runs, got {states:?}"
            );
        }
        if row.also_an_idle_device {
            // The queue is per DEVICE: an idle device's execution is not held up by
            // another device's work, even when the two belong to one task.
            let elsewhere = execution_state(&router, tasks.last().expect("a row has tasks"), &idle, case).await;
            assert_eq!(
                elsewhere, "IN_PROGRESS",
                "case {case:?}: the idle device has nothing else to perform, so its \
                 execution runs on time, got {elsewhere:?}"
            );
        }

        // Drain the queue, checking every reading of it against the invariant.
        let chain = (EXECUTION_START_SPAN + EXECUTION_FINISH_SPAN)
            * u32::try_from(row.commands.len() + 1).expect("a row holds a handful of tasks");
        let deadline = Instant::now() + chain + POLL_GRACE;
        loop {
            let states = queue_states(&router, &tasks, &busy, case).await;
            if let Some(violation) = queue_violation(&states) {
                // These reads are not one snapshot: a transition landing between two of
                // them can show the execution that ended and the one that started
                // together, so a complaint is confirmed against a second reading before
                // it is reported.
                let again = queue_states(&router, &tasks, &busy, case).await;
                assert!(
                    queue_violation(&again).is_none(),
                    "case {case:?}: a device performs one execution at a time, in the order \
                     they were created: {violation}, and again on a second reading {again:?}"
                );
            }
            if states
                .iter()
                .all(|state| TERMINAL_STATES.contains(&state.as_str()))
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: the queue must drain one execution after another, got \
                 {states:?}"
            );
            tokio::time::sleep(POLL_EVERY).await;
        }

        // The persisted record of each run, which is the invariant without a race in it:
        // the runs of one device do not overlap, each lasted the span an execution takes,
        // and they are in the order the tasks were created.
        let mut ended_before: Option<f64> = None;
        let mut started_on_busy: Vec<f64> = Vec::new();
        for (index, task_id) in tasks.iter().enumerate() {
            let body = ok(&describe_execution(&router, task_id, &busy).await, case);
            let state = string(&body, "state", case);
            assert_eq!(
                state, "SUCCEEDED",
                "case {case:?}: execution {index} ran to its own outcome, got {state:?}"
            );
            let started = epoch(&body, "startedAt", case);
            let ended = epoch(&body, "lastUpdatedAt", case);
            assert!(
                ended - started >= EXECUTION_FINISH_SPAN.as_secs_f64() - TOLERANCE,
                "case {case:?}: execution {index} must run for the whole span it takes, \
                 got {} s",
                ended - started
            );
            if let Some(ended_before) = ended_before {
                assert!(
                    started >= ended_before - TOLERANCE,
                    "case {case:?}: execution {index} started at {started}, inside the run \
                     of the one ahead of it in the queue, which ended at {ended_before}"
                );
            }
            ended_before = Some(ended);
            started_on_busy.push(started);
        }

        if row.also_an_idle_device {
            // The same task on an idle device ran straight away, which is what makes the
            // wait above a property of the DEVICE and not of the task.
            let body = ok(
                &describe_execution(&router, tasks.last().expect("a row has tasks"), &idle).await,
                case,
            );
            let state = string(&body, "state", case);
            assert_eq!(
                state, "SUCCEEDED",
                "case {case:?}: the idle device's execution ran to its own outcome, got \
                 {state:?}"
            );
            let elsewhere_started = epoch(&body, "startedAt", case);
            let here_started = *started_on_busy.last().expect("a row has tasks");
            assert!(
                elsewhere_started < here_started - TOLERANCE,
                "case {case:?}: one task's execution on an idle device must start before \
                 its execution on a busy one, got {elsewhere_started} and {here_started}"
            );
        }

        // Every task finished, and the commands reached the device: an unlock anywhere in
        // the queue leaves it unlocked, a reboot leaves the record as it was, and a device
        // that answered a command checked in.
        for (index, task_id) in tasks.iter().enumerate() {
            let described = ok(&describe_task(&router, task_id).await, case);
            let state = string(&described, "state", case);
            assert_eq!(
                state, "COMPLETED",
                "case {case:?}: task {index} is completed once its executions are, got \
                 {state:?}"
            );
        }
        let after = ok(&describe_device(&router, &busy).await, case);
        let expected_device_state = if row.commands.contains(&"unlock") {
            "UNLOCKED"
        } else {
            "LOCKED"
        };
        let device_state = string(&after, "deviceState", case);
        assert_eq!(
            device_state, expected_device_state,
            "case {case:?}: expected the device to be {expected_device_state} once the \
             queue drained, got {device_state:?}"
        );
        let reached_out_after = epoch(&after, "lastReachedOutAt", case);
        assert!(
            reached_out_after > reached_out_before,
            "case {case:?}: a device that answered the commands checked in, so \
             lastReachedOutAt must advance past {reached_out_before}, got {reached_out_after}"
        );
    }
}
