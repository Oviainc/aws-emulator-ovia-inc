// SPDX-License-Identifier: CC0-1.0

//! `ReturnSavingsPlan`: the per-payer-account, per-calendar-month allowance a
//! return is bounded by, on top of the plan's own return window.
//!
//! The rule these suites pin: a return request that would take the account past
//! its allowance for the calendar month the call falls in — in the NUMBER of
//! plans returned or in their CUMULATIVE hourly commitment — is refused with the
//! operation's modelled `ServiceQuotaExceededException` (HTTP 402), and the
//! refused plan keeps the state it was returnable from. An allowance is counted
//! per calendar month, so returns recorded in one month do not consume another's.
//!
//! The first two suites go over the wire through the same `MultiServiceRouter`
//! the emulator boots, reusing the harness in [`crate::savings_plans_lifecycle`].
//! The last drives the store directly, because a month boundary cannot be reached
//! by waiting for it.

use std::collections::BTreeMap;

use serde_json::json;
use service_com_amazonaws_savingsplans_awssavingsplan::state;

use crate::savings_plans_lifecycle::{text, Emulator, ONE_YEAR};

/// Parse a `commitment` the way the service does, into `AMOUNT_SCALE` units.
fn units(commitment: &str) -> i128 {
    state::parse_amount("commitment", commitment)
        .unwrap_or_else(|error| panic!("case {commitment:?}: a valid commitment, got {error}"))
}

/// Parse one of this service's rendered instants into epoch millis.
fn millis(instant: &str) -> i64 {
    state::parse_millis(instant)
        .unwrap_or_else(|| panic!("case {instant:?}: a parseable instant, got none"))
}

/// Returns stop at the month's allowance of PLANS: up to
/// [`state::MAX_RETURNS_PER_CALENDAR_MONTH`] are accepted, every further one is
/// a 402 `ServiceQuotaExceededException`, and a refused return leaves the plan
/// exactly where it was — still returnable, so nothing about it was consumed.
///
/// The commitments here are the smallest the service admits, so the cumulative
/// allowance cannot be what refuses a row: the count is.
#[tokio::test]
async fn returns_stop_at_the_monthly_plan_allowance() {
    let emulator = Emulator::new();
    let offering_id = text(
        &emulator.offering("Compute", "No Upfront", ONE_YEAR).await,
        "offeringId",
    );

    let attempts = state::MAX_RETURNS_PER_CALENDAR_MONTH + 3;
    let mut plans: Vec<String> = Vec::with_capacity(attempts);
    for _ in 0..attempts {
        plans.push(
            emulator
                .purchase(json!({ "savingsPlanOfferingId": offering_id, "commitment": "0.001" }))
                .await,
        );
    }

    for (index, savings_plan_id) in plans.iter().enumerate() {
        let case = format!("return {} of {attempts}", index + 1);
        let accepted = index < state::MAX_RETURNS_PER_CALENDAR_MONTH;
        let answer = emulator
            .call(
                "ReturnSavingsPlan",
                json!({ "savingsPlanId": savings_plan_id }),
            )
            .await;
        let plan = emulator.plan(savings_plan_id).await;
        let state_after = text(&plan, "state");
        if accepted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                state_after,
                state::STATE_RETURNED,
                "case {case:?}: an accepted return must leave the plan {:?}, got {state_after:?}",
                state::STATE_RETURNED
            );
        } else {
            assert_eq!(
                (answer.status, answer.code.as_str()),
                (402, "ServiceQuotaExceededException"),
                "case {case:?}: expected a 402 ServiceQuotaExceededException, got {} {:?}",
                answer.status,
                answer.body
            );
            assert!(
                matches!(
                    state_after.as_str(),
                    state::STATE_PAYMENT_PENDING | state::STATE_ACTIVE
                ),
                "case {case:?}: a refused return must leave the plan in the state it was \
                 returnable from, got {state_after:?}"
            );
            assert!(
                text(&plan, "returnableUntil") > text(&plan, "start"),
                "case {case:?}: a refused return must leave the plan's window open, got {plan:?}"
            );
        }
    }
}

