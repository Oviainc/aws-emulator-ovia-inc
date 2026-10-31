// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the retrieval window every reader of the automation
//! event store answers under.
//!
//! The rule: an automation event is reportable only while it was created inside
//! the retrieval window (`ListAutomationEvents` documents "You can retrieve
//! events that were created within the past year"), so an event created before
//! that floor is out of reach of EVERY query of the store — the per-`eventId`
//! readers refuse it with the modelled `ResourceNotFoundException` (404), as
//! they do an id the store never held, and the listings neither report nor count
//! it however far back the caller's start bound points.
//!
//! Each case drives the service the way a client does: an awsJson1_0 `POST /`
//! through the same router the emulator binary mounts. The
//! events are SEEDED into the shared store, because an event created through
//! `StartAutomationEvent` is stamped with the instant of the call and no wire
//! call can place one a year in the past.

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::state::{
    self, StoredEvent, StoredStep, EVENT_RETRIEVAL_WINDOW,
};

use crate::automation_events::{emulator, now_secs, seed, stored_event};
use crate::automation_rule_lifecycle::{assert_refused, call, call_ok, member, string_member};

/// The retrieval window in whole seconds, which is what a case offsets an
/// event's `createdTimestamp` by.
fn window_secs() -> i64 {
    i64::try_from(EVENT_RETRIEVAL_WINDOW.as_secs()).expect("a representable window")
}

/// One row of the table: what a failing assertion calls it, how long before the
/// base instant the event was created, and whether an event created then is
/// still reportable.
struct Row {
    case: &'static str,
    age_secs: i64,
    reachable: bool,
}

/// One seeded event, created `age_secs` before `base`, carrying one step so the
/// step reader has something to report for it.
///
/// `eventType` alternates over the two values the model's `EventType` enum
/// declares (`smithy_query(query="shape",shape="EventType")`:
/// `SnapshotAndDeleteUnattachedEbsVolume` | `UpgradeEbsVolumeType`), and the
/// step type is the one that event type performs, so no row depends on the one
/// pair the issue cited.
fn seeded_event(index: usize, base: i64, age_secs: i64) -> StoredEvent {
    let snapshot_and_delete = index.is_multiple_of(2);
    let (event_type, step_type) = if snapshot_and_delete {
        (
            state::EVENT_TYPE_SNAPSHOT_AND_DELETE_UNATTACHED_EBS_VOLUME,
            state::STEP_TYPE_CREATE_EBS_SNAPSHOT,
        )
    } else {
        (
            state::EVENT_TYPE_UPGRADE_EBS_VOLUME_TYPE,
            state::STEP_TYPE_MODIFY_EBS_VOLUME,
        )
    };
    let stored = stored_event(
        index,
        engine::EMULATOR_ACCOUNT_ID,
        event_type,
        state::EVENT_STATUS_COMPLETE,
        base - age_secs,
    );
    let step = StoredStep {
        step_id: format!("stp{index:013}"),
        step_type: step_type.to_string(),
        step_status: state::STEP_STATUS_COMPLETE.to_string(),
        resource_id: stored.resource_id.clone(),
        start_timestamp: None,
        completed_timestamp: None,
        estimated_monthly_savings: None,
    };
    StoredEvent {
        steps: vec![step],
        ..stored
    }
}

/// The `eventId`s one `ListAutomationEvents` response reports, in response
/// order.
///
/// `pub(crate)` with [`summarised_count`] below, because the sibling suite about
/// the instant this window is judged at reads a response the same way rather
/// than growing a copy.
pub(crate) fn listed_ids(body: &Value) -> Vec<String> {
    member(body, "automationEvents")
        .as_array()
        .unwrap_or_else(|| panic!("automationEvents must be a list, got {body}"))
        .iter()
        .map(|event| string_member(event, "eventId"))
        .collect()
}

