// SPDX-License-Identifier: CC0-1.0

//! The wire FORM of a Savings Plan record, the return window it reports, and
//! the states a return is accepted from.
//!
//! Each rule is asserted over a table that spans the class rather than the one
//! purchase that exposed it:
//!
//! * `SavingsPlan.commitment`/`upfrontPaymentAmount`/`recurringPaymentAmount`
//!   target `Amount`, a *string*, so the rendering is the wire value: every
//!   amount comes back at ONE fixed scale ([`state::AMOUNT_WIRE_DIGITS`]
//!   fractional digits), whatever the request spelled;
//! * `start`/`end`/`returnableUntil` target `String` too, so this service
//!   chooses their spelling: ISO-8601 UTC at fixed width, with exactly
//!   [`state::TIME_FRACTION_DIGITS`] fractional digits;
//! * `returnableUntil` is the instant the plan's return window closes
//!   ([`state::RETURN_WINDOW_MILLIS`] after its start) while that window is
//!   open — including for a plan whose term has not begun — and the plan's
//!   start time once it is not;
//! * a return is accepted only from a plan that has STARTED and been paid for:
//!   a `queued` purchase is refused with a 400 `ValidationException`, since it
//!   is cancelled through `DeleteQueuedSavingsPlan` instead.
//!
//! What a tag key may be is pinned beside this suite, in
//! [`crate::tag_key_round_trip`].
//!
//! Everything goes through the same `MultiServiceRouter` the emulator boots,
//! reusing the harness in [`crate::savings_plans_lifecycle`].

use serde_json::json;
use service_com_amazonaws_savingsplans_awssavingsplan::state;

use crate::savings_plans_lifecycle::{text, Emulator, ONE_YEAR, THREE_YEARS};

/// Whether `value` is an `Amount` as this service renders one: a plain decimal
/// with exactly [`state::AMOUNT_WIRE_DIGITS`] fractional digits.
fn is_wire_amount(value: &str) -> bool {
    let digits = |text: &str| !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit());
    match value.split_once('.') {
        Some((whole, fraction)) => {
            digits(whole) && digits(fraction) && fraction.len() == state::AMOUNT_WIRE_DIGITS
        }
        None => false,
    }
}

/// Whether `value` is an instant as this service renders one:
/// `YYYY-MM-DDThh:mm:ss.sssZ`, fixed width, UTC.
fn is_wire_instant(value: &str) -> bool {
    // `1970-01-01T00:00:00` is 19 characters, then the point, the fraction and
    // the zone designator.
    let expected_len = 19 + 1 + state::TIME_FRACTION_DIGITS + 1;
    if value.len() != expected_len {
        return false;
    }
    value.char_indices().all(|(index, character)| match index {
        4 | 7 => character == '-',
        10 => character == 'T',
        13 | 16 => character == ':',
        19 => character == '.',
        index if index + 1 == expected_len => character == 'Z',
        _ => character.is_ascii_digit(),
    })
}

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

/// Every `Amount` a plan reports is rendered at ONE fixed scale, whatever the
/// request spelled and whatever the arithmetic produced.
///
/// The rows differ along what the rule generalises over: the plan type and
/// payment option (which decide WHICH amounts are non-zero), the term, and the
/// spelling of the commitment — including both published bounds of the
/// documented commitment range ("a value between 0.001 and 1 million") and two
/// spellings of one value.
#[tokio::test]
async fn every_amount_is_rendered_at_one_fixed_scale() {
    let emulator = Emulator::new();

    // (case, plan type, payment option, term, commitment as sent, commitment
    //  as rendered)
    let cases: [(&str, &str, &str, i64, &str, &str); 8] = [
        (
            "the smallest commitment the service accepts",
            "Compute",
            "All Upfront",
            ONE_YEAR,
            "0.001",
            "0.00100000",
        ),
        (
            "the largest commitment the service accepts",
            "Compute",
            "All Upfront",
            ONE_YEAR,
            "1000000",
            "1000000.00000000",
        ),
        (
            "a whole-number commitment",
            "Compute",
            "No Upfront",
            ONE_YEAR,
            "2",
            "2.00000000",
        ),
        (
            "one value spelled short",
            "Compute",
            "No Upfront",
            THREE_YEARS,
            "0.1",
            "0.10000000",
        ),
        (
            "the same value spelled with trailing zeros",
            "Compute",
            "No Upfront",
            THREE_YEARS,
            "0.10000",
            "0.10000000",
        ),
        (
            "a commitment at the full five digits a request may specify",
            "Database",
            "Partial Upfront",
            ONE_YEAR,
            "0.12345",
            "0.12345000",
        ),
        (
            "a partial-upfront plan, whose recurring payment is computed",
            "SageMaker",
            "Partial Upfront",
            THREE_YEARS,
            "3",
            "3.00000000",
        ),
        (
            "an EC2 Instance plan",
            "EC2Instance",
            "All Upfront",
            ONE_YEAR,
            "0.5",
            "0.50000000",
        ),
    ];

    for (case, plan_type, payment_option, duration, sent, rendered) in cases {
        let offering_id = text(
            &emulator.offering(plan_type, payment_option, duration).await,
            "offeringId",
        );
        let plan = emulator
            .plan(
                &emulator
                    .purchase(json!({
                        "savingsPlanOfferingId": offering_id,
                        "commitment": sent,
                    }))
                    .await,
            )
            .await;

        assert_eq!(
            text(&plan, "commitment"),
            rendered,
            "case {case:?}: expected commitment {rendered:?}, got {:?}",
            text(&plan, "commitment")
        );
        for member in [
            "commitment",
            "upfrontPaymentAmount",
            "recurringPaymentAmount",
        ] {
            let value = text(&plan, member);
            assert!(
                is_wire_amount(&value),
                "case {case:?}: {member} must be a decimal with exactly {} fractional \
                 digits, got {value:?}",
                state::AMOUNT_WIRE_DIGITS
            );
        }
        // The rendering does not change the arithmetic: the term's total value
        // is paid for exactly once, up front or hour by hour.
        let hours = duration as f64 / 3600.0;
        let hourly: f64 = sent.parse().expect("the case commitment parses");
        let upfront: f64 = text(&plan, "upfrontPaymentAmount")
            .parse()
            .expect("a rendered amount parses");
        let recurring: f64 = text(&plan, "recurringPaymentAmount")
            .parse()
            .expect("a rendered amount parses");
        let paid = upfront + recurring * hours;
        assert!(
            (paid - hourly * hours).abs() <= 1.0,
            "case {case:?}: the plan must be paid for exactly once: expected {} over the \
             term, got {paid}",
            hourly * hours
        );
    }
}

