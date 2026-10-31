// SPDX-License-Identifier: CC0-1.0

//! How WIDE an `Amount` is spelled never decides whether it is an amount.
//!
//! `commitment` and `upfrontPaymentAmount` both target `Amount`, a plain string the
//! model constrains in prose only
//! (`smithy_query(query="shape",shape="CreateSavingsPlanRequest")`), so the service's
//! own reading of the value is the whole rule. That reading splits in two, and the two
//! halves must stay split however many digits a value carries:
//!
//! * FORM — a value that is not a plain decimal number (letters, exponent notation, two
//!   decimal points, an empty value), or one spelled at more fractional digits than a
//!   request may specify, is refused on its form;
//! * MAGNITUDE — a value that IS a plain decimal number is refused only by the rule
//!   governing its member's magnitude, naming the bound or window it missed. A number
//!   too wide for any fixed-width arithmetic the service happens to read it with is
//!   still a plain decimal number, so it belongs to this half, not the one above.
//!
//! The rows below therefore span integer WIDTH — from values that fit a 128-bit scaled
//! integer to values hundreds of digits wide — and assert the refusal a value gets is
//! the one its magnitude earns, in both directions (ceiling and floor) and on both
//! members. The inside of the rule is asserted too: a value spelled with redundant
//! leading zeros is as wide as an over-ceiling value and is simply purchased.
//!
//! Everything goes through the same `MultiServiceRouter` the emulator boots, reusing
//! the harness in [`crate::savings_plans_lifecycle`].

use serde_json::json;

use crate::savings_plans_lifecycle::{text, Emulator, ONE_YEAR};

/// The documented maximum hourly commitment ("1 million"), as the service quotes it.
const MAX_COMMITMENT: &str = "1000000";

/// The documented minimum hourly commitment ("0.001"), as the service quotes it.
///
/// `pub(crate)` so the sibling suites of this test target share the one spelling of
/// each documented bound rather than each writing it down again.
pub(crate) const MIN_COMMITMENT: &str = "0.001";

/// `lead` followed by `zeros` zeros — a plain decimal integer of a chosen width.
pub(crate) fn wide(lead: &str, zeros: usize) -> String {
    format!("{lead}{}", "0".repeat(zeros))
}

/// How many Savings Plans the account holds, so a refusal can be shown to buy nothing.
pub(crate) async fn plan_count(emulator: &Emulator) -> usize {
    let answer = emulator.call("DescribeSavingsPlans", json!({})).await;
    assert_eq!(
        answer.status, 200,
        "describing every plan: expected 200, got {} {:?}",
        answer.status, answer.body
    );
    answer.list("savingsPlans").len()
}

/// A `commitment` above the documented ceiling is refused with the CEILING, at every
/// width it can be spelled at — including widths past what a 128-bit scaled integer
/// holds, where the service cannot represent the value exactly and must still classify
/// it by its magnitude.
#[tokio::test]
async fn a_commitment_above_the_ceiling_names_the_ceiling_at_every_width() {
    let emulator = Emulator::new();
    let offering_id = text(
        &emulator.offering("Compute", "No Upfront", ONE_YEAR).await,
        "offeringId",
    );
    let before = plan_count(&emulator).await;
    let expected = format!("'commitment' cannot be greater than {MAX_COMMITMENT}");

    // (case, the commitment as sent). The widths straddle the point where the value
    // stops fitting the scaled 128-bit integer the service reads amounts into (around
    // 31 integer digits), so the rows on either side of it must answer alike.
    let rows: [(&str, String); 8] = [
        ("one integer digit past the ceiling", wide("1", 7)),
        ("twenty integer digits", wide("1", 19)),
        ("thirty integer digits", wide("1", 29)),
        ("thirty-one integer digits", wide("2", 30)),
        ("thirty-two integer digits", wide("1", 31)),
        ("forty integer digits", wide("9", 39)),
        ("a hundred and twenty integer digits", wide("5", 119)),
        (
            "a wide value with a fractional part a request may specify",
            format!("{}.12345", "9".repeat(35)),
        ),
    ];

    for (case, commitment) in rows {
        let answer = emulator
            .call(
                "CreateSavingsPlan",
                json!({ "savingsPlanOfferingId": offering_id, "commitment": commitment }),
            )
            .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?} (commitment {commitment:?}): expected 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code, "ValidationException",
            "case {case:?} (commitment {commitment:?}): expected ValidationException, got {:?}",
            answer.code
        );
        let message = text(&answer.body, "message");
        assert_eq!(
            &message, &expected,
            "case {case:?} (commitment {commitment:?}): expected {expected:?}, got {message:?}"
        );
    }

    assert_eq!(
        plan_count(&emulator).await,
        before,
        "a refused purchase must create nothing"
    );
}

