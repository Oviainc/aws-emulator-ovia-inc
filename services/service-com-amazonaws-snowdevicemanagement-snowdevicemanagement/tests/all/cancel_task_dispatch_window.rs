// SPDX-License-Identifier: CC0-1.0

//! What a `CancelTask` can and cannot stop, end to end over HTTP.
//!
//! The rule under test: a cancel stops only the work no device has started yet — a
//! per-device execution still `QUEUED` becomes `CANCELED` and the device never performs
//! the command, while an execution already dispatched keeps running, reaches its own
//! outcome and still applies the command to its device; the task itself reports
//! `CANCELED` only when the call had a queued execution to stop.
//!
//! Rows differ along what the rule generalises over: the command, how many devices the
//! task targets, and where in the dispatch window the cancel lands (before the command
//! left the queue, after the device started it, and after it already finished). The
//! expected execution state, task state and device record are COMPUTED from the state
//! each execution was in when the cancel landed, so no row is fitted to one request.
//!
//! Every case goes through the router, so an answer that is not computed from persisted
//! state cannot pass.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_snowdevicemanagement_snowdevicemanagement::state::{
    EXECUTION_FINISH_SPAN, EXECUTION_START_SPAN,
};
use smithy_server_runtime::MultiServiceRouter;

use super::device_task_lifecycle::{
    cancel_task, create_task, describe_device, describe_execution, describe_task, emulator, ids_of,
    ok, queue_task, string, whole_inventory,
};

/// Slack added to a span before reading the state the clock owes past it, so a loaded
/// host cannot read the record before the due instant it stores has passed.
const SETTLE_SLACK: Duration = Duration::from_millis(80);

/// Where in the dispatch window the cancel lands.
#[derive(Debug, Clone, Copy)]
enum When {
    /// Before the command left the queue: the execution is still `QUEUED`.
    StillQueued,
    /// After the device started the command: the execution is `IN_PROGRESS`.
    AlreadyStarted,
    /// After the execution already reached its outcome.
    AlreadyFinished,
}

impl When {
    /// How long to wait after the create before cancelling.
    fn wait(self) -> Duration {
        match self {
            // Deliberately short of the dispatch span: nothing has been handed to a
            // device yet.
            Self::StillQueued => Duration::from_millis(40),
            Self::AlreadyStarted => EXECUTION_START_SPAN + SETTLE_SLACK,
            Self::AlreadyFinished => EXECUTION_START_SPAN + EXECUTION_FINISH_SPAN + SETTLE_SLACK,
        }
    }
}

/// `TaskState`'s values, from `smithy_query(query="shape",shape="TaskState")`.
const TASK_STATES: &[&str] = &["IN_PROGRESS", "CANCELED", "COMPLETED"];

/// The `state` member of one execution, read back through `DescribeExecution`.
async fn execution_state(
    router: &MultiServiceRouter,
    task_id: &str,
    device: &str,
    case: &str,
) -> String {
    let body = ok(&describe_execution(router, task_id, device).await, case);
    string(&body, "state", case)
}

/// The `state` member of one task, read back through `DescribeTask`.
async fn task_state(router: &MultiServiceRouter, task_id: &str, case: &str) -> String {
    let body = ok(&describe_task(router, task_id).await, case);
    let state = string(&body, "state", case);
    assert!(
        TASK_STATES.contains(&state.as_str()),
        "case {case:?}: state {state:?} is not one of TaskState's values {TASK_STATES:?}"
    );
    state
}

/// One device's record, read back through `DescribeDevice`.
async fn device_record(router: &MultiServiceRouter, device: &str, case: &str) -> Value {
    ok(&describe_device(router, device).await, case)
}

