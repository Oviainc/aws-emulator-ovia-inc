// SPDX-License-Identifier: CC0-1.0

//! Where a Savings Plan's return window CLOSES, and which states still have one.
//!
//! `returnableUntil` is answered against the instant the request is served, so a
//! suite that computes its expectation from a clock it sampled earlier is
//! asserting how long it took itself to get to the call. The rules here are
//! pinned two ways instead:
//!
//! * the instant the window turns over is asserted by handing the rule the
//!   instant to answer as of ([`the_window_turns_over_one_window_after_the_start`]),
//!   so the millisecond either side of the close is a row and no row depends on
//!   wall-clock timing;
//! * the member a `DescribeSavingsPlans` reports is asserted over the wire
//!   ([`describe_reports_the_window_of_the_state_the_plan_is_in`]) across the
//!   states that decide whether a window exists at all, each far enough from its
//!   own close that the answer cannot change while the suite runs.

use std::collections::BTreeMap;

use serde_json::json;
use service_com_amazonaws_savingsplans_awssavingsplan::state;

use crate::savings_plans_lifecycle::{text, Emulator, ONE_YEAR};

/// A stored plan bought at `purchase_millis` and left in `plan_state`, so a row
/// can ask the window rule about a state without having to reach it over the
/// wire first.
fn stored_plan(
    offering: &state::Offering,
    savings_plan_id: &str,
    purchase_millis: i64,
    plan_state: &str,
) -> state::SavingsPlanRecord {
    let mut record = state::purchase(
        offering,
        savings_plan_id.to_string(),
        // 0.1 an hour, nothing up front, 0.1 an hour recurring, in
        // AMOUNT_SCALE units: the window rule does not read the amounts.
        state::AMOUNT_SCALE / 10,
        0,
        state::AMOUNT_SCALE / 10,
        purchase_millis,
        purchase_millis,
        BTreeMap::new(),
    );
    record.state = plan_state.to_string();
    record
}

/// The return window closes exactly [`state::RETURN_WINDOW_MILLIS`] after the
/// plan's start, for every state that has a window at all: the close is reported
/// up to and including that instant, and the plan's start from the millisecond
/// after it.
///
/// Driven through the stored record rather than the wire, so the instant the
/// window turns over is pinned without a suite racing its own clock. The rows
/// span both halves of the rule: each state a plan can be in (which decides
/// whether it has a window) crossed with where the serving instant falls
/// relative to the close — the start itself, each side of the close, and far
/// past it.
#[test]
fn the_window_turns_over_one_window_after_the_start() {
    let store = state::ComAmazonawsSavingsplansAwssavingsplanState::default();
    let offering = store.offerings()[0].clone();
    // A whole second, since a purchase time is second-granular.
    let start = 1_777_000_000_000_i64;
    let close = start + state::RETURN_WINDOW_MILLIS;

    // (state, whether the state has a return window, whether a return may be
    //  requested from it while the window is open). Every state a plan of this
    //  service reaches, from the members of the model enum `SavingsPlanState`
    //  (`smithy_query(query="shape", shape="SavingsPlanState")`).
    let states: [(&str, bool, bool); 6] = [
        // Not started yet: the window its start will open is still ahead of it,
        // but a return is not how a queued purchase is cancelled.
        (state::STATE_QUEUED, true, false),
        (state::STATE_PAYMENT_PENDING, true, true),
        (state::STATE_ACTIVE, true, true),
        (state::STATE_RETURNED, false, false),
        (state::STATE_QUEUED_DELETED, false, false),
        (state::STATE_RETIRED, false, false),
    ];

    // (case, the instant the answer is computed as of)
    let instants: [(&str, i64); 5] = [
        ("the instant it was bought", start),
        ("a millisecond before the close", close - 1),
        ("the instant of the close", close),
        ("a millisecond after the close", close + 1),
        ("a whole window past the close", close + state::RETURN_WINDOW_MILLIS),
    ];

    for (plan_state, has_window, returnable_from) in states {
        let plan = stored_plan(&offering, "plan-window-bound", start, plan_state);
        assert_eq!(
            plan.return_window_closes_millis(),
            close,
            "case {plan_state:?}: the window must close one window after the start, got {}",
            plan.return_window_closes_millis()
        );

        for (case, now) in instants {
            let window_open = has_window && now <= close;
            let expected = if window_open { close } else { start };
            let reported = plan.returnable_until_millis(now);
            assert_eq!(
                reported, expected,
                "case {plan_state:?} at {case:?}: expected returnableUntil {expected} ms \
                 (start {start} ms, close {close} ms), got {reported} ms"
            );
            assert_eq!(
                plan.is_returnable(now),
                returnable_from && now <= close,
                "case {plan_state:?} at {case:?}: expected a return to be {} there",
                if returnable_from && now <= close {
                    "accepted"
                } else {
                    "refused"
                }
            );
        }
    }
}

/// And that is the member `DescribeSavingsPlans` reports: a plan in a state that
/// has a window reports when the window closes, and one in a state that has none
/// reports its start.
///
/// The rows span the states that decide whether a window exists, reached the way
/// a caller reaches them, and each sits a whole window away from its own close so
/// the answer cannot turn over while the suite runs.
#[tokio::test]
async fn describe_reports_the_window_of_the_state_the_plan_is_in() {
    let emulator = Emulator::new();
    let offering_id = text(
        &emulator.offering("Compute", "No Upfront", ONE_YEAR).await,
        "offeringId",
    );
    let now = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("a clock after 1970")
            .as_secs(),
    )
    .expect("a clock this century");

    // (case, purchaseTime offset in seconds — `None` means "let the service date
    //  it" — the call that drives the plan into the row's state, whether the
    //  state has a return window)
    let cases: [(&str, Option<i64>, Option<&str>, bool); 5] = [
        ("bought now", None, None, true),
        ("queued for tomorrow", Some(86_400), None, true),
        ("returned", None, Some("ReturnSavingsPlan"), false),
        (
            "cancelled before its term began",
            Some(86_400),
            Some("DeleteQueuedSavingsPlan"),
            false,
        ),
        // Past the end of its one-year term, so the clock retired it.
        ("past the end of its term", Some(-2 * 365 * 86_400), None, false),
    ];

    for (case, offset, drive, has_window) in cases {
        let mut request = json!({
            "savingsPlanOfferingId": offering_id,
            "commitment": "0.1",
        });
        if let Some(offset) = offset {
            request["purchaseTime"] = json!(now + offset);
        }
        let savings_plan_id = emulator.purchase(request).await;
        if let Some(operation) = drive {
            let answer = emulator
                .call(operation, json!({ "savingsPlanId": savings_plan_id }))
                .await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected {operation} to be accepted, got {} {:?}",
                answer.status, answer.body
            );
        }

        let plan = emulator.plan(&savings_plan_id).await;
        let start = state::parse_millis(&text(&plan, "start")).unwrap_or_default();
        let reported = state::parse_millis(&text(&plan, "returnableUntil")).unwrap_or_default();
        let expected = if has_window {
            start + state::RETURN_WINDOW_MILLIS
        } else {
            start
        };
        assert_eq!(
            reported, expected,
            "case {case:?}: expected returnableUntil {expected} ms (start {start} ms, window \
             {} ms), got {reported} ms ({:?})",
            state::RETURN_WINDOW_MILLIS,
            text(&plan, "returnableUntil")
        );
    }
}
