// SPDX-License-Identifier: CC0-1.0

//! What a device's `UnlockState` reports WHILE a command runs on it, end to end over
//! HTTP.
//!
//! The rule under test: a device reports the state the command it is performing has
//! reached, not only the state that command leaves behind — while an unlock the device
//! has taken is in flight `DescribeDevice` reports the transient `UNLOCKING`, and only
//! once the device finished unlocking does it report `UNLOCKED`; a command no value of
//! `UnlockState` reports leaves the member alone throughout.
//!
//! Rows differ along what the rule generalises over: the command, how many devices the
//! task targets, and the state the device was in when the command arrived (locked, and
//! already unlocked by an earlier task). Each observation is checked against the
//! execution state read around it, so the expected device state is COMPUTED from how
//! far the command has got rather than from a sleep landing on one instant.
//!
//! Every case goes through the router, so an answer that is not computed from persisted
//! state cannot pass.

use std::time::Duration;

use serde_json::Value;
use service_com_amazonaws_snowdevicemanagement_snowdevicemanagement::state::{
    EXECUTION_FINISH_SPAN, EXECUTION_START_SPAN,
};
use smithy_server_runtime::MultiServiceRouter;

use super::device_task_lifecycle::{
    describe_device, describe_execution, emulator, ok, queue_task, string, UNLOCK_STATES,
};

/// How often the device and its execution are read while the command runs.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// Slack added to the spans before reading the state the clock owes past them, so a
/// loaded host cannot read a record before the due instant it stores has passed.
const SETTLE_SLACK: Duration = Duration::from_millis(120);

/// One observation of a device and of the execution acting on it.
///
/// The execution is read on BOTH sides of the device read, because each call settles
/// the clock against its own reading of it: a due instant landing between two calls
/// leaves the device a step ahead of the execution read before it. Holding both
/// readings is what lets a case accept either window the device could legitimately be
/// in, instead of racing one read against the other.
#[derive(Debug, Clone)]
struct Observed {
    /// The `ExecutionState` read before the device: how far the command had got.
    execution_before: String,
    /// The `ExecutionState` read after the device: how far it had got by then.
    execution_after: String,
    /// The `UnlockState` the device reports.
    device_state: String,
    /// When the device record was last refreshed.
    last_updated_at: f64,
}

/// Read one device and the execution of `task_id` acting on it.
async fn observe(router: &MultiServiceRouter, task_id: &str, device: &str, case: &str) -> Observed {
    let before = ok(&describe_execution(router, task_id, device).await, case);
    let record = ok(&describe_device(router, device).await, case);
    let after = ok(&describe_execution(router, task_id, device).await, case);
    let device_state = string(&record, "deviceState", case);
    assert!(
        UNLOCK_STATES.contains(&device_state.as_str()),
        "case {case:?}: deviceState {device_state:?} is not one of UnlockState's values \
         {UNLOCK_STATES:?}"
    );
    Observed {
        execution_before: string(&before, "state", case),
        execution_after: string(&after, "state", case),
        device_state,
        last_updated_at: last_updated_at(&record, case),
    }
}

/// When a device record was last refreshed, as the stored record reports it.
fn last_updated_at(record: &Value, case: &str) -> f64 {
    record["lastUpdatedAt"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: expected a lastUpdatedAt, got {record}"))
}

/// The `UnlockState` a device must report for a command that has got as far as
/// `execution_state`, given the state it was in before the command arrived.
///
/// `before` is what a command that no `UnlockState` value reports (a reboot) leaves the
/// member at, in every window.
fn expected_device_state(command: &str, before: &str, execution_state: &str) -> &'static str {
    match (command, execution_state) {
        // "Unlocks the device": in flight the device is unlocking, and once the
        // execution succeeded it is unlocked.
        ("unlock", "IN_PROGRESS") => "UNLOCKING",
        ("unlock", "SUCCEEDED") => "UNLOCKED",
        // Before the device has the command, nothing about it has changed; and a
        // reboot changes no member of the record at all.
        _ => match before {
            "UNLOCKED" => "UNLOCKED",
            "UNLOCKING" => "UNLOCKING",
            _ => "LOCKED",
        },
    }
}