/// Returns stop at the month's allowance of COMMITMENT: a return is accepted
/// while the month's returned commitment stays within
/// [`state::MAX_RETURNED_COMMITMENT_UNITS_PER_CALENDAR_MONTH`] and refused with a
/// 402 past it — including a single plan whose own commitment is larger than the
/// whole allowance.
///
/// The rows vary the commitment's magnitude and scale, and each row's expectation
/// is COMPUTED from the rule against the running total (a refused return
/// consumes nothing), so the table pins the rule rather than a sequence of
/// statuses someone wrote down. The row that lands exactly ON the allowance and
/// the row one step past it are both here.
#[tokio::test]
async fn returns_stop_at_the_monthly_commitment_allowance() {
    let emulator = Emulator::new();
    let offering_id = text(
        &emulator.offering("Compute", "No Upfront", ONE_YEAR).await,
        "offeringId",
    );

    // Against an allowance of 1000 an hour: 0.001 + 250 + 0.5 + 600 = 850.501,
    // then 1000000 alone is past it, then 149.499 lands exactly on 1000.000, and
    // the smallest commitment the service admits is then one step too far.
    let commitments = [
        "0.001", "250", "0.5", "600", "1000000", "149.499", "0.001", "0.00123",
    ];

    let mut returned_units: i128 = 0;
    let mut returned_plans: usize = 0;
    for commitment in commitments {
        let savings_plan_id = emulator
            .purchase(json!({ "savingsPlanOfferingId": offering_id, "commitment": commitment }))
            .await;
        let commitment_units = units(commitment);
        // The rule, applied by the test: the return fits only if it keeps both
        // halves of the month's allowance.
        let fits = returned_plans < state::MAX_RETURNS_PER_CALENDAR_MONTH
            && returned_units + commitment_units
                <= state::MAX_RETURNED_COMMITMENT_UNITS_PER_CALENDAR_MONTH;

        let answer = emulator
            .call(
                "ReturnSavingsPlan",
                json!({ "savingsPlanId": savings_plan_id }),
            )
            .await;
        let state_after = text(&emulator.plan(&savings_plan_id).await, "state");
        let case = format!("commitment {commitment} after {returned_units} units returned");
        if fits {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                state_after,
                state::STATE_RETURNED,
                "case {case:?}: an accepted return must leave the plan {:?}, got {state_after:?}",
                state::STATE_RETURNED
            );
            returned_units += commitment_units;
            returned_plans += 1;
        } else {
            assert_eq!(
                (answer.status, answer.code.as_str()),
                (402, "ServiceQuotaExceededException"),
                "case {case:?}: expected a 402 ServiceQuotaExceededException, got {} {:?}",
                answer.status,
                answer.body
            );
            assert_ne!(
                state_after,
                state::STATE_RETURNED,
                "case {case:?}: a refused return must not return the plan, got {state_after:?}"
            );
        }
    }
    assert!(
        returned_plans > 1 && returned_units > 0,
        "the table must accept some returns before the allowance bites, got {returned_plans} \
         returns of {returned_units} units"
    );
}

