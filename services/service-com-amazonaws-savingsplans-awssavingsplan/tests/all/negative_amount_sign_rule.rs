// SPDX-License-Identifier: CC0-1.0

//! A leading `-` never decides whether a value is an `Amount`.
//!
//! Both `Amount` members of `CreateSavingsPlan` are documented with a MAGNITUDE rule
//! — `commitment` is "a value between 0.001 and 1 million" and `upfrontPaymentAmount`
//! is "a whole number between 50 and 99 percent of the total value of the Savings
//! Plan"
//! (`smithy_query(query="doc",shape="CreateSavingsPlanRequest",member="commitment")`
//! and the same for `upfrontPaymentAmount`) — so a signed decimal number is read as
//! the number it spells and refused by that rule, which is the only place the bound
//! or window it missed is reported back. Only a value that is not a plain decimal
//! number, or one spelled at more fractional digits than a request may specify, is
//! refused on its form.
//!
//! The rows below span the member, the magnitude, the integer width (from one digit
//! to past what a 128-bit scaled integer holds), the presence of a fractional part
//! and the spelling (redundant leading zeros, a signed zero), and each asserts the
//! clause the value's own member names rather than the form clause.
//!
//! Everything goes through the same `MultiServiceRouter` the emulator boots, reusing
//! the harness in [`crate::savings_plans_lifecycle`].

use serde_json::json;

use crate::amount_magnitude_refusals::{plan_count, wide, MIN_COMMITMENT};
use crate::savings_plans_lifecycle::{text, Emulator, ONE_YEAR};

/// The opening of the window clause `upfrontPaymentAmount`'s documented rule answers
/// with, quoting that rule's own sentence.
const WINDOW_OPENING: &str = "upfrontPaymentAmount must be a whole number between 50 and 99 \
                              percent of the total value of the Savings Plan (";

/// The clause a value refused on its FORM carries. No row below may earn it: every
/// row is a plain decimal number.
const FORM_CLAUSE: &str = "decimal amount";

/// Which documented rule must refuse the row, and therefore which clause the caller
/// must be given.
#[derive(Debug, Clone, Copy)]
enum Rule {
    /// `commitment`'s interval: a number below it names the floor.
    CommitmentFloor,
    /// `upfrontPaymentAmount`'s window: a whole number outside it names the window.
    UpfrontWindow,
    /// The whole-number half of the same sentence, for a value carrying a fraction.
    UpfrontWholeNumber,
}

impl Rule {
    /// The member the row sends its value in.
    fn member(self) -> &'static str {
        match self {
            Rule::CommitmentFloor => "commitment",
            Rule::UpfrontWindow | Rule::UpfrontWholeNumber => "upfrontPaymentAmount",
        }
    }
}

/// A negative amount is a number on EVERY `Amount` member: it is refused by the rule
/// governing that member's magnitude, naming the bound or window it missed, and never
/// reported as not being an amount at all.
#[tokio::test]
async fn a_negative_amount_is_refused_by_its_member_s_magnitude_rule() {
    let emulator = Emulator::new();
    // Partial Upfront, the one payment option that takes an up-front amount, so one
    // offering serves both members. A one-year term at 0.1 an hour is a total value
    // of 876, so the documented window is 438 (50%) to 867 (99%, floored to a whole
    // number).
    let offering_id = text(
        &emulator
            .offering("Compute", "Partial Upfront", ONE_YEAR)
            .await,
        "offeringId",
    );
    let before = plan_count(&emulator).await;

    // (case, the value as sent, the rule that must refuse it)
    let rows: [(&str, String, Rule); 14] = [
        ("a one-digit negative commitment", "-1".to_string(), Rule::CommitmentFloor),
        (
            "a negative commitment one step under the floor",
            format!("-{MIN_COMMITMENT}"),
            Rule::CommitmentFloor,
        ),
        ("a signed zero commitment", "-0".to_string(), Rule::CommitmentFloor),
        (
            "a negative commitment with a fractional part",
            "-7.5".to_string(),
            Rule::CommitmentFloor,
        ),
        (
            "a negative commitment behind twenty leading zeros",
            format!("-{}1", "0".repeat(20)),
            Rule::CommitmentFloor,
        ),
        (
            "a forty-digit negative commitment",
            format!("-{}", wide("4", 39)),
            Rule::CommitmentFloor,
        ),
        (
            "a hundred-and-twenty-digit negative commitment",
            format!("-{}", wide("5", 119)),
            Rule::CommitmentFloor,
        ),
        (
            "a one-digit negative up-front payment",
            "-1".to_string(),
            Rule::UpfrontWindow,
        ),
        (
            "a negative up-front payment the size of the window's floor",
            "-438".to_string(),
            Rule::UpfrontWindow,
        ),
        ("a signed zero up-front payment", "-0".to_string(), Rule::UpfrontWindow),
        (
            "a negative whole up-front payment behind ten leading zeros",
            format!("-{}500", "0".repeat(10)),
            Rule::UpfrontWindow,
        ),
        (
            "a thirty-two digit negative up-front payment",
            format!("-{}", wide("1", 31)),
            Rule::UpfrontWindow,
        ),
        (
            "a negative up-front payment with a fractional part",
            "-500.5".to_string(),
            Rule::UpfrontWholeNumber,
        ),
        (
            "a negative up-front payment at the widest fraction a request may specify",
            "-0.12345".to_string(),
            Rule::UpfrontWholeNumber,
        ),
    ];

    for (case, sent, rule) in rows {
        let mut request = json!({
            "savingsPlanOfferingId": offering_id,
            "commitment": "0.1",
        });
        request[rule.member()] = json!(sent);
        let answer = emulator.call("CreateSavingsPlan", request).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?} ({} {sent:?}): expected 400, got {} {:?}",
            rule.member(),
            answer.status,
            answer.body
        );
        assert_eq!(
            answer.code, "ValidationException",
            "case {case:?} ({} {sent:?}): expected ValidationException, got {:?}",
            rule.member(),
            answer.code
        );
        let message = text(&answer.body, "message");
        assert!(
            !message.contains(FORM_CLAUSE),
            "case {case:?} ({} {sent:?}): a plain decimal number must not be refused on its \
             form, got {message:?}",
            rule.member()
        );
        match rule {
            Rule::CommitmentFloor => {
                let expected = format!("'commitment' cannot be less than {MIN_COMMITMENT}");
                assert_eq!(
                    message, expected,
                    "case {case:?} (commitment {sent:?}): expected {expected:?}, got {message:?}"
                );
            }
            Rule::UpfrontWindow => {
                assert!(
                    message.starts_with(WINDOW_OPENING),
                    "case {case:?} (upfrontPaymentAmount {sent:?}): expected the window clause \
                     {WINDOW_OPENING:?}, got {message:?}"
                );
                assert!(
                    message.ends_with(&format!("got '{sent}'")),
                    "case {case:?} (upfrontPaymentAmount {sent:?}): expected the clause to \
                     report the submitted value, got {message:?}"
                );
            }
            Rule::UpfrontWholeNumber => {
                let expected = format!("upfrontPaymentAmount must be a whole number, got '{sent}'");
                assert_eq!(
                    message, expected,
                    "case {case:?} (upfrontPaymentAmount {sent:?}): expected {expected:?}, got \
                     {message:?}"
                );
            }
        }
    }

    assert_eq!(
        plan_count(&emulator).await,
        before,
        "a refused purchase must create nothing"
    );
}

