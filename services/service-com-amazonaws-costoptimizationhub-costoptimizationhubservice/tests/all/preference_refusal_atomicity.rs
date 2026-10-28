// SPDX-License-Identifier: CC0-1.0

//! A refused `UpdatePreferences` stores NOTHING — driven end-to-end over the
//! wire (`awsJson1_0`) through the same multi-service router the emulator boots.
//!
//! The rule under test is the one `UpdatePreferences` judges on the record a
//! request would LEAVE: the discount-adjusted savings estimation mode
//! (`savingsEstimationMode` = `AfterDiscounts`) reports figures net of the
//! organization's member-account discounts, so a request is accepted only where
//! the pair it leaves has that mode beside
//! `memberAccountDiscountVisibility` = `All` — whichever of the two members the
//! request carries. `preference_state_consistency` pins which requests that rule
//! accepts and refuses; this suite pins what a REFUSED request does to the rest
//! of the record: a refusal is answered before the merge, so none of the
//! request's members is stored, not just the two the rule is about.
//!
//! Each row therefore runs the same request twice: once in a state the rule
//! refuses it in (the whole record — both gated members and every
//! `preferredCommitment` member — must be exactly what it was), and once with
//! the mode the rule strands moved out of the way in the same request, which the
//! rule accepts (every member the request carries must now be stored). The rows
//! differ in which members the request carries beside the gated one, how many,
//! and the values they carry, drawn from the model's own enums
//! (`shared_types::{TERM_VALUES, PAYMENT_OPTION_VALUES}`).

use serde_json::{json, Map, Value};
use service_com_amazonaws_costoptimizationhub_costoptimizationhubservice as cost_optimization_hub;
use smithy_server_runtime::MultiServiceRouter;

/// The commitment enums the rows span, read from the generated catalogue so the
/// table cannot drift from the model (`smithy_query(query="shape",
/// shape="PreferredCommitment")`).
use cost_optimization_hub::ops::types::shared_types::{PAYMENT_OPTION_VALUES, TERM_VALUES};

/// The two savings estimation modes and the visibility that hides the
/// organization's discounts — read from the service rather than copied here.
use cost_optimization_hub::state::{AFTER_DISCOUNTS, BEFORE_DISCOUNTS, DISCOUNT_VISIBILITY_NONE};

use crate::cost_optimization_hub::{
    assert_stated_rule_refusal, call, management_account_router, ok, preferences_record,
    refused_member, DISCOUNT_VISIBILITY_ALL, MODE_MEMBER, VISIBILITY_MEMBER,
};

/// The third input member, which the rule is not about and which a refused
/// request must therefore not store either.
const COMMITMENT_MEMBER: &str = "preferredCommitment";

/// The whole effective preference record, as `GetPreferences` reports it.
async fn record(router: &MultiServiceRouter) -> Value {
    preferences_record(router).await
}

/// The record a request leaves when it is accepted: every member it states
/// replaces the stored one, every member it omits keeps its stored value, and
/// the rule holds at each level of the commitment — which is the merge
/// `UpdatePreferences` documents.
///
/// `stored` is a record a read already reported, so the service's defaults are
/// materialised in it and none is applied here (`preference_writes` owns the
/// defaults a member that has never been set reports).
fn merged(stored: &Value, request: &Value) -> Value {
    let pick = |member: &str| -> Option<String> {
        request[member]
            .as_str()
            .or_else(|| stored[member].as_str())
            .map(str::to_string)
    };
    let commitment = |member: &str| -> Option<String> {
        request[COMMITMENT_MEMBER][member]
            .as_str()
            .or_else(|| stored[COMMITMENT_MEMBER][member].as_str())
            .map(str::to_string)
    };

    let mut left = Map::new();
    for (member, value) in [
        (MODE_MEMBER, pick(MODE_MEMBER)),
        (VISIBILITY_MEMBER, pick(VISIBILITY_MEMBER)),
    ] {
        if let Some(value) = value {
            left.insert(member.to_string(), Value::String(value));
        }
    }
    let mut nested = Map::new();
    for member in ["term", "paymentOption"] {
        if let Some(value) = commitment(member) {
            nested.insert(member.to_string(), Value::String(value));
        }
    }
    if !nested.is_empty() {
        left.insert(COMMITMENT_MEMBER.to_string(), Value::Object(nested));
    }
    Value::Object(left)
}