/// The allowance is counted PER CALENDAR MONTH: a return recorded in one month
/// consumes that month's allowance and no other's, and the boundary between two
/// months is a single instant.
///
/// Driven through the store, because a month boundary cannot be waited out. The
/// rows span a year boundary, the last and first instant of a month, and a month
/// whose plan count is at the allowance.
#[test]
fn the_return_allowance_is_counted_per_calendar_month() {
    let mut store = state::ComAmazonawsSavingsplansAwssavingsplanState::default();
    let offering = store.offerings()[0].clone();

    // (the calendar month the return falls in, the instant it was accepted at,
    //  its commitment)
    let mut rows: Vec<(&str, &str, &str)> = vec![
        ("2025-12", "2025-12-31T23:59:59.999Z", "400"),
        ("2026-01", "2026-01-01T00:00:00.000Z", "500"),
        ("2026-01", "2026-01-15T12:00:00.000Z", "500"),
        ("2026-02", "2026-02-01T00:00:00.000Z", "1"),
        ("2026-02", "2026-02-28T23:59:59.999Z", "2"),
    ];
    // ... and a month taken to the allowance in PLANS rather than commitment.
    for _ in 0..state::MAX_RETURNS_PER_CALENDAR_MONTH {
        rows.push(("2026-03", "2026-03-10T08:00:00.000Z", "0.001"));
    }

    for (index, (month, accepted_at, commitment)) in rows.iter().enumerate() {
        let commitment_units = units(commitment);
        let at = millis(accepted_at);
        assert_eq!(
            state::calendar_month(at),
            *month,
            "case {accepted_at:?}: expected the calendar month {month:?}, got {:?}",
            state::calendar_month(at)
        );
        let mut record = state::purchase(
            &offering,
            format!("plan-{index}"),
            commitment_units,
            0,
            commitment_units,
            at,
            at,
            BTreeMap::new(),
        );
        record.mark_returned(at);
        store.insert_plan(record);
    }

    // What each month holds, computed from the rows themselves.
    for month in ["2025-12", "2026-01", "2026-02", "2026-03", "2026-04"] {
        let expected_count = rows.iter().filter(|(label, ..)| *label == month).count();
        let expected_units: i128 = rows
            .iter()
            .filter(|(label, ..)| *label == month)
            .map(|(.., commitment)| units(commitment))
            .sum();
        // Any instant inside the month reports the same tally.
        for day in ["01", "14", "28"] {
            let at = millis(&format!("{month}-{day}T06:30:00.000Z"));
            assert_eq!(
                store.returns_in_calendar_month(at),
                (expected_count, expected_units),
                "case {month}-{day}: expected {expected_count} returns of {expected_units} units, \
                 got {:?}",
                store.returns_in_calendar_month(at)
            );
        }
    }

    // (case, an instant in the month, the commitment to return, whether the
    //  allowance admits it)
    let smallest = units("0.001");
    let allowance = state::MAX_RETURNED_COMMITMENT_UNITS_PER_CALENDAR_MONTH;
    let cases: [(&str, &str, i128, bool); 7] = [
        // 400 returned: the rest of the allowance fits, one unit more does not.
        ("2025-12 up to the allowance", "2025-12-05T00:00:00.000Z", allowance - units("400"), true),
        ("2025-12 one unit past it", "2025-12-05T00:00:00.000Z", allowance - units("400") + 1, false),
        // 1000 returned: the commitment allowance is spent, however small the
        // next return is.
        ("2026-01 spent", "2026-01-20T00:00:00.000Z", smallest, false),
        // 3 returned in two plans: plenty of room.
        ("2026-02 barely used", "2026-02-10T00:00:00.000Z", units("900"), true),
        ("2026-02 past the allowance", "2026-02-10T00:00:00.000Z", units("998"), false),
        // The plan allowance is spent even though the commitment is not.
        ("2026-03 at the plan allowance", "2026-03-11T00:00:00.000Z", smallest, false),
        // A month with no returns at all admits the largest single commitment.
        ("2026-04 untouched", "2026-04-01T00:00:00.000Z", allowance, true),
    ];

    for (case, at, commitment_units, admitted) in cases {
        let refusal = store.return_allowance_exceeded(commitment_units, millis(at));
        assert_eq!(
            refusal.is_none(),
            admitted,
            "case {case:?}: expected the allowance to {} a return of {commitment_units} units, \
             got {refusal:?}",
            if admitted { "admit" } else { "refuse" }
        );
    }

    // The boundary between two months is one instant, not a range.
    let last = millis("2026-01-31T23:59:59.999Z");
    assert_eq!(
        (
            state::calendar_month(last),
            state::calendar_month(last + 1)
        ),
        ("2026-01".to_string(), "2026-02".to_string()),
        "the month must turn over at the millisecond after its last, got {:?} then {:?}",
        state::calendar_month(last),
        state::calendar_month(last + 1)
    );
}
