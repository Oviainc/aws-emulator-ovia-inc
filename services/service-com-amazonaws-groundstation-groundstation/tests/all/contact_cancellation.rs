// SPDX-License-Identifier: CC0-1.0

//! `CancelContact`: which contacts a cancel can still release, and what it leaves behind.
//!
//! The RULE under test: a cancel RELEASES a contact's reservation, so it is answered only
//! while there is one to release. A contact whose pass has yet to start is cancelled
//! outright; one whose pass is under way is STOPPED, which it reports as `CANCELLING`
//! until the stop settles on `CANCELLED`; a stop already under way is answered without
//! starting a second one; and a contact in every other `ContactStatus` — already
//! released, its pass over, never to be flown, or a window never reserved — is refused
//! with the operation's modelled `InvalidParameterException` (HTTP 431) and keeps the
//! status it held. Antenna time a pass has already used is not given back by a cancel, so
//! a stopped or a refused contact still counts its minutes against the month's usage.
//!
//! The suites below assert that over the WHOLE `ContactStatus` enum rather than one call,
//! through the real front door — the router, deserialization, constraint validation, the
//! handler and serialization — on the harness `scheduling_loop.rs` stands up.

use service_com_amazonaws_groundstation_groundstation::state::service_state;
use smithy_server_runtime::MultiServiceRouter;

use crate::minute_usage::{epoch, minute_usage, minutes, month_of, reserve_pass};
use crate::scheduling_loop::{create_mission_profile, harness_with_engine, send, AFTER_SETTLE};

/// What a cancel does to a contact that holds a given `ContactStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// Accepted, and the reservation is released outright: the pass had yet to start, so
    /// the contact reads back `CANCELLED`.
    Cancelled,
    /// Accepted as a STOP of a pass already under way: the contact reads back
    /// `CANCELLING`, then `CANCELLED` once the stop the service owes it is due.
    Stopped,
    /// Accepted and changes nothing: a stop is already being carried out.
    AlreadyStopping,
    /// Refused with the modelled `InvalidParameterException`, status untouched.
    Refused,
}

/// Every value of the model's `ContactStatus`, beside what a cancel of a contact in it
/// does.
///
/// Source: `smithy_query(query="shape", shape="ContactStatus",
/// model_path="services/service-com-amazonaws-groundstation-groundstation/smithy-model/\
/// com-amazonaws-groundstation-groundstation.json")` — all thirteen values, not only the
/// ones a request can reach today, because the rule is over the whole enum. Which side of
/// the pass each one sits on is `CancelContact`'s own doc: "for contacts that: have yet to
/// start, the contact will be cancelled. Have started but have yet to finish, the contact
/// will be stopped".
const CANCEL_OUTCOMES: &[(&str, Outcome)] = &[
    ("AVAILABLE", Outcome::Refused),
    ("AWS_CANCELLED", Outcome::Refused),
    ("AWS_FAILED", Outcome::Refused),
    ("CANCELLED", Outcome::Refused),
    ("CANCELLING", Outcome::AlreadyStopping),
    ("COMPLETED", Outcome::Refused),
    ("FAILED", Outcome::Refused),
    ("FAILED_TO_SCHEDULE", Outcome::Refused),
    ("PASS", Outcome::Stopped),
    ("POSTPASS", Outcome::Stopped),
    ("PREPASS", Outcome::Stopped),
    ("SCHEDULED", Outcome::Cancelled),
    ("SCHEDULING", Outcome::Cancelled),
];

/// Put the stored contact into `status`.
///
/// Most of these statuses are reached by the scheduler and by the pass itself, which no
/// request drives, so the stored contact's status is set directly — the one place these
/// suites touch the store instead of the front door.
fn put_in_status(engine: &engine::InMemoryEngine, contact_id: &str, status: &str) {
    let state = service_state(engine).expect("this service's state store");
    let mut guard = state.lock();
    guard
        .contacts
        .get_mut(contact_id)
        .expect("the reserved contact")
        .contact_status = status.to_string();
}

/// `DescribeContact`'s `contactStatus` for `contact_id`.
async fn described_status(router: &MultiServiceRouter, contact_id: &str, case: &str) -> String {
    let described = send(router, "GET", &format!("/contact/{contact_id}"), None).await;
    assert_eq!(
        described.status, 200,
        "case {case}: DescribeContact expected 200, got {} {}",
        described.status, described.body
    );
    described.string("contactStatus")
}