/// A request the rule refuses leaves the WHOLE record untouched — the gated
/// members and every other preference member it carried — while the same request
/// with the stranded mode moved out of the way in the same call is accepted and
/// stores every member it carries.
#[tokio::test]
async fn a_refused_discount_rule_write_stores_none_of_the_request() {
    // (case, the commitment established first, the members the refused request
    // carries beside the visibility that strands the stored mode). The rows
    // differ in which members the request carries, how many, and their values;
    // each row's established commitment differs from the values its request
    // carries, so a member that leaked into the store would be visible.
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "the visibility alone",
            json!({"term": TERM_VALUES[0], "paymentOption": PAYMENT_OPTION_VALUES[0]}),
            json!({}),
        ),
        (
            "beside the commitment term",
            json!({"term": TERM_VALUES[0], "paymentOption": PAYMENT_OPTION_VALUES[0]}),
            json!({COMMITMENT_MEMBER: {"term": TERM_VALUES[1]}}),
        ),
        (
            "beside the commitment payment option",
            json!({"term": TERM_VALUES[1], "paymentOption": PAYMENT_OPTION_VALUES[0]}),
            json!({COMMITMENT_MEMBER: {"paymentOption": PAYMENT_OPTION_VALUES[1]}}),
        ),
        (
            "beside both commitment members",
            json!({"term": TERM_VALUES[1], "paymentOption": PAYMENT_OPTION_VALUES[2]}),
            json!({COMMITMENT_MEMBER: {
                "term": TERM_VALUES[0],
                "paymentOption": PAYMENT_OPTION_VALUES[1],
            }}),
        ),
        (
            "beside the discount-adjusted mode restated",
            json!({"term": TERM_VALUES[0], "paymentOption": PAYMENT_OPTION_VALUES[1]}),
            json!({
                MODE_MEMBER: AFTER_DISCOUNTS,
                COMMITMENT_MEMBER: {"paymentOption": PAYMENT_OPTION_VALUES[2]},
            }),
        ),
    ];

    for (case, commitment, beside) in &cases {
        // A fresh organization per row, with the caller recorded as its
        // management account, so the organization-wide member is its to set.
        let router = management_account_router();

        // The state the rule can be left unsatisfied from: the discounts
        // visible, the discount-adjusted mode stored, and a commitment whose
        // values the request under test does not restate.
        for step in [
            json!({VISIBILITY_MEMBER: DISCOUNT_VISIBILITY_ALL}),
            json!({MODE_MEMBER: AFTER_DISCOUNTS}),
            json!({COMMITMENT_MEMBER: commitment}),
        ] {
            ok(&router, "UpdatePreferences", &step).await;
        }
        let before = record(&router).await;
        assert_eq!(
            before[MODE_MEMBER].as_str(),
            Some(AFTER_DISCOUNTS),
            "case {case:?}: expected the discount-adjusted mode stored, got {before}"
        );

        // The request the rule refuses: hiding the organization's discounts
        // would leave the stored mode reporting figures net of discounts the
        // member accounts are no longer shown.
        let mut request = beside.clone();
        request
            .as_object_mut()
            .expect("the request is an object")
            .insert(
                VISIBILITY_MEMBER.to_string(),
                Value::String(DISCOUNT_VISIBILITY_NONE.to_string()),
            );

        let (status, payload) = call(&router, "UpdatePreferences", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: {request} applied to {before} leaves the discount-adjusted mode \
             without visible discounts, so expected HTTP 400, got {status} with {payload}"
        );
        assert_stated_rule_refusal(&payload, case, refused_member(&request));

        // The refusal is answered before the merge, so not one member of the
        // refused request reached the store — including the ones the rule is
        // not about.
        let after = record(&router).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused write must leave the whole record at {before}, got {after}"
        );

        // The same request, with the mode it would strand moved out of the way
        // in the same call: the record it leaves satisfies the rule, so it is
        // accepted — and every member it carries is stored, which is what makes
        // the assertion above a real check rather than a write that never
        // happens.
        let mut accepted = request.clone();
        accepted
            .as_object_mut()
            .expect("the request is an object")
            .insert(
                MODE_MEMBER.to_string(),
                Value::String(BEFORE_DISCOUNTS.to_string()),
            );
        let expected = merged(&before, &accepted);
        let answered = ok(&router, "UpdatePreferences", &accepted).await;
        assert_eq!(
            answered, expected,
            "case {case:?}: expected {accepted} to answer the record {expected}, got {answered}"
        );
        let stored = record(&router).await;
        assert_eq!(
            stored, expected,
            "case {case:?}: expected the read to report {expected}, got {stored}"
        );
    }
}