/// Run `command` over the first `target_count` devices and watch each target's record
/// across the whole dispatch window, answering what each device was observed reporting,
/// in order, per device.
async fn watch_command(
    router: &MultiServiceRouter,
    command: &str,
    target_count: usize,
    before: &str,
    case: &str,
) -> Vec<Vec<Observed>> {
    let (task_id, _, targets) = queue_task(router, &[command], target_count, None).await;
    let mut timeline: Vec<Vec<Observed>> = vec![Vec::new(); targets.len()];

    // Poll from the create until past both spans, so every window the command passes
    // through is read: queued, running on the device, and finished.
    let deadline =
        std::time::Instant::now() + EXECUTION_START_SPAN + EXECUTION_FINISH_SPAN + SETTLE_SLACK;
    loop {
        for (index, device) in targets.iter().enumerate() {
            let case = format!("{case}, device {device}");
            timeline[index].push(observe(router, &task_id, device, &case).await);
        }
        if std::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    for (index, device) in targets.iter().enumerate() {
        let case = format!("{case}, device {device}");
        let seen = &timeline[index];
        let states: Vec<&str> = seen
            .iter()
            .map(|observed| observed.execution_after.as_str())
            .collect();
        assert_eq!(
            seen.last()
                .map(|observed| observed.execution_after.as_str()),
            Some("SUCCEEDED"),
            "case {case:?}: past both spans the clock owes the execution SUCCEEDED, \
             observed {states:?}"
        );
        assert!(
            states.contains(&"IN_PROGRESS"),
            "case {case:?}: the command is performed over a span, so some read must find \
             the execution running, observed {states:?}"
        );
        assert!(
            states.contains(&"QUEUED"),
            "case {case:?}: the dispatch span is never zero, so the first read must find \
             the execution queued, observed {states:?}"
        );

        // Every observation: the device reports the state the command has reached. The
        // device read sits between the two execution reads, so what it may report is
        // what either of them accounts for — one expectation when they agree.
        let mut previous: Option<&Observed> = None;
        for observed in seen {
            let expected = [
                expected_device_state(command, before, &observed.execution_before),
                expected_device_state(command, before, &observed.execution_after),
            ];
            assert!(
                expected.contains(&observed.device_state.as_str()),
                "case {case:?}: with the execution {:?} then {:?}, and the device {before} \
                 before the {command}, expected deviceState one of {expected:?}, got {:?}",
                observed.execution_before,
                observed.execution_after,
                observed.device_state
            );
            // A record that reports something different was refreshed when it changed.
            if let Some(previous) = previous.filter(|p| p.device_state != observed.device_state) {
                assert!(
                    observed.last_updated_at > previous.last_updated_at,
                    "case {case:?}: the device moved {:?} -> {:?}, so lastUpdatedAt must \
                     advance past {}, got {}",
                    previous.device_state,
                    observed.device_state,
                    previous.last_updated_at,
                    observed.last_updated_at
                );
            }
            previous = Some(observed);
        }
    }
    timeline
}

#[tokio::test]
async fn a_device_reports_unlocking_while_the_unlock_it_took_is_in_flight() {
    // Rows differ in command and in how many devices the task targets; what each
    // target must report in each window is computed from the execution it is running.
    let rows: &[(&str, usize)] = &[
        ("unlock", 1),
        ("reboot", 1),
        ("unlock", 2),
        ("reboot", 3),
        ("unlock", 3),
    ];

    for (command, target_count) in rows {
        let router = emulator();
        let case = format!("{command} over {target_count} targets from LOCKED");
        let timeline = watch_command(&router, command, *target_count, "LOCKED", &case).await;

        for (index, seen) in timeline.iter().enumerate() {
            let case = format!("{case}, target {index}");
            let states: Vec<&str> = seen
                .iter()
                .map(|observed| observed.device_state.as_str())
                .collect();
            if *command == "unlock" {
                // The whole point: the modelled transient is reachable through the API.
                assert!(
                    states.contains(&"UNLOCKING"),
                    "case {case:?}: an unlock is a transition the caller polls through, so \
                     some read must report UNLOCKING, observed {states:?}"
                );
                assert_eq!(
                    states.last(),
                    Some(&"UNLOCKED"),
                    "case {case:?}: once the device finished unlocking it reports UNLOCKED, \
                     observed {states:?}"
                );
                // It never goes back: LOCKED, then UNLOCKING, then UNLOCKED.
                let rank = |state: &str| match state {
                    "LOCKED" => 0,
                    "UNLOCKING" => 1,
                    _ => 2,
                };
                for pair in states.windows(2) {
                    assert!(
                        rank(pair[0]) <= rank(pair[1]),
                        "case {case:?}: a device unlocking does not go back to {:?} from \
                         {:?}, observed {states:?}",
                        pair[1],
                        pair[0]
                    );
                }
            } else {
                assert!(
                    states.iter().all(|state| *state == "LOCKED"),
                    "case {case:?}: a {command} reports no UnlockState value, so the device \
                     stays LOCKED throughout, observed {states:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn an_unlock_of_an_already_unlocked_device_reports_unlocking_while_it_runs() {
    // The transient reports the command the device is performing, not the difference
    // between two stored values: a device that is already unlocked and is told to
    // unlock again reports UNLOCKING for as long as it is doing it.
    let router = emulator();
    let case = "a second unlock over an already unlocked device";
    let (first_task, _, targets) = queue_task(&router, &["unlock"], 1, None).await;
    let device = targets[0].clone();

    tokio::time::sleep(EXECUTION_START_SPAN + EXECUTION_FINISH_SPAN + SETTLE_SLACK).await;
    let settled = observe(&router, &first_task, &device, case).await;
    assert_eq!(
        settled.execution_after, "SUCCEEDED",
        "case {case:?}: the first unlock must have finished before the second is created"
    );
    assert_eq!(
        settled.device_state, "UNLOCKED",
        "case {case:?}: the device the first unlock finished on reports UNLOCKED"
    );

    let timeline = watch_command(&router, "unlock", 1, "UNLOCKED", case).await;
    let states: Vec<&str> = timeline[0]
        .iter()
        .map(|observed| observed.device_state.as_str())
        .collect();
    assert!(
        states.contains(&"UNLOCKING"),
        "case {case:?}: the device performing the second unlock reports UNLOCKING while it \
         runs, observed {states:?}"
    );
    assert_eq!(
        states.last(),
        Some(&"UNLOCKED"),
        "case {case:?}: and reports UNLOCKED once it finished, observed {states:?}"
    );
}
