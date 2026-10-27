// SPDX-License-Identifier: CC0-1.0

//! The one "this task still holds work" property, driven through both the paths that
//! ask it.
//!
//! The rule under test: whether a task still holds work is ONE property of its stored
//! record — its `TaskState` carries the single non-terminal value — and every path that
//! needs it reads that one property. The quota path counts the records that hold work
//! (what `CreateTask` judges against its bound), and `CancelTask` stops queued work only
//! on a task that still holds some. So the count the quota path reports always equals
//! the number of tasks the task operations report `IN_PROGRESS`, and a cancel aimed at a
//! task that holds no work leaves its record exactly as it was.
//!
//! Rows span how a record came to hold work or stop holding it: executions still queued,
//! an execution the clock has dispatched, every execution carried to its outcome, and
//! queued work an earlier cancel already stopped — over one, two and three target
//! devices and both members of the modelled `Command` union. Every call goes through the
//! router, so a state a handler did not persist cannot pass.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::json;
use service_com_amazonaws_snowdevicemanagement_snowdevicemanagement::state::{
    service_state, EXECUTION_FINISH_SPAN, EXECUTION_START_SPAN, IN_FLIGHT_TASK_QUOTA,
};
use service_com_amazonaws_snowdevicemanagement_snowdevicemanagement::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use super::device_task_lifecycle::{
    call, cancel_task, create_task, describe_task, executions_of, ids_of, list_executions, ok,
    queue_task, string, whole_inventory, SIGNED_REGION,
};

/// How many devices this suite activates Snow Device Management on: enough for a row to
/// target three of them.
const DEVICES: u64 = 3;

/// Slack added to a span before reading the state the clock owes past it, so a loaded
/// host cannot read a record before the due instant it stores has passed.
const SETTLE_SLACK: Duration = Duration::from_millis(80);

/// `TaskState`'s values, from `smithy_query(query="shape",shape="TaskState")`.
const TASK_STATES: &[&str] = &["IN_PROGRESS", "CANCELED", "COMPLETED"];

/// The page every list call in this suite asks for, which is the largest the model's
/// `@range(min: 1, max: 100)` on `maxResults` allows: one page holds everything a row
/// creates, so a sample is one call and one settle.
const PAGE: usize = 100;

/// One emulator: a router carrying only this service over one shared engine, with Snow
/// Device Management activated on [`DEVICES`] devices in the Region every call here
/// signs for.
///
/// `device_task_lifecycle::emulator` builds the same router through the same activation
/// seam but keeps the engine to itself; this suite checks what the operations report
/// against the quota path's count, which is state rather than wire, so it needs the
/// handle.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    service_state(&*engine)
        .expect("the in-memory engine carries this service's state")
        .lock()
        .activate_devices(SIGNED_REGION, DEVICES, SystemTime::now());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// What `CreateTask` judges against its two bounds: the records holding work in the
/// Region a request is signed for, and in the whole store.
///
/// Reading them takes no lock the operations do not take and settles nothing, so the
/// counts are over exactly the records the sample's preceding call left behind.
fn in_flight_counts(engine: &Arc<engine::InMemoryEngine>) -> (usize, usize) {
    let state = service_state(&**engine).expect("the in-memory engine carries this service's state");
    let store = state.lock();
    (
        store.tasks_in_flight_in(SIGNED_REGION),
        store.tasks_in_flight(),
    )
}

