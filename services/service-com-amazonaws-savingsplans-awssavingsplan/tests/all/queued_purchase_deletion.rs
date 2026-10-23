// SPDX-License-Identifier: CC0-1.0

//! `DeleteQueuedSavingsPlan`: cancelling a purchase whose term has not begun.
//!
//! The rules these suites pin:
//!
//! * a deletion is accepted ONLY from a plan whose term has not begun
//!   (`queued`): it moves to `queued-deleted`, which a later
//!   `DescribeSavingsPlans` reports, and the cancelled purchase can no longer be
//!   returned;
//! * a plan in any other state has no queued purchase to delete, so the call is
//!   refused with a 400 `ValidationException` and the plan stays exactly where
//!   it was;
//! * an id that names no plan of this account is a 404
//!   `ResourceNotFoundException`, whatever the id looks like.
//!
//! Everything goes through the same `MultiServiceRouter` the emulator boots,
//! reusing the harness in [`crate::savings_plans_lifecycle`].

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_savingsplans_awssavingsplan::state;

use crate::savings_plans_lifecycle::{text, Emulator, ONE_YEAR};

/// Now, in whole seconds since the epoch: what a `purchaseTime` carries.
fn now_seconds() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("a clock after 1970")
            .as_secs(),
    )
    .expect("a clock this century")
}

/// How a plan is brought into the state a row needs before the deletion arrives.
#[derive(Debug, Clone, Copy)]
enum Setup {
    /// Purchase dated this many seconds from now, and nothing else.
    Purchased(i64),
    /// Purchase dated now, then wait out the activation the clock owes it.
    PurchasedAndSettled,
    /// Purchase dated now, then returned.
    Returned,
    /// Purchase queued for the future, then already cancelled once.
    AlreadyCancelled,
}

/// Buy a plan and drive it into the state `setup` describes, answering with its
/// id.
async fn plan_in_state(emulator: &Emulator, offering_id: &str, setup: Setup) -> String {
    let mut request = json!({
        "savingsPlanOfferingId": offering_id,
        "commitment": "0.1",
    });
    let offset = match setup {
        Setup::Purchased(offset) => Some(offset),
        Setup::AlreadyCancelled => Some(86_400),
        Setup::PurchasedAndSettled | Setup::Returned => None,
    };
    if let Some(offset) = offset {
        request["purchaseTime"] = json!(now_seconds() + offset);
    }
    let savings_plan_id = emulator.purchase(request).await;

    match setup {
        Setup::Purchased(_) => {}
        Setup::PurchasedAndSettled => {
            // The move out of `payment-pending` is a transition the clock owes,
            // not one a caller triggers: wait it out rather than poking state.
            let settle = u64::try_from(state::ACTIVATION_DELAY_MILLIS).unwrap_or_default();
            std::thread::sleep(Duration::from_millis(settle + 100));
        }
        Setup::Returned => {
            let answer = emulator
                .call(
                    "ReturnSavingsPlan",
                    json!({ "savingsPlanId": savings_plan_id }),
                )
                .await;
            assert_eq!(
                answer.status, 200,
                "setup {setup:?}: expected the return to be accepted, got {} {:?}",
                answer.status, answer.body
            );
        }
        Setup::AlreadyCancelled => {
            let answer = emulator
                .call(
                    "DeleteQueuedSavingsPlan",
                    json!({ "savingsPlanId": savings_plan_id }),
                )
                .await;
            assert_eq!(
                answer.status, 200,
                "setup {setup:?}: expected the first cancellation to be accepted, got {} {:?}",
                answer.status, answer.body
            );
        }
    }
    savings_plan_id
}

