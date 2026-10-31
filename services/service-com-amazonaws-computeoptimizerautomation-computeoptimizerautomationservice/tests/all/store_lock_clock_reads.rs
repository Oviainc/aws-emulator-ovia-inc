// SPDX-License-Identifier: CC0-1.0

//! The instant a handler judges and stamps this service's store with is read
//! while the handler HOLDS the store lock.
//!
//! The rule under test: the one instant an operation decides a stored
//! resource's clock-owed status on, and stamps what it writes with, is read
//! after it has taken the store lock — so the status it judges is the status the
//! store holds when it looks, and what it stores is stamped with the time the
//! store took it. An instant read before the lock belongs to an earlier moment
//! and says nothing about the store the handler then reads and writes, so under
//! contention it makes two handlers disagree about one resource at one instant,
//! and stores a transition that is already overdue as if it had just begun.
//!
//! Every case drives the service the way a client does, through the harness
//! `automation_rule_lifecycle` owns, while a background thread HOLDS the store
//! lock across the window in which the clock's transition falls due. The only
//! thing reached into the store directly is an automation event mid-run, which
//! is what gives the clock something to owe; everything asserted afterwards is
//! read back over the wire.

use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::state::{
    self, PendingTransition, StoredEvent,
};
use smithy_server_runtime::MultiServiceRouter;

use super::automation_rule_lifecycle::{
    assert_refused, call, call_ok, emulator_with_engine, member, seed_event, string_member,
};

/// How long the test waits between recording the release instant and actually
/// dropping the store guard. It gives every timestamp a handler stamps under the
/// lock a margin over the recorded instant that is far larger than the
/// resolution of a serialized epoch-second double, so the assertions below are
/// about which side of the lock the clock was read on and never about rounding.
const RELEASE_MARGIN: Duration = Duration::from_millis(20);

/// One automation event mid-run: it is running, and the completion the clock
/// owes it falls due `due_in` from now. `eventId` is 16 characters of
/// `[0-9A-Za-z]` (the model's `EventId` pattern) and `resourceType` carries the
/// single value the model's `ResourceType` enum declares. Its steps are built
/// where `StartAutomationEvent` builds them, so the forward run this rollback
/// appends to is the one a real run would have stored.
fn running_event(index: usize, due_in: Duration) -> StoredEvent {
    let event_id = format!("clk{index:013}");
    let started = SystemTime::now();
    let resource_id = format!("vol-{index:08}");
    StoredEvent {
        event_id: event_id.clone(),
        event_description: Some(format!("event {index} mid-run")),
        event_type: "UpgradeEbsVolumeType".to_string(),
        event_status: state::EVENT_STATUS_IN_PROGRESS.to_string(),
        event_status_reason: None,
        resource_arn: None,
        resource_id: Some(resource_id.clone()),
        recommended_action_id: None,
        account_id: engine::EMULATOR_ACCOUNT_ID.to_string(),
        region: Some("us-east-1".to_string()),
        rule_id: None,
        resource_type: "EbsVolume".to_string(),
        created_timestamp: DateTime::from(started),
        completed_timestamp: None,
        estimated_monthly_savings: None,
        steps: state::steps_for_started_event(
            &event_id,
            "UpgradeEbsVolumeType",
            Some(&resource_id),
            None,
            DateTime::from(started),
        ),
        pending: Some(PendingTransition {
            due: started + due_in,
            terminal_status: state::EVENT_STATUS_COMPLETE.to_string(),
            first_step: 0,
        }),
    }
}

/// Take the store lock in a background thread and hold it for `hold`.
///
/// The returned receiver fires once the lock is HELD, so the caller can issue a
/// request that is then granted the lock only after `hold` has passed — which is
/// the contention every case here is about. The join handle reports the instant
/// recorded [`RELEASE_MARGIN`] before the guard was released, so any timestamp
/// read under the lock is strictly later than it.
///
/// `pub(crate)` because [`crate::event_window_floor`] puts the same contention
/// behind its own cases rather than growing a copy of this.
pub(crate) fn hold_store_lock(
    engine: &Arc<engine::InMemoryEngine>,
    hold: Duration,
) -> (mpsc::Receiver<()>, std::thread::JoinHandle<SystemTime>) {
    let handle = state::service_state(&**engine).expect("the service state is reachable");
    let (held, now_held) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        let guard = handle.lock();
        held.send(()).expect("the test awaits the held lock");
        std::thread::sleep(hold);
        let released = SystemTime::now();
        std::thread::sleep(RELEASE_MARGIN);
        drop(guard);
        released
    });
    (now_held, thread)
}

/// Epoch seconds, the form an awsJson1_0 timestamp arrives in.
fn epoch_seconds(at: SystemTime) -> f64 {
    at.duration_since(UNIX_EPOCH)
        .expect("an instant after the epoch")
        .as_secs_f64()
}

