// SPDX-License-Identifier: CC0-1.0

//! The retrieval-window floor an automation event is judged against comes from
//! the instant the handler reads while it HOLDS the store lock.
//!
//! The rule under test: one request answers from ONE instant, read after the
//! handler has taken the store lock — the retrieval-window floor that decides
//! which events are reportable at all and the clock-owed status transitions the
//! request materialises both come from that single read. A floor derived from an
//! instant read before the lock belongs to an earlier moment, so an event that
//! fell out of the window while the request waited for the store would be
//! reported by the waiting reader and absent from every reader that did not
//! wait, and the status that reader reports for it would have been settled
//! against a different instant than its window membership was judged on.
//!
//! Every case drives the service the way a client does, through the harness
//! `automation_rule_lifecycle` owns, while a background thread HOLDS the store
//! lock across the instant the event's `createdTimestamp` falls out of the
//! retrieval window (the contention helper `store_lock_clock_reads` owns). The
//! events are SEEDED into the shared store, because no wire call can place an
//! event a year in the past.

use std::time::{Duration, SystemTime};

use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::state::{
    self, PendingTransition, StoredEvent, EVENT_RETRIEVAL_WINDOW,
};

use super::automation_event_retrieval_window::{listed_ids, summarised_count};
use super::automation_events::stored_event;
use super::automation_rule_lifecycle::{
    assert_refused, call, call_ok, emulator_with_engine, member, seed_event, string_member,
};
use super::store_lock_clock_reads::hold_store_lock;

/// How long a case holds the store lock, so the request it issues is granted the
/// store only after this has passed.
const LOCK_HELD: Duration = Duration::from_millis(600);

/// How far into the held window the boundary a case is about falls: an event
/// that leaves the retrieval window — or a run that completes — here is still
/// inside the moment the request was issued and already past by the moment the
/// store is handed over, which is the whole difference between the two clock
/// reads.
const CROSSES_AT: Duration = Duration::from_millis(300);

/// The oldest instant an event may have been created at and still be reportable
/// at `at`: [`EVENT_RETRIEVAL_WINDOW`] before it.
fn floor(at: SystemTime) -> SystemTime {
    at.checked_sub(EVENT_RETRIEVAL_WINDOW)
        .expect("an instant more than a retrieval window after the epoch")
}

/// One stored event created at `created`, mid-run, with the completion the clock
/// owes it falling due at `due`.
///
/// The steps are the ones `StartAutomationEvent` gives a started event
/// (`state::steps_for_started_event`), so the run a reader reaches is the one a
/// real start would have stored and the step reader has something to report for
/// it. `eventType` is one of the two values the model's `EventType` enum
/// declares (`smithy_query(query="shape",shape="EventType")`).
fn event_created_at(index: usize, created: SystemTime, due: SystemTime) -> StoredEvent {
    let created_at = DateTime::from(created);
    let stored = stored_event(
        index,
        engine::EMULATOR_ACCOUNT_ID,
        state::EVENT_TYPE_UPGRADE_EBS_VOLUME_TYPE,
        state::EVENT_STATUS_IN_PROGRESS,
        created_at.secs(),
    );
    let steps = state::steps_for_started_event(
        &stored.event_id,
        &stored.event_type,
        stored.resource_id.as_deref(),
        None,
        created_at,
    );
    StoredEvent {
        // The seeded instant carries sub-second precision, which the whole
        // seconds `stored_event` names cannot: a case places an event a few
        // hundred milliseconds either side of the floor.
        created_timestamp: created_at,
        steps,
        pending: Some(PendingTransition {
            due,
            terminal_status: state::EVENT_STATUS_COMPLETE.to_string(),
            first_step: 0,
        }),
        ..stored
    }
}

/// An event created at `created`, whose run the clock finished as soon as it
/// started it: whichever instant a reader judges it at, the status it reports is
/// the terminal one, so the case is about window membership alone.
fn settled_event(index: usize, created: SystemTime) -> StoredEvent {
    event_created_at(index, created, created + state::EVENT_SETTLEMENT_DELAY)
}

/// One row of the floor table: where the event's `createdTimestamp` sits
/// relative to the floor, and whether an event there is still reportable once
/// the store is handed over.
struct FloorCase {
    case: &'static str,
    /// The event's `createdTimestamp`, from the instant the row began.
    created: fn(SystemTime) -> SystemTime,
    reportable: bool,
}

/// One row of the reader table: an operation that reports an automation event,
/// how it is asked about one, whether it answers for an unreachable event with
/// the modelled 404 (the per-`eventId` readers) or with a page that omits it,
/// and how to see whether it reported the event.
struct ReaderCase {
    case: &'static str,
    operation: &'static str,
    request: fn(&str) -> Value,
    per_id: bool,
    reports: fn(&Value, &str) -> bool,
}

/// Did `GetAutomationEvent` report this event?
fn get_reports(body: &Value, event_id: &str) -> bool {
    string_member(body, "eventId") == event_id
}

/// Did `ListAutomationEventSteps` report this event's run?
fn steps_report(body: &Value, _event_id: &str) -> bool {
    member(body, "automationEventSteps")
        .as_array()
        .is_some_and(|steps| !steps.is_empty())
}

/// Did `ListAutomationEvents` report this event?
fn listing_reports(body: &Value, event_id: &str) -> bool {
    listed_ids(body).iter().any(|id| id == event_id)
}

/// Did `ListAutomationEventSummaries` count this event? One event is seeded per
/// row, so any counted event is that one.
fn summary_reports(body: &Value, _event_id: &str) -> bool {
    summarised_count(body) > 0
}