/// The instant a device last checked in, as its stored record reports it.
fn last_reached_out_at(record: &Value, case: &str) -> f64 {
    record["lastReachedOutAt"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: expected a lastReachedOutAt, got {record}"))
}

#[tokio::test]
async fn cancel_task_stops_only_the_executions_no_device_has_started() {
    // Rows differ in command, in how many devices the task targets, and in where in
    // the dispatch window the cancel lands.
    let rows: &[(&str, usize, When)] = &[
        ("unlock", 1, When::StillQueued),
        ("reboot", 2, When::StillQueued),
        ("unlock", 1, When::AlreadyStarted),
        ("reboot", 3, When::AlreadyStarted),
        ("unlock", 2, When::AlreadyFinished),
    ];

    for (command, target_count, when) in rows {
        let router = emulator();
        let case = format!("{command} over {target_count} targets, cancel {when:?}");
        let (task_id, _, targets) = queue_task(&router, &[command], *target_count, None).await;

        // What each target device reported before anything was dispatched to it.
        let mut before_device = Vec::new();
        for device in &targets {
            before_device.push(device_record(&router, device, &case).await);
        }

        tokio::time::sleep(when.wait()).await;

        // The state each execution is in when the cancel lands is what decides
        // whether the cancel may stop it.
        let mut at_cancel = Vec::new();
        for device in &targets {
            let case = format!("{case}, device {device}");
            at_cancel.push(execution_state(&router, &task_id, device, &case).await);
        }
        for (device, state) in targets.iter().zip(&at_cancel) {
            let expected: &[&str] = match when {
                When::StillQueued => &["QUEUED"],
                // Past the dispatch span the device has the command; past the finish
                // span it has answered.
                When::AlreadyStarted => &["IN_PROGRESS", "SUCCEEDED"],
                When::AlreadyFinished => &["SUCCEEDED"],
            };
            assert!(
                expected.contains(&state.as_str()),
                "case {case:?}, device {device}: the clock owes this execution one of \
                 {expected:?} by the cancel, got {state:?}"
            );
        }
        let state_before_cancel = task_state(&router, &task_id, &case).await;

        let cancelled = ok(&cancel_task(&router, &task_id).await, &case);
        assert_eq!(
            cancelled["taskId"].as_str(),
            Some(task_id.as_str()),
            "case {case:?}: CancelTask reports the task it was asked about, got {cancelled}"
        );

        // Only a QUEUED execution may have changed, and only to CANCELED: an
        // execution the device already has keeps running to its own outcome.
        for (device, state_at_cancel) in targets.iter().zip(&at_cancel) {
            let case = format!("{case}, device {device}");
            let after = execution_state(&router, &task_id, device, &case).await;
            let expected: &[&str] = match state_at_cancel.as_str() {
                "QUEUED" => &["CANCELED"],
                "IN_PROGRESS" => &["IN_PROGRESS", "SUCCEEDED"],
                _ => &["SUCCEEDED"],
            };
            assert!(
                expected.contains(&after.as_str()),
                "case {case:?}: an execution {state_at_cancel:?} at the cancel must read \
                 one of {expected:?} after it, got {after:?}"
            );
        }

        // The task reports CANCELED only when the cancel had queued work to stop.
        let cancellable = at_cancel.iter().any(|state| state == "QUEUED");
        let after_cancel = task_state(&router, &task_id, &case).await;
        if cancellable {
            assert_eq!(
                after_cancel, "CANCELED",
                "case {case:?}: a cancel that stopped queued work must leave the task \
                 CANCELED, got {after_cancel:?}"
            );
        } else {
            assert_ne!(
                after_cancel, "CANCELED",
                "case {case:?}: a cancel that found nothing left to stop (task was \
                 {state_before_cancel:?}) must not report the task CANCELED, got \
                 {after_cancel:?}"
            );
        }

        // Let the clock carry whatever survived the cancel to its outcome.
        tokio::time::sleep(EXECUTION_START_SPAN + EXECUTION_FINISH_SPAN + SETTLE_SLACK).await;

        for (index, device) in targets.iter().enumerate() {
            let case = format!("{case}, device {device}");
            let cancelled_here = at_cancel[index] == "QUEUED";
            let final_state = execution_state(&router, &task_id, device, &case).await;
            let expected = if cancelled_here {
                "CANCELED"
            } else {
                "SUCCEEDED"
            };
            assert_eq!(
                final_state, expected,
                "case {case:?}: an execution the cancel {} must end {expected}, got \
                 {final_state:?}",
                if cancelled_here {
                    "stopped"
                } else {
                    "arrived too late for"
                }
            );

            // What the device did follows the execution: a cancelled command never
            // reaches it, and one it had already started is applied.
            let record = device_record(&router, device, &case).await;
            let reached_out = last_reached_out_at(&record, &case);
            let reached_out_before = last_reached_out_at(&before_device[index], &case);
            let device_state = string(&record, "deviceState", &case);
            if cancelled_here {
                assert_eq!(
                    reached_out, reached_out_before,
                    "case {case:?}: a cancelled command never reaches the device, so its \
                     check-in instant must not move, got {reached_out}"
                );
                assert_eq!(
                    device_state, "LOCKED",
                    "case {case:?}: a cancelled command must leave the device record as it \
                     was, got {device_state:?}"
                );
            } else {
                assert!(
                    reached_out > reached_out_before,
                    "case {case:?}: a device that answered the command checked in, so its \
                     check-in instant must move past {reached_out_before}, got {reached_out}"
                );
                let expected_device_state = if *command == "unlock" {
                    "UNLOCKED"
                } else {
                    "LOCKED"
                };
                assert_eq!(
                    device_state, expected_device_state,
                    "case {case:?}: a {command} the device performed must leave it \
                     {expected_device_state}, got {device_state:?}"
                );
            }
        }

        // A task whose executions all reached an outcome without being cancelled is
        // COMPLETED; one the cancel stopped stays CANCELED.
        let settled = task_state(&router, &task_id, &case).await;
        let expected_task_state = if cancellable { "CANCELED" } else { "COMPLETED" };
        assert_eq!(
            settled, expected_task_state,
            "case {case:?}: expected the task to settle {expected_task_state}, got \
             {settled:?}"
        );
    }
}

#[tokio::test]
async fn a_cancel_stops_the_queued_task_of_a_device_and_not_the_one_it_is_running() {
    // Two tasks over the SAME device, created far enough apart that the first is
    // running when the second is still queued, so the selection cannot be per call:
    // one cancel each, and only the queued one stops.
    let router = emulator();
    let case = "one device, a running task and a queued one";
    let device = ids_of(&whole_inventory(&router, case).await, case)[0].clone();

    let running = ok(
        &create_task(
            &router,
            json!({ "targets": [device.clone()], "command": { "unlock": {} } }),
        )
        .await,
        case,
    );
    let running_task = string(&running, "taskId", case);
    let before = device_record(&router, &device, case).await;

    tokio::time::sleep(EXECUTION_START_SPAN + SETTLE_SLACK).await;

    let queued = ok(
        &create_task(
            &router,
            json!({ "targets": [device.clone()], "command": { "reboot": {} } }),
        )
        .await,
        case,
    );
    let queued_task = string(&queued, "taskId", case);

    assert_eq!(
        execution_state(&router, &running_task, &device, case).await,
        "IN_PROGRESS",
        "case {case:?}: past the dispatch span the first task's execution is running"
    );
    assert_eq!(
        execution_state(&router, &queued_task, &device, case).await,
        "QUEUED",
        "case {case:?}: the second task's execution has not been dispatched yet"
    );

    ok(&cancel_task(&router, &running_task).await, case);
    ok(&cancel_task(&router, &queued_task).await, case);

    assert_ne!(
        execution_state(&router, &running_task, &device, case).await,
        "CANCELED",
        "case {case:?}: the running execution must survive its cancel"
    );
    assert_eq!(
        execution_state(&router, &queued_task, &device, case).await,
        "CANCELED",
        "case {case:?}: the queued execution must be stopped by its cancel"
    );

    // The command the device had already begun still lands on it.
    tokio::time::sleep(EXECUTION_FINISH_SPAN + SETTLE_SLACK).await;
    assert_eq!(
        execution_state(&router, &running_task, &device, case).await,
        "SUCCEEDED",
        "case {case:?}: the running execution must reach its own outcome"
    );
    let after = device_record(&router, &device, case).await;
    assert_eq!(
        string(&after, "deviceState", case),
        "UNLOCKED",
        "case {case:?}: the unlock the device had begun must be applied, got {after}"
    );
    assert!(
        last_reached_out_at(&after, case) > last_reached_out_at(&before, case),
        "case {case:?}: the device that answered the command checked in, got {after}"
    );
    assert_eq!(
        task_state(&router, &queued_task, case).await,
        "CANCELED",
        "case {case:?}: the task whose only execution was stopped reports CANCELED"
    );
    assert_eq!(
        task_state(&router, &running_task, case).await,
        "COMPLETED",
        "case {case:?}: the task whose execution ran to its outcome reports COMPLETED"
    );
}