/// The same rule under the FLOOR: a negative amount of any width is a number below the
/// minimum, so it names the floor rather than being called malformed.
#[tokio::test]
async fn a_wide_negative_commitment_names_the_floor() {
    let emulator = Emulator::new();
    let offering_id = text(
        &emulator.offering("Compute", "No Upfront", ONE_YEAR).await,
        "offeringId",
    );
    let expected = format!("'commitment' cannot be less than {MIN_COMMITMENT}");

    // (case, the commitment as sent)
    let rows: [(&str, String); 4] = [
        ("a small negative amount", "-1".to_string()),
        ("thirty negative integer digits", format!("-{}", wide("1", 29))),
        ("forty negative integer digits", format!("-{}", wide("4", 39))),
        (
            "a wide negative amount with a fractional part",
            format!("-{}.5", "7".repeat(40)),
        ),
    ];

    for (case, commitment) in rows {
        let answer = emulator
            .call(
                "CreateSavingsPlan",
                json!({ "savingsPlanOfferingId": offering_id, "commitment": commitment }),
            )
            .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?} (commitment {commitment:?}): expected 400, got {} {:?}",
            answer.status, answer.body
        );
        let message = text(&answer.body, "message");
        assert_eq!(
            &message, &expected,
            "case {case:?} (commitment {commitment:?}): expected {expected:?}, got {message:?}"
        );
    }
}

/// The FORM half of the rule is untouched by width: a wide value that is not a plain
/// decimal number, or one carrying more fractional digits than a request may specify,
/// is still refused on its form and not with a bound.
#[tokio::test]
async fn a_wide_value_that_is_not_a_decimal_number_is_still_refused_on_its_form() {
    let emulator = Emulator::new();
    let offering_id = text(
        &emulator.offering("Compute", "No Upfront", ONE_YEAR).await,
        "offeringId",
    );
    let form = "commitment must be a decimal amount, got";
    let digits = "commitment cannot specify more than 5 digits after the decimal point, got";

    // (case, the commitment as sent, the clause the refusal must open with)
    let rows: [(&str, String, &str); 6] = [
        ("letters", "abc".to_string(), form),
        (
            "a wide value with a trailing letter",
            format!("{}x", "9".repeat(40)),
            form,
        ),
        (
            "a wide value in exponent notation",
            "1e40".to_string(),
            form,
        ),
        (
            "a wide value with two decimal points",
            format!("{}.1.2", "9".repeat(40)),
            form,
        ),
        (
            "a wide value with six fractional digits",
            format!("{}.123456", "9".repeat(40)),
            digits,
        ),
        (
            "a value at the ceiling with six fractional digits",
            format!("{MAX_COMMITMENT}.000000"),
            digits,
        ),
    ];

    for (case, commitment, expected) in rows {
        let answer = emulator
            .call(
                "CreateSavingsPlan",
                json!({ "savingsPlanOfferingId": offering_id, "commitment": commitment }),
            )
            .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?} (commitment {commitment:?}): expected 400, got {} {:?}",
            answer.status, answer.body
        );
        let message = text(&answer.body, "message");
        let clause = format!("{expected} '{commitment}'");
        assert_eq!(
            message, clause,
            "case {case:?} (commitment {commitment:?}): expected {clause:?}, got {message:?}"
        );
    }
}

/// Width is never the refusal, from the INSIDE: a commitment inside the documented
/// interval spelled with redundant leading zeros is as wide as the refused rows above,
/// and is purchased — the plan reading back at the commitment the digits denote.
#[tokio::test]
async fn a_wide_spelling_of_an_admitted_commitment_is_purchased() {
    let emulator = Emulator::new();
    let offering_id = text(
        &emulator.offering("Compute", "No Upfront", ONE_YEAR).await,
        "offeringId",
    );

    // (case, the commitment as sent, the commitment as the plan reports it)
    let rows: [(&str, String, &str); 3] = [
        (
            "the ceiling behind fifty leading zeros",
            format!("{}{MAX_COMMITMENT}", "0".repeat(50)),
            "1000000.00000000",
        ),
        (
            "the floor behind a hundred leading zeros",
            format!("{}{MIN_COMMITMENT}", "0".repeat(100)),
            "0.00100000",
        ),
        (
            "a mid-interval value behind forty leading zeros",
            format!("{}500", "0".repeat(40)),
            "500.00000000",
        ),
    ];

    for (case, commitment, reported) in rows {
        let answer = emulator
            .call(
                "CreateSavingsPlan",
                json!({ "savingsPlanOfferingId": offering_id, "commitment": commitment }),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?} (commitment {commitment:?}): expected 200, got {} {:?}",
            answer.status, answer.body
        );
        // Round trip: the purchase is only real if the reader sees it.
        let savings_plan_id = text(&answer.body, "savingsPlanId");
        let plan = emulator.plan(&savings_plan_id).await;
        let actual = text(&plan, "commitment");
        assert_eq!(
            actual, reported,
            "case {case:?} (commitment {commitment:?}): expected the plan to report \
             {reported:?}, got {actual:?}"
        );
    }
}