/// How many events one `ListAutomationEventSummaries` response counted, over
/// every group it reports.
pub(crate) fn summarised_count(body: &Value) -> i64 {
    let listed = member(body, "automationEventSummaries");
    if listed.is_null() {
        return 0;
    }
    listed
        .as_array()
        .unwrap_or_else(|| panic!("automationEventSummaries must be a list, got {body}"))
        .iter()
        .map(|summary| {
            member(member(summary, "total"), "automationEventCount")
                .as_i64()
                .unwrap_or_default()
        })
        .sum()
}

/// Every reader of the event store agrees about one event: inside the retrieval
/// window it is reportable by id, listable and counted; before the window it is
/// as absent as an id the store never held, to the per-id readers and the
/// listings alike.
#[tokio::test]
async fn every_event_reader_agrees_on_the_retrieval_window() {
    let (router, engine) = emulator();
    let base = now_secs();
    let window = window_secs();

    // The class the rule generalises over is the event's AGE: a fresh event,
    // ages well inside the window, one just inside its far edge, and three ways
    // of being before it (just past the edge, a day past it, and many windows
    // back).
    let rows = vec![
        Row {
            case: "created-at-the-base-instant",
            age_secs: 0,
            reachable: true,
        },
        Row {
            case: "an-hour-old",
            age_secs: 3_600,
            reachable: true,
        },
        Row {
            case: "a-day-old",
            age_secs: 86_400,
            reachable: true,
        },
        Row {
            case: "half-the-window-old",
            age_secs: window / 2,
            reachable: true,
        },
        Row {
            case: "just-inside-the-far-edge",
            age_secs: window - 60,
            reachable: true,
        },
        Row {
            case: "just-outside-the-far-edge",
            age_secs: window + 60,
            reachable: false,
        },
        Row {
            case: "a-day-outside",
            age_secs: window + 86_400,
            reachable: false,
        },
        Row {
            case: "five-windows-old",
            age_secs: 5 * window,
            reachable: false,
        },
        Row {
            case: "created-at-the-epoch",
            age_secs: base,
            reachable: false,
        },
    ];

    let events: Vec<StoredEvent> = rows
        .iter()
        .enumerate()
        .map(|(index, row)| seeded_event(index, base, row.age_secs))
        .collect();
    seed(&engine, &events);

    for (row, event) in rows.iter().zip(&events) {
        let case = row.case;
        let request = json!({"eventId": &event.event_id});
        if row.reachable {
            let reported = call_ok(&router, "GetAutomationEvent", &request, case).await;
            for (field, expected) in [
                ("eventId", event.event_id.clone()),
                ("eventType", event.event_type.clone()),
                ("eventStatus", event.event_status.clone()),
                ("accountId", event.account_id.clone()),
            ] {
                assert_eq!(
                    string_member(&reported, field),
                    expected,
                    "case {case:?}: expected {field} {expected:?}, got {reported}"
                );
            }
            let created = member(&reported, "createdTimestamp").as_f64();
            let expected_created = (base - row.age_secs) as f64;
            assert_eq!(
                created,
                Some(expected_created),
                "case {case:?}: expected createdTimestamp {expected_created}, got {reported}"
            );

            let steps = call_ok(&router, "ListAutomationEventSteps", &request, case).await;
            let step_ids: Vec<String> = member(&steps, "automationEventSteps")
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: expected a step list, got {steps}"))
                .iter()
                .map(|step| string_member(step, "stepId"))
                .collect();
            assert_eq!(
                step_ids,
                vec![event.steps[0].step_id.clone()],
                "case {case:?}: a reportable event's steps are listable, got {steps}"
            );
        } else {
            for operation in ["GetAutomationEvent", "ListAutomationEventSteps"] {
                let refused = call(&router, operation, &request).await;
                assert_refused(refused, 404, "ResourceNotFoundException", case);
            }
        }
    }

    // The id readers and the listings agree: exactly the events inside the
    // window are reported, however far back the caller asks.
    let reachable: Vec<String> = rows
        .iter()
        .zip(&events)
        .filter(|(row, _)| row.reachable)
        .map(|(_, event)| event.event_id.clone())
        .collect();
    for (case, request) in [
        ("listing-with-no-bounds", json!({})),
        (
            "listing-from-the-epoch",
            json!({"startTimeInclusive": 0, "endTimeExclusive": base + 60}),
        ),
        (
            "listing-from-ten-windows-back",
            json!({"startTimeInclusive": base - 10 * window}),
        ),
    ] {
        let listed = call_ok(&router, "ListAutomationEvents", &request, case).await;
        let mut ids = listed_ids(&listed);
        ids.sort();
        let mut expected = reachable.clone();
        expected.sort();
        assert_eq!(
            ids, expected,
            "case {case:?}: expected exactly the events inside the retrieval window, got {listed}"
        );
    }

    let expected_count = i64::try_from(reachable.len()).expect("a representable count");
    for (case, request) in [
        ("summary-with-no-bounds", json!({})),
        (
            "summary-from-the-epoch",
            json!({"startDateInclusive": "1970-01-01"}),
        ),
    ] {
        let summarised = call_ok(&router, "ListAutomationEventSummaries", &request, case).await;
        assert_eq!(
            summarised_count(&summarised),
            expected_count,
            "case {case:?}: expected {expected_count} events counted, got {summarised}"
        );
    }
}

