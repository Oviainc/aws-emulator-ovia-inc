// SPDX-License-Identifier: CC0-1.0

//! What a task record reports for `completedAt`, end to end over HTTP.
//!
//! The rule under test: a task record carries "when the task was completed" from the
//! instant it reaches a TERMINAL `TaskState` — whichever path took it there — and
//! carries nothing there while it is still in flight, so the member tells a task that
//! stopped holding work from one that has not. The instant it carries is the one the
//! record last moved at ("when the state of the task was last updated"), and a
//! terminal record never changes again, however long a later caller waits.
//!
//! Rows differ along what the rule generalises over: which path terminalised the task
//! (a cancel that stopped the queued work, a cancel that arrived too late to stop
//! anything, and the clock carrying every execution to its outcome), the command, and
//! how many devices the task targets. The expected state and the presence of
//! `completedAt` are COMPUTED from the state each read reports, so no row is fitted to
//! one request.
//!
//! Every case goes through the router, so an answer that is not computed from
//! persisted state cannot pass.

use std::time::Duration;

use serde_json::Value;
use service_com_amazonaws_snowdevicemanagement_snowdevicemanagement::state::{
    EXECUTION_FINISH_SPAN, EXECUTION_START_SPAN,
};
use smithy_server_runtime::MultiServiceRouter;

use super::device_task_lifecycle::{
    cancel_task, describe_task, emulator, ok, queue_task, string,
};

/// Slack added to a span before reading the state the clock owes past it, so a loaded
/// host cannot read the record before the due instant it stores has passed.
const SETTLE_SLACK: Duration = Duration::from_millis(80);

/// `TaskState`'s one value that names work still in flight, and its two terminal
/// values, from `smithy_query(query="shape",shape="TaskState")`.
const IN_FLIGHT_TASK_STATE: &str = "IN_PROGRESS";
const TERMINAL_TASK_STATES: &[&str] = &["CANCELED", "COMPLETED"];

/// How close two epoch-second timestamps count as the same instant.
const SAME_INSTANT: f64 = 1e-6;

/// A required timestamp member of a `DescribeTask` answer, as epoch seconds.
fn instant(body: &Value, member: &str, case: &str) -> f64 {
    body[member]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: expected a {member} timestamp, got {body}"))
}

/// Assert the invariant on ONE `DescribeTask` answer and hand back the state it
/// reports: `completedAt` is present exactly when the state is terminal, and then it
/// is the instant the record last moved at and is not before the task was created.
fn assert_completed_at_tracks_state(body: &Value, case: &str) -> String {
    let state = string(body, "state", case);
    let created_at = instant(body, "createdAt", case);
    let last_updated_at = instant(body, "lastUpdatedAt", case);
    if state == IN_FLIGHT_TASK_STATE {
        assert!(
            body["completedAt"].is_null(),
            "case {case:?}: a task still {IN_FLIGHT_TASK_STATE} has not completed, so it \
             must report no completedAt, got {body}"
        );
    } else {
        assert!(
            TERMINAL_TASK_STATES.contains(&state.as_str()),
            "case {case:?}: state {state:?} is neither {IN_FLIGHT_TASK_STATE} nor one of \
             the terminal values {TERMINAL_TASK_STATES:?}, got {body}"
        );
        let completed_at = instant(body, "completedAt", case);
        assert!(
            (completed_at - last_updated_at).abs() <= SAME_INSTANT,
            "case {case:?}: a task that reached {state} stopped holding work when its \
             record last moved, so completedAt must be {last_updated_at}, got \
             {completed_at}"
        );
        assert!(
            completed_at >= created_at,
            "case {case:?}: completedAt {completed_at} must not precede createdAt \
             {created_at}, got {body}"
        );
    }
    state
}

/// `DescribeTask` for one task, with the invariant checked on the answer.
async fn described(router: &MultiServiceRouter, task_id: &str, case: &str) -> (Value, String) {
    let body = ok(&describe_task(router, task_id).await, case);
    let state = assert_completed_at_tracks_state(&body, case);
    (body, state)
}

/// Which path takes the task to a terminal state.
#[derive(Debug, Clone, Copy)]
enum Path {
    /// A cancel that lands before any command left the queue, so it stops the work and
    /// the task reports `CANCELED`.
    CancelWhileQueued,
    /// A cancel that lands after every device started its command, so it stops nothing
    /// and the clock still carries the task to `COMPLETED`.
    CancelTooLate,
    /// No cancel at all: the clock carries every execution to its outcome.
    Clock,
}