/// The same split on the OTHER `Amount` member of the operation: a wide
/// `upfrontPaymentAmount` is refused with the window it missed — "a whole number
/// between 50 and 99 percent of the total value of the Savings Plan" — and the clause
/// reports the value the caller sent.
#[tokio::test]
async fn a_wide_upfront_payment_amount_names_the_window_it_missed() {
    let emulator = Emulator::new();
    let offering_id = text(
        &emulator
            .offering("Compute", "Partial Upfront", ONE_YEAR)
            .await,
        "offeringId",
    );
    let before = plan_count(&emulator).await;
    let opening = "upfrontPaymentAmount must be a whole number between 50 and 99 percent of the \
                   total value of the Savings Plan (";

    // (case, the up-front amount as sent)
    let rows: [(&str, String); 5] = [
        ("one order of magnitude above the window", "8672".to_string()),
        ("twenty integer digits", wide("1", 19)),
        ("thirty-one integer digits", wide("2", 30)),
        ("thirty-two integer digits", wide("1", 31)),
        ("a hundred integer digits", wide("3", 99)),
    ];

    for (case, upfront) in rows {
        let answer = emulator
            .call(
                "CreateSavingsPlan",
                json!({
                    "savingsPlanOfferingId": offering_id,
                    "commitment": "0.1",
                    "upfrontPaymentAmount": upfront,
                }),
            )
            .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?} (upfrontPaymentAmount {upfront:?}): expected 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code, "ValidationException",
            "case {case:?} (upfrontPaymentAmount {upfront:?}): expected ValidationException, \
             got {:?}",
            answer.code
        );
        let message = text(&answer.body, "message");
        assert!(
            message.starts_with(opening),
            "case {case:?} (upfrontPaymentAmount {upfront:?}): expected the window clause, got \
             {message:?}"
        );
        assert!(
            message.ends_with(&format!("got '{upfront}'")),
            "case {case:?} (upfrontPaymentAmount {upfront:?}): expected the clause to report the \
             submitted value, got {message:?}"
        );
    }

    assert_eq!(
        plan_count(&emulator).await,
        before,
        "a refused purchase must create nothing"
    );
}

/// `upfrontPaymentAmount`'s own FORM half, at width: a wide value that is not a plain
/// decimal number is refused on its form, and a wide one carrying a fraction the
/// window cannot take is refused for not being a whole number — each reporting the
/// value as the caller spelled it. A SIGNED decimal number is in neither half: it is
/// a number, and [`crate::negative_amount_sign_rule`] pins the bound it misses.
#[tokio::test]
async fn a_wide_upfront_payment_amount_that_is_not_an_amount_is_refused_on_its_form() {
    let emulator = Emulator::new();
    let offering_id = text(
        &emulator
            .offering("Compute", "Partial Upfront", ONE_YEAR)
            .await,
        "offeringId",
    );

    // (case, the up-front amount as sent, the clause the refusal must state)
    let rows: [(&str, String, String); 4] = [
        (
            "letters",
            "abc".to_string(),
            "upfrontPaymentAmount must be a decimal amount, got 'abc'".to_string(),
        ),
        (
            "a wide value with a trailing letter",
            format!("{}x", "9".repeat(40)),
            format!(
                "upfrontPaymentAmount must be a decimal amount, got '{}x'",
                "9".repeat(40)
            ),
        ),
        (
            "a wide value with two decimal points",
            format!("{}.1.2", "9".repeat(40)),
            format!(
                "upfrontPaymentAmount must be a decimal amount, got '{}.1.2'",
                "9".repeat(40)
            ),
        ),
        (
            "a fractional amount inside the window",
            "500.5".to_string(),
            "upfrontPaymentAmount must be a whole number, got '500.5'".to_string(),
        ),
    ];

    for (case, upfront, expected) in rows {
        let answer = emulator
            .call(
                "CreateSavingsPlan",
                json!({
                    "savingsPlanOfferingId": offering_id,
                    "commitment": "0.1",
                    "upfrontPaymentAmount": upfront,
                }),
            )
            .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?} (upfrontPaymentAmount {upfront:?}): expected 400, got {} {:?}",
            answer.status, answer.body
        );
        let message = text(&answer.body, "message");
        assert_eq!(
            message, expected,
            "case {case:?} (upfrontPaymentAmount {upfront:?}): expected {expected:?}, got \
             {message:?}"
        );
    }
}