/// Reading a reportable event changes nothing a later caller sees: the same
/// `GetAutomationEvent` answer twice over, and the listings unmoved by the read.
#[tokio::test]
async fn reading_a_reportable_event_leaves_the_store_as_it_was() {
    let (router, engine) = emulator();
    let base = now_secs();
    let window = window_secs();

    // One event at each end of the reportable range, so the read is exercised
    // both on a fresh event and on one at the far edge of the window.
    let events = vec![
        seeded_event(0, base, 0),
        seeded_event(1, base, window - 3_600),
    ];
    seed(&engine, &events);

    let before = call_ok(&router, "ListAutomationEvents", &json!({}), "before").await;
    for event in &events {
        let case = event.event_id.clone();
        let request = json!({"eventId": &event.event_id});
        let first = call_ok(&router, "GetAutomationEvent", &request, &case).await;
        let second = call_ok(&router, "GetAutomationEvent", &request, &case).await;
        assert_eq!(
            first, second,
            "case {case:?}: a read reports the same event twice, got {first} then {second}"
        );
    }
    let after = call_ok(&router, "ListAutomationEvents", &json!({}), "after").await;
    assert_eq!(
        listed_ids(&after),
        listed_ids(&before),
        "case \"after-the-reads\": reading an event by id changes no listing, got {after}"
    );
}

/// An event before the window is refused in exactly the words, code and status
/// an `eventId` the store never held is: the retrieval floor makes an event as
/// absent as one that was never stored, so the refusal discloses nothing about
/// which of the two it was.
#[tokio::test]
async fn an_id_the_store_never_held_is_refused_like_one_outside_the_window() {
    let (router, engine) = emulator();
    let base = now_secs();
    let outside = seeded_event(0, base, window_secs() + 1);
    seed(&engine, std::slice::from_ref(&outside));

    // Well-formed `EventId`s (`^[0-9A-Za-z]{16}$`) no stored event carries,
    // differing in case, digits and letters.
    let cases: Vec<(&str, String)> = vec![
        ("outside-the-window", outside.event_id.clone()),
        ("absent-uppercase", "AAAAAAAAAAAAAAAA".to_string()),
        ("absent-digits", "0000000000000000".to_string()),
        ("absent-mixed", "zZ9aA0bB1cC2dD3e".to_string()),
    ];
    for (case, event_id) in &cases {
        let (status, body) = call(&router, "GetAutomationEvent", &json!({"eventId": event_id})).await;
        assert_refused((status, body.clone()), 404, "ResourceNotFoundException", case);
        assert_eq!(
            string_member(&body, "message"),
            state::RESOURCE_NOT_FOUND_MESSAGE,
            "case {case:?}: every unreachable event is refused in the same words, got {body}"
        );
    }
}