/// `start`, `end` and `returnableUntil` are rendered at fixed width with
/// millisecond precision, for every purchase time — and the width is what keeps
/// the strings sorting in time order.
///
/// The rows differ along where the purchase time falls relative to now, which is
/// what decides the plan's state and therefore which branch renders each member.
#[tokio::test]
async fn every_instant_is_rendered_at_millisecond_precision() {
    let emulator = Emulator::new();
    let offering_id = text(
        &emulator.offering("Compute", "No Upfront", ONE_YEAR).await,
        "offeringId",
    );
    let now = now_seconds();

    // (case, purchaseTime offset in seconds — `None` means "let the service
    //  date it")
    let cases: [(&str, Option<i64>); 6] = [
        ("dated by the service", None),
        ("bought on the second", Some(0)),
        ("bought a minute ago", Some(-60)),
        ("bought a fortnight ago", Some(-14 * 86_400)),
        ("queued for an hour's time", Some(3_600)),
        ("queued for a year's time", Some(365 * 86_400)),
    ];

    for (case, offset) in cases {
        let mut request = json!({
            "savingsPlanOfferingId": offering_id,
            "commitment": "0.1",
        });
        if let Some(offset) = offset {
            request["purchaseTime"] = json!(now + offset);
        }
        let plan = emulator.plan(&emulator.purchase(request).await).await;

        for member in ["start", "end", "returnableUntil"] {
            let value = text(&plan, member);
            assert!(
                is_wire_instant(&value),
                "case {case:?}: {member} must be an ISO-8601 UTC instant with exactly {} \
                 fractional digits, got {value:?}",
                state::TIME_FRACTION_DIGITS
            );
            assert!(
                state::parse_millis(&value).is_some(),
                "case {case:?}: {member} must parse as an instant, got {value:?}"
            );
        }
        // Fixed width means string order and time order agree.
        let (start, end) = (text(&plan, "start"), text(&plan, "end"));
        assert!(
            start < end,
            "case {case:?}: the term must end after it starts, got start {start:?} end {end:?}"
        );
        assert_eq!(
            state::parse_millis(&end).unwrap_or_default()
                - state::parse_millis(&start).unwrap_or_default(),
            ONE_YEAR * 1000,
            "case {case:?}: the term must be the offering's, got start {start:?} end {end:?}"
        );
    }
}