/// Every task `ListTasks` reports, as `(taskId, state)` pairs.
async fn reported_states(router: &MultiServiceRouter, case: &str) -> Vec<(String, String)> {
    let body = ok(
        &call(router, "GET", &format!("/tasks?maxResults={PAGE}"), None).await,
        case,
    );
    assert!(
        body["nextToken"].is_null(),
        "case {case:?}: a page of {PAGE} holds every task a row creates, got {body}"
    );
    let tasks = body["tasks"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a tasks array, got {body}"))
        .clone();
    tasks
        .iter()
        .map(|task| {
            let state = string(task, "state", case);
            assert!(
                TASK_STATES.contains(&state.as_str()),
                "case {case:?}: state {state:?} is not one of TaskState's values {TASK_STATES:?}"
            );
            (string(task, "taskId", case), state)
        })
        .collect()
}

/// Sample the two paths at once and assert they agree: the quota path's count of
/// records holding work is the number of tasks the task operations report
/// `IN_PROGRESS`, in the signed Region and in the whole store alike (this suite signs
/// for one Region, so the two counts are over the same records).
///
/// Answers what `ListTasks` reported, so a caller can key its own expectations on the
/// states of the very sample that was checked.
async fn assert_counts_agree(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
) -> Vec<(String, String)> {
    let reported = reported_states(router, case).await;
    let (in_region, in_store) = in_flight_counts(engine);
    let holding_work = reported
        .iter()
        .filter(|(_, state)| state == "IN_PROGRESS")
        .count();
    assert_eq!(
        in_region, holding_work,
        "case {case:?}: expected the quota path to count the {holding_work} task(s) \
         reported IN_PROGRESS, got {in_region} for {reported:?}"
    );
    assert_eq!(
        in_store, holding_work,
        "case {case:?}: expected the store-wide count to be the same {holding_work} \
         record(s), got {in_store} for {reported:?}"
    );
    reported
}

/// The state `ListTasks` reported for one task in a sample.
fn state_of(reported: &[(String, String)], task_id: &str, case: &str) -> String {
    reported
        .iter()
        .find(|(id, _)| id == task_id)
        .map(|(_, state)| state.clone())
        .unwrap_or_else(|| panic!("case {case:?}: expected {task_id} among {reported:?}"))
}

/// Every execution state of one task, as `ListExecutions` reports them.
async fn execution_states(router: &MultiServiceRouter, task_id: &str, case: &str) -> Vec<String> {
    let body = ok(
        &list_executions(router, &format!("?taskId={task_id}&maxResults={PAGE}")).await,
        case,
    );
    executions_of(&body, case)
        .iter()
        .map(|execution| string(execution, "state", case))
        .collect()
}

/// Where in its life the row's task is when the cancel under test lands.
#[derive(Debug, Clone, Copy)]
enum Phase {
    /// Nothing has been dispatched yet: every execution is still queued.
    StillQueued,
    /// The clock has handed the command to the devices.
    Dispatched,
    /// The clock has carried every execution to its outcome.
    Finished,
    /// An earlier cancel already stopped the queued work.
    AlreadyCancelled,
}

impl Phase {
    /// How long to wait after the create before the row's own calls.
    fn wait(self) -> Duration {
        match self {
            // Deliberately short of the dispatch span: nothing has left the queue.
            Self::StillQueued | Self::AlreadyCancelled => Duration::from_millis(40),
            Self::Dispatched => EXECUTION_START_SPAN + SETTLE_SLACK,
            Self::Finished => EXECUTION_START_SPAN + EXECUTION_FINISH_SPAN + SETTLE_SLACK,
        }
    }
}

#[tokio::test]
async fn the_quota_count_and_a_cancel_read_the_same_in_flight_record() {
    // Rows differ in the command, in how many devices the task targets and in how far
    // the record has got when the cancel lands.
    let rows: &[(&str, usize, Phase)] = &[
        ("unlock", 1, Phase::StillQueued),
        ("reboot", 2, Phase::StillQueued),
        ("unlock", 1, Phase::Dispatched),
        ("reboot", 3, Phase::Dispatched),
        ("unlock", 2, Phase::Finished),
        ("reboot", 1, Phase::AlreadyCancelled),
        ("unlock", 3, Phase::AlreadyCancelled),
    ];

    for (command, target_count, phase) in rows {
        let (router, engine) = emulator();
        let case = format!("{command} over {target_count} targets, cancelled {phase:?}");
        let (task_id, _, _) = queue_task(&router, &[command], *target_count, None).await;

        tokio::time::sleep(phase.wait()).await;
        if matches!(phase, Phase::AlreadyCancelled) {
            ok(&cancel_task(&router, &task_id).await, &case);
        }

        // A second task, created a moment before the sample, so the count under test is
        // never just the row's own task.
        let device = ids_of(&whole_inventory(&router, &case).await, &case)[0].clone();
        let companion = string(
            &ok(
                &create_task(
                    &router,
                    json!({ "targets": [device], "command": { "reboot": {} } }),
                )
                .await,
                &case,
            ),
            "taskId",
            &case,
        );

        let reported = assert_counts_agree(&router, &engine, &case).await;
        let before = state_of(&reported, &task_id, &case);
        assert_eq!(
            reported.len(),
            2,
            "case {case:?}: the row creates exactly two tasks, got {reported:?}"
        );

        // A record that holds no work is not touched by a cancel: its whole
        // `DescribeTask` projection — the state it reached, the instant it reached it
        // and the instant it was last updated — reads the same afterwards.
        let terminal = before != "IN_PROGRESS";
        let described_before = ok(&describe_task(&router, &task_id).await, &case);
        ok(&cancel_task(&router, &task_id).await, &case);
        let described_after = ok(&describe_task(&router, &task_id).await, &case);
        if terminal {
            assert_eq!(
                described_after, described_before,
                "case {case:?}: a task reported {before:?} holds no work, so the cancel \
                 must leave its record as it was, got {described_after}"
            );
        }

        // Whichever way the row got here, the task reports CANCELED exactly when a
        // cancel stopped one of its executions — nothing else produces that execution
        // state — so the two paths never disagree about what the record holds.
        let after = string(&described_after, "state", &case);
        let executions = execution_states(&router, &task_id, &case).await;
        let stopped = executions.iter().any(|state| state == "CANCELED");
        assert_eq!(
            after == "CANCELED",
            stopped,
            "case {case:?}: a task reads CANCELED exactly when a cancel stopped one of \
             its executions; got state {after:?} with executions {executions:?}"
        );
        assert_eq!(
            executions.len(),
            *target_count,
            "case {case:?}: the task keeps one execution per target, got {executions:?}"
        );

        // And the quota path still counts exactly the records the operations report as
        // holding work, now that the cancel has changed some of them.
        let settled = assert_counts_agree(&router, &engine, &case).await;
        assert_eq!(
            state_of(&settled, &task_id, &case),
            after,
            "case {case:?}: DescribeTask and ListTasks report one record"
        );

        // The count is what `CreateTask` judges against its bound: a Region holding
        // fewer records than the bound allows has room, so the create is served, and
        // the record it persists is then counted by the same property as every other.
        let (in_region, _) = in_flight_counts(&engine);
        assert!(
            in_region < IN_FLIGHT_TASK_QUOTA,
            "case {case:?}: a row creates far fewer records than the bound of \
             {IN_FLIGHT_TASK_QUOTA}, so the create must have room, got {in_region}"
        );
        let fresh = string(
            &ok(
                &create_task(
                    &router,
                    json!({ "targets": [&device], "command": { "unlock": {} } }),
                )
                .await,
                &case,
            ),
            "taskId",
            &case,
        );
        let last = assert_counts_agree(&router, &engine, &case).await;
        let ids: Vec<&str> = last.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(
            ids.len(),
            3,
            "case {case:?}: every task the row created keeps its record, got {last:?}"
        );
        for expected in [&task_id, &companion, &fresh] {
            assert!(
                ids.contains(&expected.as_str()),
                "case {case:?}: expected {expected} among the reported tasks, got {last:?}"
            );
        }
    }
}