/// The window clause reports the window the service computed, so the bounds the
/// negative rows above missed are the documented percentages of the plan's total
/// value — and a value INSIDE that window is still purchased, reading back at the
/// amount it names.
#[tokio::test]
async fn the_window_the_refusal_names_is_the_window_a_purchase_is_admitted_by() {
    let emulator = Emulator::new();
    let offering_id = text(
        &emulator
            .offering("Compute", "Partial Upfront", ONE_YEAR)
            .await,
        "offeringId",
    );

    // Read the window out of the refusal a negative value earns, rather than writing
    // the two numbers down: the clause is the only place the service reports them.
    let answer = emulator
        .call(
            "CreateSavingsPlan",
            json!({
                "savingsPlanOfferingId": offering_id,
                "commitment": "0.1",
                "upfrontPaymentAmount": "-438",
            }),
        )
        .await;
    let message = text(&answer.body, "message");
    assert_eq!(
        (answer.status, answer.code.as_str()),
        (400, "ValidationException"),
        "a negative up-front payment: expected a 400 ValidationException, got {} {:?}",
        answer.status,
        answer.body
    );
    let window = message
        .strip_prefix(WINDOW_OPENING)
        .and_then(|rest| rest.split_once(')'))
        .map(|(window, _)| window.to_string())
        .unwrap_or_default();
    let (low, high) = window
        .split_once(" to ")
        .unwrap_or_else(|| panic!("expected a window of the form 'low to high', got {message:?}"));
    let low_value: f64 = low.parse().unwrap_or(f64::NAN);
    let high_value: f64 = high.parse().unwrap_or(f64::NAN);
    // A one-year term at 0.1 an hour is a total value of 876 (8760 hours), and the
    // documented share is 50 to 99 percent of it.
    assert!(
        (low_value - 438.0).abs() < 1e-6 && (high_value - 867.0).abs() < 1e-6,
        "expected the window to be 50 to 99 percent of 876 (438 to 867), got {low:?} to {high:?}"
    );

    // Round trip: the window the refusal names admits a purchase at its own floor.
    // The bound is quoted at the service's rendering scale, which is wider than the
    // fractional digits a request may specify, so the purchase names the whole
    // number it is.
    let floor = low.split_once('.').map_or(low, |(whole, _)| whole);
    let savings_plan_id = emulator
        .purchase(json!({
            "savingsPlanOfferingId": offering_id,
            "commitment": "0.1",
            "upfrontPaymentAmount": floor,
        }))
        .await;
    let plan = emulator.plan(&savings_plan_id).await;
    let reported = text(&plan, "upfrontPaymentAmount");
    assert_eq!(
        reported, low,
        "expected the plan to report the up-front payment {low:?}, got {reported:?}"
    );
}
