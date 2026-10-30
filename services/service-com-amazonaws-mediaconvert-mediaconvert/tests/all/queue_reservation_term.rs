// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of WHEN a reserved queue's 12-month commitment term begins.
//!
//! One rule, asserted over a table that spans what it generalises over rather than the one
//! value a report happened to carry:
//!
//! * A reservation plan's TERM — the instant it was set up (`PurchasedAt`) and the instant
//!   it runs out (`ExpiresAt`, a commitment later) — begins again only on a call that
//!   PURCHASES capacity. `ReservationPlanSettings$ReservedSlots`: "You can't decrease the
//!   number of RTS in your reserved queue. You can increase the number of RTS by extending
//!   your existing commitment with a new 12-month commitment for the larger number. The new
//!   commitment begins when you purchase the additional capacity." So an update stating a
//!   slot count the standing plan already covers buys nothing and leaves both instants
//!   where they were, while one stating more — or one replacing a plan whose term has
//!   already run out, which `UpdateQueue$ReservationPlanSettings` calls setting "up a new
//!   pricing plan to replace an expired one", entering "another 12-month commitment" —
//!   starts a term at the call.
//! * Either way every member the settings state is "the new details of your pricing plan",
//!   so the commitment length, the renewal type and the slot count read back as sent.
//!
//! The age of the standing commitment is STATE, so each row pins it in the store instead of
//! waiting out real time; everything it then asserts is read back over the wire, from the
//! update's own response and from the `GetQueue` after it.
//!
//! Each case drives the real restJson1 wire path through the router (the harness is the one
//! `job_lifecycle` builds), so what is asserted is the bytes a client would see.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::json;

use service_com_amazonaws_mediaconvert_mediaconvert::state::{
    epoch_seconds, MIN_RESERVED_SLOTS, RESERVATION_TERM,
};
use service_com_amazonaws_mediaconvert_mediaconvert::ComAmazonawsMediaconvertMediaconvertState;

use super::job_lifecycle::Emulator;
use super::queue_update::update_queue;

/// The reserved queue every row works on.
const QUEUE: &str = "reserved";

/// A day, the unit the rows state the age of a standing commitment in.
const DAY: Duration = Duration::from_secs(24 * 60 * 60);

/// An instant as the `__timestampUnix` (epoch-seconds) wire value of a timestamp member.
fn secs(at: SystemTime) -> i64 {
    i64::try_from(
        at.duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_secs()),
    )
    .unwrap_or_default()
}

/// What the stored plan's term must do once the update is applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Term {
    /// No capacity is purchased, so the standing commitment is kept: both of its instants
    /// are exactly where they were.
    Kept,
    /// Capacity is purchased, so a fresh commitment begins at this call.
    Begun,
}

/// One row: the plan the queue stands under, the plan an update states, how long the
/// standing commitment has been running, and what must become of its term.
struct TermCase {
    label: &'static str,
    /// `ReservedSlots` the standing plan reserves.
    stored_slots: i32,
    /// `ReservedSlots` the update states.
    stated_slots: i32,
    /// `RenewalType` the standing plan carries.
    stored_renewal: &'static str,
    /// `RenewalType` the update states.
    stated_renewal: &'static str,
    /// How long before the update the standing commitment was set up.
    age: Duration,
    expected: Term,
}

/// The rows span what the rule generalises over: the DIRECTION of the stated count against
/// the standing one (equal and higher) and the SIZE of an increase (one slot and several),
/// the slot count it starts from (the published floor [`MIN_RESERVED_SLOTS`], a small plan
/// and a larger one), both modelled `RenewalType` values in both directions
/// (`smithy_query(query="shape",shape="RenewalType")`: AUTO_RENEW, EXPIRE), whether the
/// stated details change at all, and how far into its term the standing commitment is —
/// moments in, part-way through, and past its end.
fn term_cases() -> Vec<TermCase> {
    vec![
        TermCase {
            label: "the same slot count, a new renewal type",
            stored_slots: 4,
            stated_slots: 4,
            stored_renewal: "EXPIRE",
            stated_renewal: "AUTO_RENEW",
            age: 30 * DAY,
            expected: Term::Kept,
        },
        TermCase {
            label: "the same slot count and the same details, part-way through the term",
            stored_slots: 3,
            stated_slots: 3,
            stored_renewal: "AUTO_RENEW",
            stated_renewal: "AUTO_RENEW",
            age: 182 * DAY,
            expected: Term::Kept,
        },
        TermCase {
            label: "the same slot count moments into the term",
            stored_slots: 7,
            stated_slots: 7,
            stored_renewal: "AUTO_RENEW",
            stated_renewal: "EXPIRE",
            age: Duration::from_secs(2),
            expected: Term::Kept,
        },
        TermCase {
            label: "the smallest plan there is, restated",
            stored_slots: MIN_RESERVED_SLOTS,
            stated_slots: MIN_RESERVED_SLOTS,
            stored_renewal: "EXPIRE",
            stated_renewal: "EXPIRE",
            age: DAY,
            expected: Term::Kept,
        },
        TermCase {
            label: "one more slot: capacity purchased",
            stored_slots: 4,
            stated_slots: 5,
            stored_renewal: "EXPIRE",
            stated_renewal: "EXPIRE",
            age: 30 * DAY,
            expected: Term::Begun,
        },
        TermCase {
            label: "four times the slots",
            stored_slots: 2,
            stated_slots: 8,
            stored_renewal: "AUTO_RENEW",
            stated_renewal: "EXPIRE",
            age: 100 * DAY,
            expected: Term::Begun,
        },
        TermCase {
            label: "one more slot than the smallest plan there is",
            stored_slots: MIN_RESERVED_SLOTS,
            stated_slots: MIN_RESERVED_SLOTS + 1,
            stored_renewal: "EXPIRE",
            stated_renewal: "AUTO_RENEW",
            age: 10 * DAY,
            expected: Term::Begun,
        },
        TermCase {
            label: "a plan replacing one whose term has run out",
            stored_slots: 4,
            stated_slots: 4,
            stored_renewal: "EXPIRE",
            stated_renewal: "AUTO_RENEW",
            age: RESERVATION_TERM + DAY,
            expected: Term::Begun,
        },
    ]
}