/// `GetAutomationEvent` for `event_id`, requiring success.
async fn get_event(router: &MultiServiceRouter, event_id: &str, case: &str) -> Value {
    call_ok(
        router,
        "GetAutomationEvent",
        &json!({"eventId": event_id}),
        case,
    )
    .await
}

/// The steps `ListAutomationEventSteps` reports for `event_id`, in page order.
async fn listed_steps(router: &MultiServiceRouter, event_id: &str, case: &str) -> Vec<Value> {
    let listed = call_ok(
        router,
        "ListAutomationEventSteps",
        &json!({"eventId": event_id}),
        case,
    )
    .await;
    member(&listed, "automationEventSteps")
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// One row of the rollback-under-contention table: when the automation's
/// completion falls due, how long the store lock is held while the rollback
/// waits for it, and whether the rollback may start once it is granted.
struct RollbackCase {
    case: &'static str,
    due_in: Duration,
    hold: Duration,
    rolls_back: bool,
}

/// A rollback is judged on the status the event has when the handler reaches the
/// store, and stamps what it writes with the time the store took it.
///
/// The table varies the two things the pre-lock-versus-post-lock instant can
/// change: whether the completion the clock owes the event falls due during the
/// wait for the lock (rows 1 and 2), or had already fallen due before the call
/// (row 3, where either instant decides the same way), or falls due after the
/// wait is over (row 4, where the handler must NOT settle an event the clock
/// does not owe a transition yet). In every row where the rollback starts, the
/// rollback it stores is stamped from the instant the store took it and its own
/// completion is therefore still ahead: the readers report it as running.
#[tokio::test]
async fn a_rollback_is_judged_and_stamped_from_the_store_time_clock() {
    let delay = state::EVENT_SETTLEMENT_DELAY;
    let cases = [
        RollbackCase {
            case: "due-passes-while-the-lock-is-held",
            due_in: delay / 2,
            hold: delay * 2,
            rolls_back: true,
        },
        RollbackCase {
            case: "due-passes-just-before-the-release",
            due_in: delay,
            hold: delay + delay / 2,
            rolls_back: true,
        },
        RollbackCase {
            case: "already-due-before-the-call",
            due_in: Duration::ZERO,
            hold: delay * 2,
            rolls_back: true,
        },
        RollbackCase {
            case: "not-due-even-after-the-release",
            // Far enough ahead that the whole row runs well inside it: the row
            // is about a completion the clock does not owe yet, not about how
            // long the row itself takes.
            due_in: delay * 120,
            hold: delay / 2,
            rolls_back: false,
        },
    ];

    for (index, row) in cases.iter().enumerate() {
        let case = row.case;
        // One store per row: a row's contention and its client tokens are its
        // own.
        let (router, engine) = emulator_with_engine();
        let event = running_event(index, row.due_in);
        let forward_steps = event.steps.len();
        seed_event(&engine, &event);

        let (held, holder) = hold_store_lock(&engine, row.hold);
        held.recv().expect("the holder takes the store lock");
        let answer = call(
            &router,
            "RollbackAutomationEvent",
            &json!({"eventId": event.event_id}),
        )
        .await;
        let released = epoch_seconds(holder.join().expect("the holder thread finishes"));

        if !row.rolls_back {
            assert_refused(answer, 400, "InvalidParameterValueException", case);
            // The clock owed this event nothing yet, so the refusal is on the
            // status it really has — and it is left exactly there.
            let unchanged = get_event(&router, &event.event_id, case).await;
            assert_eq!(
                string_member(&unchanged, "eventStatus"),
                state::EVENT_STATUS_IN_PROGRESS,
                "case {case:?}: an automation the clock has not completed yet stays running, \
                 got {unchanged}"
            );
            assert_eq!(
                listed_steps(&router, &event.event_id, case).await.len(),
                forward_steps,
                "case {case:?}: a refused rollback appends no step"
            );
            continue;
        }

        let (status, body) = answer;
        assert_eq!(
            status, 200,
            "case {case:?}: the automation's completion fell due before the handler reached the \
             store, so its rollback starts, got {body}"
        );
        assert_eq!(
            string_member(&body, "eventStatus"),
            state::EVENT_STATUS_ROLLBACK_IN_PROGRESS,
            "case {case:?}: a just-initiated rollback is still running, got {body}"
        );

        // The rollback was stored with its own completion ahead of it, not
        // already overdue: the reader agrees it is still running.
        let running = get_event(&router, &event.event_id, case).await;
        assert_eq!(
            string_member(&running, "eventStatus"),
            state::EVENT_STATUS_ROLLBACK_IN_PROGRESS,
            "case {case:?}: the rollback the call reported as running has not already settled, \
             got {running}"
        );

        // The step the rollback is on was stamped when the store took it, so it
        // did not start before the store recorded it.
        let steps = listed_steps(&router, &event.event_id, case).await;
        assert!(
            steps.len() > forward_steps,
            "case {case:?}: the rollback appends its own steps to the forward run, got {steps:?}"
        );
        let first_rollback_step = &steps[forward_steps];
        assert_eq!(
            string_member(first_rollback_step, "stepStatus"),
            state::STEP_STATUS_IN_PROGRESS,
            "case {case:?}: the rollback is on its first step, got {first_rollback_step}"
        );
        let started = member(first_rollback_step, "startTimestamp").as_f64();
        let Some(started) = started else {
            panic!("case {case:?}: a running step carries its start instant, got {steps:?}");
        };
        assert!(
            started >= released,
            "case {case:?}: expected a start instant at or after the store was reachable again \
             ({released}), got {started}"
        );

        // The clock still owes this rollback its completion, and settles it.
        tokio::time::sleep(state::EVENT_SETTLEMENT_DELAY + Duration::from_millis(250)).await;
        let settled = get_event(&router, &event.event_id, case).await;
        assert_eq!(
            string_member(&settled, "eventStatus"),
            state::EVENT_STATUS_ROLLBACK_COMPLETE,
            "case {case:?}: the rollback the handler stored completes once its window passes, \
             got {settled}"
        );
    }
}

/// One row of the reader table: an operation that reports an automation event,
/// and how to read the status it reports for one.
struct ReaderCase {
    case: &'static str,
    operation: &'static str,
    request: fn(&str) -> Value,
    reported_status: fn(&Value, &str) -> String,
}

/// The status `GetAutomationEvent` reports.
fn status_from_get(body: &Value, _event_id: &str) -> String {
    string_member(body, "eventStatus")
}

/// The status `ListAutomationEvents` reports for the one event in the store.
fn status_from_list(body: &Value, event_id: &str) -> String {
    member(body, "automationEvents")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .find(|event| string_member(event, "eventId") == event_id)
        .map(|event| string_member(event, "eventStatus"))
        .unwrap_or_default()
}

/// The status `ListAutomationEventSummaries` groups the one event in the store
/// under.
fn status_from_summaries(body: &Value, _event_id: &str) -> String {
    member(body, "automationEventSummaries")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .find(|summary| {
            member(summary, "total")
                .get("automationEventCount")
                .and_then(Value::as_i64)
                .unwrap_or_default()
                > 0
        })
        .map(|summary| string_member(summary, "key"))
        .unwrap_or_default()
}

/// The status `ListAutomationEventSteps` implies: the run is over exactly when
/// every step of it is terminal.
fn status_from_steps(body: &Value, _event_id: &str) -> String {
    let steps = member(body, "automationEventSteps")
        .as_array()
        .cloned()
        .unwrap_or_default();
    if !steps.is_empty()
        && steps
            .iter()
            .all(|step| string_member(step, "stepStatus") == state::STEP_STATUS_COMPLETE)
    {
        state::EVENT_STATUS_COMPLETE.to_string()
    } else {
        state::EVENT_STATUS_IN_PROGRESS.to_string()
    }
}

/// Every operation that reports an automation event judges its clock-owed status
/// on an instant read under the store lock, so they agree about one event at one
/// instant however long any of them waited for the store.
///
/// The table is the event-reporting surface of this service: the event reader,
/// the event listing, the status summary it groups by and the step listing. Each
/// row is served a store whose lock was held across the instant the event's
/// completion fell due, so a clock read taken before the lock would report it as
/// still running while the others report it complete.
#[tokio::test]
async fn every_event_reader_reports_the_status_the_store_holds_when_it_looks() {
    let delay = state::EVENT_SETTLEMENT_DELAY;
    let readers = [
        ReaderCase {
            case: "get-automation-event",
            operation: "GetAutomationEvent",
            request: |event_id| json!({"eventId": event_id}),
            reported_status: status_from_get,
        },
        ReaderCase {
            case: "list-automation-events",
            operation: "ListAutomationEvents",
            request: |_| json!({}),
            reported_status: status_from_list,
        },
        ReaderCase {
            case: "list-automation-event-summaries",
            operation: "ListAutomationEventSummaries",
            request: |_| json!({}),
            reported_status: status_from_summaries,
        },
        ReaderCase {
            case: "list-automation-event-steps",
            operation: "ListAutomationEventSteps",
            request: |event_id| json!({"eventId": event_id}),
            reported_status: status_from_steps,
        },
    ];

    for (index, row) in readers.iter().enumerate() {
        let case = row.case;
        let (router, engine) = emulator_with_engine();
        let event = running_event(100 + index, delay / 2);
        seed_event(&engine, &event);

        let (held, holder) = hold_store_lock(&engine, delay * 2);
        held.recv().expect("the holder takes the store lock");
        let body = call_ok(&router, row.operation, &(row.request)(&event.event_id), case).await;
        holder.join().expect("the holder thread finishes");

        assert_eq!(
            (row.reported_status)(&body, &event.event_id),
            state::EVENT_STATUS_COMPLETE,
            "case {case:?}: the automation's run window passed before {} reached the store, so \
             it reports the completed automation, got {body}",
            row.operation
        );
    }
}