/// `returnableUntil` is the instant the return window closes — the plan's start
/// plus the window AWS documents — while that window is open, and the plan's
/// start time once it is not.
///
/// The rows differ along how far the purchase is from now, which is what decides
/// whether the window is open: a plan whose term has not begun still has the
/// whole window ahead of it, and one bought longer ago than the window has none
/// left. Each bound of the window is a row, plus one past it.
#[tokio::test]
async fn returnable_until_reports_the_window_close_while_it_is_open() {
    let emulator = Emulator::new();
    let offering_id = text(
        &emulator
            .offering("Compute", "All Upfront", THREE_YEARS)
            .await,
        "offeringId",
    );
    let now = now_seconds();
    let window_seconds = state::RETURN_WINDOW_MILLIS / 1000;

    // (case, purchaseTime offset in seconds, whether the window is still open)
    let cases: [(&str, Option<i64>, bool); 7] = [
        ("dated by the service", None, true),
        ("bought a minute ago", Some(-60), true),
        ("bought a day ago", Some(-86_400), true),
        ("one second inside the window", Some(1 - window_seconds), true),
        ("one second past the window", Some(-1 - window_seconds), false),
        ("queued for tomorrow", Some(86_400), true),
        ("queued for a year's time", Some(365 * 86_400), true),
    ];

    for (case, offset, window_open) in cases {
        let mut request = json!({
            "savingsPlanOfferingId": offering_id,
            "commitment": "0.001",
        });
        if let Some(offset) = offset {
            request["purchaseTime"] = json!(now + offset);
        }
        let plan = emulator.plan(&emulator.purchase(request).await).await;
        let start = state::parse_millis(&text(&plan, "start")).unwrap_or_default();
        let returnable_until =
            state::parse_millis(&text(&plan, "returnableUntil")).unwrap_or_default();
        let expected = if window_open {
            start + state::RETURN_WINDOW_MILLIS
        } else {
            start
        };
        assert_eq!(
            returnable_until, expected,
            "case {case:?}: expected returnableUntil {expected} ms (start {start} ms, window \
             {} ms), got {returnable_until} ms ({:?})",
            state::RETURN_WINDOW_MILLIS,
            text(&plan, "returnableUntil")
        );
    }

    // And a plan that HAS been returned has no window left, whatever its start.
    let savings_plan_id = emulator
        .purchase(json!({
            "savingsPlanOfferingId": offering_id,
            "commitment": "0.001",
        }))
        .await;
    let returned = emulator
        .call(
            "ReturnSavingsPlan",
            json!({ "savingsPlanId": savings_plan_id }),
        )
        .await;
    assert_eq!(
        returned.status, 200,
        "case a returnable plan: expected 200, got {} {:?}",
        returned.status, returned.body
    );
    let plan = emulator.plan(&savings_plan_id).await;
    assert_eq!(
        text(&plan, "returnableUntil"),
        text(&plan, "start"),
        "case a returned plan: expected returnableUntil to fall back to start, got {plan:?}"
    );
}

/// A return is accepted only from a plan that has started and been paid for and
/// is still inside its window. Every other state is refused with a 400
/// `ValidationException`, and the refusal leaves the plan exactly where it was.
///
/// The rows span the states a plan can be in when the return arrives, which is
/// what the rule generalises over.
#[tokio::test]
async fn a_return_is_refused_from_every_state_it_is_not_accepted_from() {
    let emulator = Emulator::new();
    let offering_id = text(
        &emulator.offering("Compute", "No Upfront", ONE_YEAR).await,
        "offeringId",
    );
    let now = now_seconds();
    let window_seconds = state::RETURN_WINDOW_MILLIS / 1000;

    // (case, purchaseTime offset in seconds, the state the plan is in when the
    //  return arrives, whether the return is accepted)
    let cases: [(&str, Option<i64>, &str, bool); 5] = [
        (
            "dated by the service",
            None,
            state::STATE_PAYMENT_PENDING,
            true,
        ),
        (
            "inside the window",
            Some(1 - window_seconds),
            state::STATE_PAYMENT_PENDING,
            true,
        ),
        (
            "past the window",
            Some(-1 - window_seconds),
            state::STATE_PAYMENT_PENDING,
            false,
        ),
        (
            "past the end of its term",
            Some(-2 * 365 * 86_400),
            state::STATE_RETIRED,
            false,
        ),
        // Not started yet: cancelled through DeleteQueuedSavingsPlan, never
        // returned.
        (
            "queued for tomorrow",
            Some(86_400),
            state::STATE_QUEUED,
            false,
        ),
    ];

    for (case, offset, state_before, accepted) in cases {
        let mut request = json!({
            "savingsPlanOfferingId": offering_id,
            "commitment": "0.1",
        });
        if let Some(offset) = offset {
            request["purchaseTime"] = json!(now + offset);
        }
        let savings_plan_id = emulator.purchase(request).await;
        let observed_before = text(&emulator.plan(&savings_plan_id).await, "state");
        assert_eq!(
            observed_before, state_before,
            "case {case:?}: expected the plan to be {state_before:?} before the return, got \
             {observed_before:?}"
        );

        let answer = emulator
            .call(
                "ReturnSavingsPlan",
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
                state::STATE_RETURNED,
                "case {case:?}: an accepted return must move the plan to {:?}, got \
                 {state_after:?}",
                state::STATE_RETURNED
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
                message.contains(state_before) || message.contains("return window"),
                "case {case:?}: the refusal must say why, got {message:?}"
            );
            // A refused return changes nothing: the plan is where it was.
            assert_eq!(
                state_after, state_before,
                "case {case:?}: a refused return must leave the plan {state_before:?}, got \
                 {state_after:?}"
            );
        }
    }
}