#[tokio::test]
async fn a_cancel_releases_only_a_contact_that_still_holds_a_reservation() {
    let (router, engine) = harness_with_engine();
    let profile_arn = create_mission_profile(&router, "cancel table", 60, None, None).await;
    let start = epoch(2033, 5, 17, 9, 0);

    for (status, outcome) in CANCEL_OUTCOMES {
        // A contact of its own per row, so one row's release cannot be what the next row
        // observes.
        let contact_id = reserve_pass(&router, &profile_arn, status, start, 20 * 60).await;
        put_in_status(&engine, &contact_id, status);

        let cancelled = send(&router, "DELETE", &format!("/contact/{contact_id}"), None).await;

        if *outcome == Outcome::Refused {
            assert_eq!(
                cancelled.status, 431,
                "case {status}: expected the modelled InvalidParameterException status 431, \
                 got {} {}",
                cancelled.status, cancelled.body
            );
            assert_eq!(
                cancelled.error_code(),
                "InvalidParameterException",
                "case {status}: wrong error shape, got {}",
                cancelled.body
            );
            let message = cancelled.field("message").to_string();
            assert!(
                message.contains(status),
                "case {status}: the refusal must name the status that refuses it, got \
                 {message:?}"
            );
            let unchanged = described_status(&router, &contact_id, status).await;
            assert_eq!(
                &unchanged, status,
                "case {status}: a refused cancel leaves the status untouched, got \
                 {unchanged:?}"
            );
            continue;
        }

        assert_eq!(
            cancelled.status, 200,
            "case {status}: CancelContact expected 200, got {} {}",
            cancelled.status, cancelled.body
        );
        assert_eq!(
            cancelled.string("contactId"),
            contact_id,
            "case {status}: another contactId came back, got {}",
            cancelled.body
        );
        assert_eq!(
            cancelled.field("versionId").as_i64(),
            Some(1),
            "case {status}: the contact's only version is 1, got {}",
            cancelled.body
        );

        // The read-back: what the cancel did is visible to the next caller.
        let expected_now = match outcome {
            Outcome::Cancelled => "CANCELLED",
            // A stop of a pass under way is reported while it is carried out, so a caller
            // reading straight after the cancel still sees the release in flight.
            Outcome::Stopped | Outcome::AlreadyStopping => "CANCELLING",
            Outcome::Refused => unreachable!("handled above"),
        };
        let now = described_status(&router, &contact_id, status).await;
        assert_eq!(
            now, expected_now,
            "case {status}: expected the contact to read back {expected_now}, got {now:?}"
        );

        if *outcome == Outcome::Stopped {
            // The stop the service owes the contact settles on CANCELLED on its own.
            std::thread::sleep(AFTER_SETTLE);
            let settled = described_status(&router, &contact_id, status).await;
            assert_eq!(
                settled, "CANCELLED",
                "case {status}: a stop settles on CANCELLED, got {settled:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_cancel_gives_back_only_antenna_time_no_pass_has_used() {
    // Rows differ along where the contact sits in its lifecycle when the cancel arrives:
    // before the pass (the reservation is released, so the minutes go back), during the
    // pass (the contact is stopped, and the antenna it already held stays spent) and after
    // it (the cancel is refused, so nothing moves at all). The minutes expected are the
    // whole window's, since a part-used pass still had the antenna for its window.
    let pass_minutes = 45_i64;
    let rows: &[(&str, i64)] = &[
        ("SCHEDULING", 0),
        ("SCHEDULED", 0),
        ("PREPASS", pass_minutes),
        ("PASS", pass_minutes),
        ("POSTPASS", pass_minutes),
        ("COMPLETED", pass_minutes),
        ("FAILED", pass_minutes),
    ];

    for (status, expected_after) in rows {
        // One store per row, so the month's usage is the one contact's.
        let (router, engine) = harness_with_engine();
        let profile_arn = create_mission_profile(&router, "antenna time", 60, None, None).await;
        let start = epoch(2034, 2, 8, 11, 0);
        let month = month_of(start);
        let contact_id = reserve_pass(&router, &profile_arn, status, start, pass_minutes * 60).await;
        put_in_status(&engine, &contact_id, status);

        let before = minute_usage(&router, month).await;
        assert_eq!(
            minutes(&before, "totalScheduledMinutes", status),
            pass_minutes,
            "case {status}: before the cancel this status holds the window's {pass_minutes} \
             minutes, got {}",
            before.body
        );

        send(&router, "DELETE", &format!("/contact/{contact_id}"), None).await;

        let after = minute_usage(&router, month).await;
        assert_eq!(
            minutes(&after, "totalScheduledMinutes", status),
            *expected_after,
            "case {status}: expected {expected_after} minutes after the cancel, got {}",
            after.body
        );

        // And once every move the cancel started has settled, the answer is the same: a
        // stop that lands on CANCELLED still does not give back the antenna time its pass
        // used.
        std::thread::sleep(AFTER_SETTLE);
        let settled = minute_usage(&router, month).await;
        assert_eq!(
            minutes(&settled, "totalScheduledMinutes", status),
            *expected_after,
            "case {status}: expected {expected_after} minutes once the cancel settled, got {}",
            settled.body
        );
    }
}