/// Put the stored plan's term `age` into the past, so the row starts from a commitment set
/// up that long before the call it then makes. Answers the two wire instants the standing
/// plan then carries.
fn age_the_standing_commitment(emulator: &Emulator, age: Duration) -> (i64, i64) {
    let purchased = SystemTime::now() - age;
    let expires = purchased + RESERVATION_TERM;
    let state = emulator
        .engine
        .state::<ComAmazonawsMediaconvertMediaconvertState>();
    let mut state = state.lock();
    let plan = state
        .queues
        .get_mut(QUEUE)
        .and_then(|record| record.reservation_plan.as_mut())
        .expect("the fixture reserved queue carries a reservation plan");
    plan.purchased_at = Some(epoch_seconds(purchased));
    plan.expires_at = Some(epoch_seconds(expires));
    (secs(purchased), secs(expires))
}

/// A reservation plan's 12-month term begins again only on the call that purchases capacity
/// the standing plan does not already cover: restating a count the plan already reserves
/// changes the stated details and leaves the commitment where it is.
#[tokio::test]
async fn a_commitment_begins_only_when_capacity_is_purchased() {
    for case in term_cases() {
        let TermCase {
            label,
            stored_slots,
            stated_slots,
            stored_renewal,
            stated_renewal,
            age,
            expected,
        } = case;
        // One account per row, so a row only ever reads its own mutation.
        let emulator = Emulator::new();
        // `Commitment` models exactly one value
        // (`smithy_query(query="shape",shape="Commitment")`).
        let created = emulator
            .create_queue(json!({
                "name": QUEUE,
                "pricingPlan": "RESERVED",
                "reservationPlanSettings": {
                    "commitment": "ONE_YEAR",
                    "renewalType": stored_renewal,
                    "reservedSlots": stored_slots
                }
            }))
            .await;
        assert_eq!(
            created.status, 201,
            "case {label:?}: the reserved queue must be created, got {} with body {}",
            created.status, created.body
        );

        let (standing_purchase, standing_expiry) = age_the_standing_commitment(&emulator, age);
        let at_the_call = secs(SystemTime::now());
        let updated = update_queue(
            &emulator,
            QUEUE,
            json!({
                "reservationPlanSettings": {
                    "commitment": "ONE_YEAR",
                    "renewalType": stated_renewal,
                    "reservedSlots": stated_slots
                }
            }),
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {label:?}: expected HTTP 200 from UpdateQueue, got {} with body {}",
            updated.status, updated.body
        );
        let past_the_call = secs(SystemTime::now());

        let fetched = emulator.get_queue(QUEUE).await;
        assert_eq!(
            fetched.status, 200,
            "case {label:?}: expected HTTP 200 from GetQueue, got {} with body {}",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.at("/queue"),
            updated.at("/queue"),
            "case {label:?}: GetQueue must project what UpdateQueue stored, got {}",
            fetched.body
        );

        for (source, reply) in [("UpdateQueue", &updated), ("GetQueue", &fetched)] {
            // "The new details of your pricing plan": every stated member reads back as
            // sent, whichever term the queue ends up under.
            assert_eq!(
                reply.at("/queue/reservationPlan/reservedSlots"),
                &json!(stated_slots),
                "case {label:?} via {source}: expected reservedSlots {stated_slots}, got {}",
                reply.body
            );
            assert_eq!(
                reply.string("/queue/reservationPlan/renewalType"),
                stated_renewal,
                "case {label:?} via {source}: expected renewalType {stated_renewal:?}, got {}",
                reply.body
            );
            assert_eq!(
                reply.string("/queue/reservationPlan/commitment"),
                "ONE_YEAR",
                "case {label:?} via {source}: expected commitment \"ONE_YEAR\", got {}",
                reply.body
            );

            let purchased = reply
                .at("/queue/reservationPlan/purchasedAt")
                .as_i64()
                .unwrap_or_default();
            let expires = reply
                .at("/queue/reservationPlan/expiresAt")
                .as_i64()
                .unwrap_or_default();
            let term = i64::try_from(RESERVATION_TERM.as_secs()).unwrap_or_default();
            match expected {
                Term::Kept => {
                    assert_eq!(
                        purchased, standing_purchase,
                        "case {label:?} via {source}: no capacity was purchased, so the \
                         commitment must still be the one set up at {standing_purchase}, got \
                         {purchased} from body {}",
                        reply.body
                    );
                    assert_eq!(
                        expires, standing_expiry,
                        "case {label:?} via {source}: expected the standing commitment to \
                         still run out at {standing_expiry}, got {expires} from body {}",
                        reply.body
                    );
                }
                Term::Begun => {
                    assert!(
                        purchased >= at_the_call && purchased <= past_the_call,
                        "case {label:?} via {source}: the new commitment begins when the \
                         capacity is purchased, so expected a purchase instant in \
                         [{at_the_call}, {past_the_call}], got {purchased} from body {}",
                        reply.body
                    );
                    assert_eq!(
                        expires - purchased,
                        term,
                        "case {label:?} via {source}: expected a {term}-second commitment \
                         from the purchase instant, got {} from body {}",
                        expires - purchased,
                        reply.body
                    );
                }
            }
            // `ReservationPlan$Status` is ACTIVE or EXPIRED
            // (`smithy_query(query="shape",shape="ReservationPlanStatus")`): the plan the
            // queue holds after an applied update is under an unexpired commitment either
            // way.
            assert_eq!(
                reply.string("/queue/reservationPlan/status"),
                "ACTIVE",
                "case {label:?} via {source}: expected an ACTIVE plan, got {}",
                reply.body
            );
            assert!(
                expires > past_the_call,
                "case {label:?} via {source}: expected a commitment that has not run out at \
                 {past_the_call}, got {expires} from body {}",
                reply.body
            );
        }
    }
}