/// A queued purchase is cancelled, and a purchase in any other state is refused
/// with a 400 `ValidationException` that leaves the plan where it was.
///
/// The rows span the states a plan can be in when the deletion arrives — which
/// is what the rule generalises over — including the plan whose purchase was
/// already cancelled, so a replayed deletion is not read as a fresh one.
#[tokio::test]
async fn a_deletion_is_accepted_only_from_a_purchase_whose_term_has_not_begun() {
    let emulator = Emulator::new();
    let offering_id = text(
        &emulator.offering("Compute", "No Upfront", ONE_YEAR).await,
        "offeringId",
    );

    // (case, how to reach the state, the state the plan is in when the deletion
    //  arrives, whether the deletion is accepted)
    let cases: [(&str, Setup, &str, bool); 6] = [
        (
            "queued for tomorrow",
            Setup::Purchased(86_400),
            state::STATE_QUEUED,
            true,
        ),
        (
            "queued for a year's time",
            Setup::Purchased(365 * 86_400),
            state::STATE_QUEUED,
            true,
        ),
        (
            "bought now, payment not settled",
            Setup::Purchased(0),
            state::STATE_PAYMENT_PENDING,
            false,
        ),
        (
            "bought now, payment settled",
            Setup::PurchasedAndSettled,
            state::STATE_ACTIVE,
            false,
        ),
        (
            "past the end of its term",
            Setup::Purchased(-2 * 365 * 86_400),
            state::STATE_RETIRED,
            false,
        ),
        (
            "already returned",
            Setup::Returned,
            state::STATE_RETURNED,
            false,
        ),
    ];

    for (case, setup, state_before, accepted) in cases {
        let savings_plan_id = plan_in_state(&emulator, &offering_id, setup).await;
        let observed_before = text(&emulator.plan(&savings_plan_id).await, "state");
        assert_eq!(
            observed_before, state_before,
            "case {case:?}: expected the plan to be {state_before:?} before the deletion, got \
             {observed_before:?}"
        );

        let answer = emulator
            .call(
                "DeleteQueuedSavingsPlan",
                json!({ "savingsPlanId": savings_plan_id }),
            )
            .await;
        let state_after = text(&emulator.plan(&savings_plan_id).await, "state");
        if accepted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                state_after,
                state::STATE_QUEUED_DELETED,
                "case {case:?}: an accepted deletion must move the plan to {:?}, got \
                 {state_after:?}",
                state::STATE_QUEUED_DELETED
            );
        } else {
            assert_eq!(
                (answer.status, answer.code.as_str()),
                (400, "ValidationException"),
                "case {case:?}: expected a 400 ValidationException, got {} {:?}",
                answer.status,
                answer.body
            );
            let message = text(&answer.body, "message");
            assert!(
                message.contains(state_before),
                "case {case:?}: the refusal must name the state it refused ({state_before:?}), \
                 got {message:?}"
            );
            assert_eq!(
                state_after, state_before,
                "case {case:?}: a refused deletion must leave the plan {state_before:?}, got \
                 {state_after:?}"
            );
        }
    }

    // The same rule covers the plan whose purchase was ALREADY cancelled: it is
    // no longer queued, so a replayed deletion is refused rather than reported
    // as a fresh cancellation.
    let cancelled = plan_in_state(&emulator, &offering_id, Setup::AlreadyCancelled).await;
    let answer = emulator
        .call(
            "DeleteQueuedSavingsPlan",
            json!({ "savingsPlanId": cancelled }),
        )
        .await;
    assert_eq!(
        (answer.status, answer.code.as_str()),
        (400, "ValidationException"),
        "case a replayed deletion: expected a 400 ValidationException, got {} {:?}",
        answer.status,
        answer.body
    );
    let state_after = text(&emulator.plan(&cancelled).await, "state");
    assert_eq!(
        state_after,
        state::STATE_QUEUED_DELETED,
        "case a replayed deletion: the plan must stay {:?}, got {state_after:?}",
        state::STATE_QUEUED_DELETED
    );
}

