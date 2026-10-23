// SPDX-License-Identifier: CC0-1.0

//! The lower and upper BOUND of `CreateSavingsPlan.commitment`, and what a request
//! outside them is told.
//!
//! `commitment` targets `Amount`, a plain string the model constrains in prose only, so
//! the rule is the documented one: "The hourly commitment, in the same currency of the
//! savingsPlanOfferingId. This is a value between 0.001 and 1 million. You cannot
//! specify more than five digits after the decimal point."
//! (`smithy_query(query="shape",shape="CreateSavingsPlanRequest")`, member
//! `commitment`.)
//!
//! What that makes a rule rather than one rejected value:
//!
//! * the interval is CLOSED at both published bounds — each is accepted — and
//!   everything outside it is refused, so zero, a fraction below the floor and a
//!   negative amount are all refused for the same reason: they are below the minimum,
//!   not malformed;
//! * a value outside the interval is refused with the modelled `ValidationException`
//!   (HTTP 400) whose message names the member and the bound the value missed;
//! * a refused purchase creates NOTHING: the account holds exactly the plans it held
//!   before the call.
//!
//! Everything goes through the same `MultiServiceRouter` the emulator boots, reusing
//! the harness in [`crate::savings_plans_lifecycle`].

use serde_json::{json, Value};

use crate::savings_plans_lifecycle::{text, Emulator, ONE_YEAR};

/// The documented minimum hourly commitment, as the service quotes it back.
const MIN_COMMITMENT: &str = "0.001";

/// The documented maximum ("1 million"), as the service quotes it back.
const MAX_COMMITMENT: &str = "1000000";

/// Every Savings Plan the account holds, so a row can assert a refusal created none.
async fn plan_ids(emulator: &Emulator) -> Vec<String> {
    let answer = emulator.call("DescribeSavingsPlans", json!({})).await;
    assert_eq!(
        answer.status, 200,
        "describing every plan: expected 200, got {} {:?}",
        answer.status, answer.body
    );
    answer
        .list("savingsPlans")
        .iter()
        .map(|plan| text(plan, "savingsPlanId"))
        .collect()
}

/// A `commitment` outside the documented interval is refused with the bound it missed,
/// and buys nothing.
///
/// The rows differ along what the rule generalises over: which bound was missed, how
/// far outside the interval the value is (one step, an order of magnitude, a negative
/// amount), and how the value is spelled (trailing zeros, no fractional part at all).
/// The floor rows deliberately include values the "non-negative amount" reading admits
/// — `0` and `0.0005` — since that reading is the one this pins against.
#[tokio::test]
async fn a_commitment_outside_the_documented_interval_names_the_bound_it_missed() {
    let emulator = Emulator::new();
    let offering_id = text(
        &emulator.offering("Compute", "No Upfront", ONE_YEAR).await,
        "offeringId",
    );
    let before = plan_ids(&emulator).await;

    let below = format!("'commitment' cannot be less than {MIN_COMMITMENT}");
    let above = format!("'commitment' cannot be greater than {MAX_COMMITMENT}");
    // (case, the commitment as sent, the whole expected message)
    let rows: [(&str, &str, &String); 8] = [
        ("zero", "0", &below),
        ("zero with a fractional part", "0.00000", &below),
        ("one step below the floor", "0.00099", &below),
        ("half the floor", "0.0005", &below),
        ("a negative amount", "-1", &below),
        ("a negative fraction", "-0.5", &below),
        ("one step above the ceiling", "1000000.00001", &above),
        ("an order of magnitude above the ceiling", "10000000", &above),
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
        assert_eq!(
            answer.code, "ValidationException",
            "case {case:?} (commitment {commitment:?}): expected ValidationException, got {:?}",
            answer.code
        );
        let message = text(&answer.body, "message");
        assert_eq!(
            &message, expected,
            "case {case:?} (commitment {commitment:?}): expected {expected:?}, got {message:?}"
        );
        assert_eq!(
            plan_ids(&emulator).await,
            before,
            "case {case:?} (commitment {commitment:?}): a refused purchase must create nothing"
        );
    }
}

/// Both published bounds are INSIDE the interval: each is purchased, and the plan reads
/// back through `DescribeSavingsPlans` carrying the commitment it was bought at.
///
/// The rows span the interval rather than one point of it: each bound, each bound
/// spelled with redundant zeros, and a value in the middle.
#[tokio::test]
async fn every_commitment_inside_the_documented_interval_is_purchased() {
    let emulator = Emulator::new();
    let offering_id = text(
        &emulator.offering("Compute", "No Upfront", ONE_YEAR).await,
        "offeringId",
    );

    // (case, the commitment as sent, the commitment as the plan reports it)
    let rows: [(&str, &str, &str); 5] = [
        ("the published minimum", MIN_COMMITMENT, "0.00100000"),
        (
            "the published minimum spelled with trailing zeros",
            "0.00100",
            "0.00100000",
        ),
        ("just inside the minimum", "0.00101", "0.00101000"),
        ("the published maximum", MAX_COMMITMENT, "1000000.00000000"),
        ("the middle of the interval", "500", "500.00000000"),
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
        assert_eq!(
            text(&plan, "commitment"),
            reported,
            "case {case:?} (commitment {commitment:?}): expected the plan to report \
             {reported:?}, got {:?}",
            plan.get("commitment").unwrap_or(&Value::Null)
        );
    }
}

/// A `commitment` that is not a decimal amount at all is refused on its FORM, not with
/// a bound it missed: the two refusals are different rules, and neither stands in for
/// the other.
#[tokio::test]
async fn a_commitment_that_is_not_an_amount_is_refused_on_its_form() {
    let emulator = Emulator::new();
    let offering_id = text(
        &emulator.offering("Compute", "No Upfront", ONE_YEAR).await,
        "offeringId",
    );

    // (case, the commitment as sent)
    let malformed: [(&str, &str); 6] = [
        ("empty", ""),
        ("letters", "abc"),
        ("scientific notation", "1e5"),
        ("two decimal points", "1.2.3"),
        ("a trailing point", "1."),
        ("more digits than a request may specify", "0.123456"),
    ];

    for (case, commitment) in malformed {
        let answer = emulator
            .call(
                "CreateSavingsPlan",
                json!({ "savingsPlanOfferingId": offering_id, "commitment": commitment }),
            )
            .await;
        let message = text(&answer.body, "message");
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
        assert!(
            message.contains("commitment") && !message.contains("cannot be less than"),
            "case {case:?} (commitment {commitment:?}): expected a refusal of the value's form, \
             got {message:?}"
        );
    }
}