/// The term a standing plan carries survives every OTHER member an update can change: a
/// call that states no reservation plan at all cannot move a commitment, however much else
/// it changes.
///
/// The rows differ in which members the update carries beside no plan.
#[tokio::test]
async fn an_update_that_states_no_plan_leaves_the_commitment_alone() {
    let updates = [
        ("the status alone", json!({ "status": "PAUSED" })),
        ("a description alone", json!({ "description": "re-described" })),
        (
            "every other mutable member at once",
            json!({
                "status": "PAUSED",
                "description": "re-described",
                "maximumConcurrentFeeds": 2
            }),
        ),
        ("nothing at all", json!({})),
    ];

    for (label, body) in updates {
        let emulator = Emulator::new();
        let created = emulator
            .create_queue(json!({
                "name": QUEUE,
                "pricingPlan": "RESERVED",
                "reservationPlanSettings": {
                    "commitment": "ONE_YEAR",
                    "renewalType": "EXPIRE",
                    "reservedSlots": 2
                }
            }))
            .await;
        assert_eq!(
            created.status, 201,
            "case {label:?}: the reserved queue must be created, got {} with body {}",
            created.status, created.body
        );
        let (standing_purchase, standing_expiry) =
            age_the_standing_commitment(&emulator, 45 * DAY);

        let updated = update_queue(&emulator, QUEUE, body).await;
        assert_eq!(
            updated.status, 200,
            "case {label:?}: expected HTTP 200 from UpdateQueue, got {} with body {}",
            updated.status, updated.body
        );

        let fetched = emulator.get_queue(QUEUE).await;
        for (source, reply) in [("UpdateQueue", &updated), ("GetQueue", &fetched)] {
            assert_eq!(
                reply.at("/queue/reservationPlan/purchasedAt"),
                &json!(standing_purchase),
                "case {label:?} via {source}: expected the standing purchase instant \
                 {standing_purchase}, got {}",
                reply.body
            );
            assert_eq!(
                reply.at("/queue/reservationPlan/expiresAt"),
                &json!(standing_expiry),
                "case {label:?} via {source}: expected the standing expiry {standing_expiry}, \
                 got {}",
                reply.body
            );
            assert_eq!(
                reply.at("/queue/reservationPlan/reservedSlots"),
                &json!(2),
                "case {label:?} via {source}: expected the stored slot count to survive, got {}",
                reply.body
            );
        }
    }
}