/// An id that names no plan of this account is a 404
/// `ResourceNotFoundException`, and nothing stored changes.
///
/// The rows differ along the SHAPE of the id — another account's, a near miss on
/// one of ours, a different casing, an ARN where an id belongs, a long opaque
/// value — because none of that is what makes the answer a 404: not being in the
/// store is.
#[tokio::test]
async fn an_id_that_names_no_plan_is_not_found() {
    let emulator = Emulator::new();
    let offering_id = text(
        &emulator.offering("Compute", "All Upfront", ONE_YEAR).await,
        "offeringId",
    );
    let mine = plan_in_state(&emulator, &offering_id, Setup::Purchased(86_400)).await;
    let arn = text(&emulator.plan(&mine).await, "savingsPlanArn");

    // (case, the id to delete)
    let cases: [(&str, String); 5] = [
        (
            "an id minted by another account",
            "11111111-2222-3333-4444-555555555555".to_string(),
        ),
        (
            "a near miss on one of ours",
            format!("{mine}-not-mine"),
        ),
        ("the same id upper-cased", mine.to_uppercase()),
        ("an ARN where an id belongs", arn.clone()),
        ("a long opaque value", "z".repeat(64)),
    ];

    for (case, savings_plan_id) in cases {
        // A row only pins the 404 for an id this store really does not hold.
        assert_ne!(
            savings_plan_id, mine,
            "case {case:?}: the row must name an id this store does not hold"
        );
        let answer = emulator
            .call(
                "DeleteQueuedSavingsPlan",
                json!({ "savingsPlanId": savings_plan_id }),
            )
            .await;
        assert_eq!(
            (answer.status, answer.code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected a 404 ResourceNotFoundException for {savings_plan_id:?}, \
             got {} {:?}",
            answer.status,
            answer.body
        );
    }

    // None of those refusals touched the plan that IS in the store.
    let state_after = text(&emulator.plan(&mine).await, "state");
    assert_eq!(
        state_after,
        state::STATE_QUEUED,
        "a refused deletion must leave the stored plan {:?}, got {state_after:?}",
        state::STATE_QUEUED
    );
}

/// A cancelled purchase round-trips: `DescribeSavingsPlans` reports it as
/// `queued-deleted`, a `states` filter finds it under that state and not under
/// its old one, it keeps no return window, and it can no longer be returned.
#[tokio::test]
async fn a_cancelled_purchase_round_trips_through_describe() {
    let emulator = Emulator::new();
    let offering_id = text(
        &emulator.offering("EC2Instance", "All Upfront", ONE_YEAR).await,
        "offeringId",
    );
    let savings_plan_id = plan_in_state(&emulator, &offering_id, Setup::Purchased(7 * 86_400)).await;

    let answer = emulator
        .call(
            "DeleteQueuedSavingsPlan",
            json!({ "savingsPlanId": savings_plan_id }),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "expected the cancellation to be accepted, got {} {:?}",
        answer.status, answer.body
    );
    // The response models no members, so the answer is an empty document.
    assert_eq!(
        answer.body,
        json!({}),
        "DeleteQueuedSavingsPlan models no output members, got {:?}",
        answer.body
    );

    let plan = emulator.plan(&savings_plan_id).await;
    assert_eq!(
        text(&plan, "state"),
        state::STATE_QUEUED_DELETED,
        "expected the plan to read back as {:?}, got {plan:?}",
        state::STATE_QUEUED_DELETED
    );
    // Nothing else about the purchase changed: the term is still the one bought.
    assert_eq!(
        plan.get("termDurationInSeconds").and_then(Value::as_i64),
        Some(ONE_YEAR),
        "a cancellation must not rewrite the term, got {plan:?}"
    );
    // A cancelled purchase has no return window left, so `returnableUntil`
    // falls back to the plan's start.
    assert_eq!(
        text(&plan, "returnableUntil"),
        text(&plan, "start"),
        "a cancelled purchase must report no open return window, got {plan:?}"
    );

    for (case, states, expected) in [
        ("its new state", json!([state::STATE_QUEUED_DELETED]), 1),
        ("the state it left", json!([state::STATE_QUEUED]), 0),
    ] {
        let found = emulator
            .call(
                "DescribeSavingsPlans",
                json!({ "savingsPlanIds": [savings_plan_id], "states": states }),
            )
            .await;
        assert_eq!(
            found.list("savingsPlans").len(),
            expected,
            "case {case:?}: expected {expected} plan(s) under {states}, got {:?}",
            found.body
        );
    }

    let returned = emulator
        .call(
            "ReturnSavingsPlan",
            json!({ "savingsPlanId": savings_plan_id }),
        )
        .await;
    assert_eq!(
        (returned.status, returned.code.as_str()),
        (400, "ValidationException"),
        "a cancelled purchase cannot be returned: expected a 400 ValidationException, got {} {:?}",
        returned.status,
        returned.body
    );
}