/// The event-reporting surface of this service: the per-id reader, the per-id
/// step listing, the event listing and the status summary.
fn readers() -> Vec<ReaderCase> {
    vec![
        ReaderCase {
            case: "get-automation-event",
            operation: "GetAutomationEvent",
            request: |event_id| json!({"eventId": event_id}),
            per_id: true,
            reports: get_reports,
        },
        ReaderCase {
            case: "list-automation-event-steps",
            operation: "ListAutomationEventSteps",
            request: |event_id| json!({"eventId": event_id}),
            per_id: true,
            reports: steps_report,
        },
        ReaderCase {
            case: "list-automation-events",
            operation: "ListAutomationEvents",
            request: |_| json!({}),
            per_id: false,
            reports: listing_reports,
        },
        ReaderCase {
            case: "list-automation-event-summaries",
            operation: "ListAutomationEventSummaries",
            request: |_| json!({}),
            per_id: false,
            reports: summary_reports,
        },
    ]
}

/// Every reader of the event store judges the retrieval window against an
/// instant read under the store lock, so a request that waited for the store
/// reports exactly the events that are inside the window when it is handed over.
///
/// The table crosses the reporting surface with where the event sits relative to
/// the floor: one that leaves the window DURING the wait for the store (the
/// sliver a pre-lock clock read would still admit), one that left it before the
/// call was made at all (where either instant decides the same way), and one
/// that stays inside it well past the release (where a reader must not drop an
/// event the floor has not reached).
#[tokio::test]
async fn every_event_reader_judges_the_window_from_the_store_time_clock() {
    let positions = [
        FloorCase {
            case: "leaves-the-window-while-the-lock-is-held",
            created: |base| floor(base) + CROSSES_AT,
            reportable: false,
        },
        FloorCase {
            case: "left-the-window-before-the-call",
            created: |base| floor(base) - Duration::from_secs(60),
            reportable: false,
        },
        FloorCase {
            case: "stays-in-the-window-past-the-release",
            created: |base| floor(base) + Duration::from_secs(30),
            reportable: true,
        },
    ];

    for (position_index, position) in positions.iter().enumerate() {
        for (reader_index, reader) in readers().iter().enumerate() {
            let case = format!("{}/{}", position.case, reader.case);
            // One store per row: a row's contention is its own.
            let (router, engine) = emulator_with_engine();
            let base = SystemTime::now();
            let event = settled_event(
                position_index * 10 + reader_index,
                (position.created)(base),
            );
            seed_event(&engine, &event);

            let (held, holder) = hold_store_lock(&engine, LOCK_HELD);
            held.recv().expect("the holder takes the store lock");
            let answer = call(&router, reader.operation, &(reader.request)(&event.event_id)).await;
            holder.join().expect("the holder thread finishes");

            if !position.reportable && reader.per_id {
                // An event outside the window is as absent as an id the store
                // never held, which is the refusal this reader answers with.
                assert_refused(answer, 404, "ResourceNotFoundException", &case);
                continue;
            }

            let (status, body) = answer;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from {}, got {status} and {body}",
                reader.operation
            );
            assert_eq!(
                (reader.reports)(&body, &event.event_id),
                position.reportable,
                "case {case:?}: expected the event to be reported: {}, got {body}",
                position.reportable
            );
        }
    }
}

/// `ListAutomationEvents` answers one request from one instant: the floor it
/// selects on and the statuses it reports are the same clock read.
///
/// Two events are seeded for the single call: one that falls out of the
/// retrieval window while the request waits for the store, and one well inside
/// it whose run completes during that same wait. A response built from one
/// post-lock instant omits the first and reports the second as completed; one
/// built from an instant read before the lock would report the first and call
/// the second still running.
#[tokio::test]
async fn list_automation_events_answers_one_request_from_one_instant() {
    let (router, engine) = emulator_with_engine();
    let base = SystemTime::now();
    let crossing = settled_event(30, floor(base) + CROSSES_AT);
    // Created an hour ago — well inside the window however long the wait — with
    // its completion falling due in the middle of the wait for the store.
    let completing = event_created_at(
        31,
        base - Duration::from_secs(3_600),
        base + CROSSES_AT,
    );
    seed_event(&engine, &crossing);
    seed_event(&engine, &completing);

    let (held, holder) = hold_store_lock(&engine, LOCK_HELD);
    held.recv().expect("the holder takes the store lock");
    let listed = call_ok(&router, "ListAutomationEvents", &json!({}), "one-instant").await;
    holder.join().expect("the holder thread finishes");

    let case = "one-instant";
    assert!(
        !listing_reports(&listed, &crossing.event_id),
        "case {case:?}: expected the event the floor passed while the call waited for the store \
         to be absent, got {listed}"
    );
    assert!(
        listing_reports(&listed, &completing.event_id),
        "case {case:?}: expected the event inside the window to be reported, got {listed}"
    );
    let reported = member(&listed, "automationEvents")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|event| string_member(event, "eventId") == completing.event_id)
        .unwrap_or_else(|| panic!("case {case:?}: the reported event is readable, got {listed}"));
    assert_eq!(
        string_member(&reported, "eventStatus"),
        state::EVENT_STATUS_COMPLETE,
        "case {case:?}: expected the status the clock made due before the store was handed over, \
         got {reported}"
    );

    // The floor is not liftable: a caller asking from the epoch gets the same
    // answer about the event that left the window.
    let from_the_epoch = call_ok(
        &router,
        "ListAutomationEvents",
        &json!({"startTimeInclusive": 0}),
        "from-the-epoch",
    )
    .await;
    assert!(
        !listing_reports(&from_the_epoch, &crossing.event_id),
        "case \"from-the-epoch\": expected an event before the floor to stay absent however far \
         back the caller asks, got {from_the_epoch}"
    );
}