#[tokio::test]
async fn every_terminal_task_reports_the_instant_it_stopped_holding_work() {
    // Rows differ in the path that terminalises the task, the command and the number
    // of targets.
    let rows: &[(Path, &str, usize)] = &[
        (Path::CancelWhileQueued, "unlock", 1),
        (Path::CancelWhileQueued, "reboot", 3),
        (Path::CancelTooLate, "unlock", 1),
        (Path::CancelTooLate, "reboot", 2),
        (Path::Clock, "unlock", 2),
        (Path::Clock, "reboot", 1),
    ];

    for (path, command, target_count) in rows {
        let router = emulator();
        let case = format!("{path:?}: {command} over {target_count} targets");
        let (task_id, _, _) = queue_task(&router, &[command], *target_count, None).await;

        // A task is in flight from the instant it is created, so it reports no
        // completedAt yet.
        let (_, state) = described(&router, &task_id, &case).await;
        assert_eq!(
            state, IN_FLIGHT_TASK_STATE,
            "case {case:?}: a freshly created task is {IN_FLIGHT_TASK_STATE}, got {state:?}"
        );

        match path {
            Path::CancelWhileQueued => {
                ok(&cancel_task(&router, &task_id).await, &case);
            }
            Path::CancelTooLate => {
                // Past the dispatch span every device has its command, so the cancel
                // has nothing queued left to stop.
                tokio::time::sleep(EXECUTION_START_SPAN + SETTLE_SLACK).await;
                ok(&cancel_task(&router, &task_id).await, &case);
                let (_, state) = described(&router, &task_id, &case).await;
                assert_ne!(
                    state, "CANCELED",
                    "case {case:?}: a cancel that stopped nothing must not terminalise \
                     the task as CANCELED, got {state:?}"
                );
            }
            Path::Clock => {}
        }

        // Let the clock carry whatever survived to its outcome, so every row ends on a
        // terminal record.
        tokio::time::sleep(EXECUTION_START_SPAN + EXECUTION_FINISH_SPAN + SETTLE_SLACK).await;
        let (terminal, state) = described(&router, &task_id, &case).await;
        let expected = match path {
            Path::CancelWhileQueued => "CANCELED",
            Path::CancelTooLate | Path::Clock => "COMPLETED",
        };
        assert_eq!(
            state, expected,
            "case {case:?}: expected the task to settle {expected}, got {state:?}"
        );
        assert!(
            !terminal["completedAt"].is_null(),
            "case {case:?}: a task that reached {state} stopped holding work, so it must \
             report a completedAt, got {terminal}"
        );

        // A terminal record is final: reading it again, and waiting another whole
        // execution span, changes nothing a caller sees.
        let (repeat, _) = described(&router, &task_id, &case).await;
        assert_eq!(
            repeat, terminal,
            "case {case:?}: DescribeTask is a report, so two calls must answer \
             identically, got {repeat}"
        );
        tokio::time::sleep(EXECUTION_FINISH_SPAN + SETTLE_SLACK).await;
        let (later, _) = described(&router, &task_id, &case).await;
        assert_eq!(
            later, terminal,
            "case {case:?}: nothing is owed on a terminal task, so its record must not \
             move afterwards, got {later}"
        );
    }
}

#[tokio::test]
async fn completed_at_discriminates_a_stopped_task_from_a_running_one_whichever_path_stopped_it() {
    // Three tasks in ONE store: one stopped by a cancel, one carried to its outcome by
    // the clock, and one still running. The member must not tell the two terminal
    // paths apart, and must tell either of them from the running one.
    let router = emulator();
    let case = "a cancelled, a completed and a running task in one store";

    let (cancelled, _, _) = queue_task(&router, &["unlock"], 1, None).await;
    ok(&cancel_task(&router, &cancelled).await, case);

    let (completed, _, _) = queue_task(&router, &["reboot"], 2, None).await;
    tokio::time::sleep(EXECUTION_START_SPAN + EXECUTION_FINISH_SPAN + SETTLE_SLACK).await;

    let (running, _, _) = queue_task(&router, &["unlock"], 1, None).await;

    let (cancelled_body, cancelled_state) = described(&router, &cancelled, case).await;
    let (completed_body, completed_state) = described(&router, &completed, case).await;
    let (running_body, running_state) = described(&router, &running, case).await;

    assert_eq!(
        cancelled_state, "CANCELED",
        "case {case:?}: the task whose queued work was stopped reports CANCELED, got \
         {cancelled_body}"
    );
    assert_eq!(
        completed_state, "COMPLETED",
        "case {case:?}: the task whose executions all reached an outcome reports \
         COMPLETED, got {completed_body}"
    );
    assert_eq!(
        running_state, IN_FLIGHT_TASK_STATE,
        "case {case:?}: the task just created is still running, got {running_body}"
    );

    for (task_id, body) in [
        (&cancelled, &cancelled_body),
        (&completed, &completed_body),
    ] {
        assert!(
            !body["completedAt"].is_null(),
            "case {case:?}: task {task_id} stopped holding work, so it must report a \
             completedAt like every other terminal task, got {body}"
        );
    }
    assert!(
        running_body["completedAt"].is_null(),
        "case {case:?}: the running task must be the only one without a completedAt, \
         got {running_body}"
    );
}
