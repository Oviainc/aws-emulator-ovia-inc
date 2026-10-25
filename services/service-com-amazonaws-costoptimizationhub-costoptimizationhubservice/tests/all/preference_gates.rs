// SPDX-License-Identifier: CC0-1.0

//! The two organization rules over `UpdatePreferences`, driven end-to-end over
//! the wire (`awsJson1_0`) through the same multi-service router the emulator
//! boots.
//!
//! The rules under test, both of them about the ORGANIZATION the calling
//! account belongs to rather than about the values a request carries:
//!
//! 1. `memberAccountDiscountVisibility` is a preference of the organization,
//!    not of one account, so only the organization's management account may set
//!    it and only that account is reported the member by a read. Every other
//!    account's request for it is refused with the operation's modelled
//!    `ValidationException` (400) and changes nothing.
//! 2. `savingsEstimationMode` = `AfterDiscounts` reports figures net of the
//!    discounts member accounts' spend carries, so it is available only while
//!    the organization has made those discounts visible (rule 1's preference set
//!    to `All`); otherwise the same `ValidationException` (400), with the stored
//!    mode left where it was.
//!
//! Both refusals are the one the model declares for this operation
//! (`smithy_query(query="operation", operation="UpdatePreferences")`: errors
//! `AccessDeniedException`, `InternalServerException`, `ThrottlingException`,
//! `ValidationException`), and both state the member and the rule in `message`,
//! so a client can tell them from the fixed field-validation summary a value
//! outside a member's enum is answered with.
//!
//! Each rule is asserted over a table spanning what it generalises over: every
//! value of the member's own enum, the members it is sent beside, and both
//! states of the organization.

use serde_json::{json, Value};
use service_com_amazonaws_costoptimizationhub_costoptimizationhubservice as cost_optimization_hub;
use smithy_server_runtime::MultiServiceRouter;

use cost_optimization_hub::ops::types::shared_types::{
    MEMBER_ACCOUNT_DISCOUNT_VISIBILITY_VALUES, PAYMENT_OPTION_VALUES, TERM_VALUES,
};
use cost_optimization_hub::state::{
    AFTER_DISCOUNTS, BEFORE_DISCOUNTS, DISCOUNT_VISIBILITY_NONE,
};

use crate::cost_optimization_hub::{
    call, error_code, fields, management_account_router, ok, router, DISCOUNT_VISIBILITY_ALL,
    FIELD_VALIDATION_SUMMARY,
};

/// Assert one refused `UpdatePreferences`: HTTP 400 `ValidationException` whose
/// `message` states the rule rather than carrying the fixed summary a
/// constraint-walk refusal carries, with `fields` attributing it to `member`.
async fn refused(router: &MultiServiceRouter, case: &str, request: &Value, member: &str) {
    let (status, payload) = call(router, "UpdatePreferences", request).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {payload}"
    );
    let code = error_code(&payload);
    assert_eq!(
        code, "ValidationException",
        "case {case:?}: expected ValidationException, got {code:?} in {payload}"
    );
    let message = payload["message"].as_str().unwrap_or_default();
    assert!(
        !message.is_empty() && message != FIELD_VALIDATION_SUMMARY,
        "case {case:?}: expected a message stating the rule, distinguishable from the \
         field-validation summary, got {message:?} in {payload}"
    );
    let named: Vec<String> = fields(&payload)
        .into_iter()
        .map(|(field_name, _)| field_name)
        .collect();
    assert_eq!(
        named,
        vec![member.to_string()],
        "case {case:?}: expected the refusal to name {member}, got {payload}"
    );
}

/// The preferences a read reports, so a case can assert a refused write changed
/// nothing.
async fn read(router: &MultiServiceRouter) -> Value {
    ok(router, "GetPreferences", &json!({})).await
}

// ---------------------------------------------------------------------------
// Rule 1 — the organization-wide preference is the management account's
// ---------------------------------------------------------------------------

/// A member account cannot set `memberAccountDiscountVisibility`, whichever
/// value of the member's enum it asks for and whatever else the request
/// carries: the write is refused, the record is untouched, and a read never
/// reports the member for that account.
#[tokio::test]
async fn the_discount_visibility_preference_is_refused_to_a_member_account() {
    let router = router();

    // A preference the account IS allowed to write, so the rows below are shown
    // to leave a populated record untouched rather than an empty one.
    ok(
        &router,
        "UpdatePreferences",
        &json!({
            "savingsEstimationMode": BEFORE_DISCOUNTS,
            "preferredCommitment": {"term": "OneYear", "paymentOption": "PartialUpfront"},
        }),
    )
    .await;
    let before = read(&router).await;
    assert!(
        before["memberAccountDiscountVisibility"].is_null(),
        "case before: a member account is never reported the organization's preference, \
         got {before}"
    );

    // One row per value of the member's enum, each sent alone and then beside
    // every other member of the input, so it is the member and not its company
    // that is refused.
    for visibility in MEMBER_ACCOUNT_DISCOUNT_VISIBILITY_VALUES {
        let companions: Vec<(&str, Value)> = vec![
            ("alone", json!({})),
            (
                "beside the savings estimation mode",
                json!({"savingsEstimationMode": BEFORE_DISCOUNTS}),
            ),
            (
                "beside the commitment",
                json!({"preferredCommitment": {"term": TERM_VALUES[0], "paymentOption": PAYMENT_OPTION_VALUES[0]}}),
            ),
        ];
        for (companion, extra) in &companions {
            let case = format!("{visibility} {companion}");
            let mut request = extra.clone();
            request
                .as_object_mut()
                .expect("the request is an object")
                .insert(
                    "memberAccountDiscountVisibility".to_string(),
                    Value::String((*visibility).to_string()),
                );

            refused(
                &router,
                &case,
                &request,
                "memberAccountDiscountVisibility",
            )
            .await;

            let after = read(&router).await;
            assert_eq!(
                after, before,
                "case {case:?}: a refused write must leave the record at {before}, got {after}"
            );
        }
    }
}

/// The management account of the organization does set the preference: every
/// value of the member's enum is accepted, echoed back, and reported by a
/// following read — the positive control of rule 1, so the refusal above is
/// about the caller and not about the member.
#[tokio::test]
async fn the_management_account_sets_the_discount_visibility_preference() {
    let router = management_account_router();

    for visibility in MEMBER_ACCOUNT_DISCOUNT_VISIBILITY_VALUES {
        let updated = ok(
            &router,
            "UpdatePreferences",
            &json!({"memberAccountDiscountVisibility": visibility}),
        )
        .await;
        assert_eq!(
            updated["memberAccountDiscountVisibility"].as_str(),
            Some(*visibility),
            "case {visibility}: expected the written value echoed, got {updated}"
        );

        let fetched = read(&router).await;
        assert_eq!(
            fetched["memberAccountDiscountVisibility"].as_str(),
            Some(*visibility),
            "case {visibility}: expected the read to report the preference, got {fetched}"
        );
    }
}

/// The refusal of rule 1 is distinguishable from the refusal of a value outside
/// the member's enum: both are 400 `ValidationException`, but only the
/// constraint walk's carries the fixed field-validation summary, and the field
/// entries differ.
#[tokio::test]
async fn the_two_refusals_of_the_same_member_are_distinguishable() {
    let router = router();

    let (status, out_of_enum) = call(
        &router,
        "UpdatePreferences",
        &json!({"memberAccountDiscountVisibility": "Everyone"}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case outside the enum: expected HTTP 400, got {status} with {out_of_enum}"
    );
    assert_eq!(
        out_of_enum["message"].as_str(),
        Some(FIELD_VALIDATION_SUMMARY),
        "case outside the enum: expected the field-validation summary, got {out_of_enum}"
    );

    for visibility in MEMBER_ACCOUNT_DISCOUNT_VISIBILITY_VALUES {
        let (status, within_enum) = call(
            &router,
            "UpdatePreferences",
            &json!({"memberAccountDiscountVisibility": visibility}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {visibility}: expected HTTP 400, got {status} with {within_enum}"
        );
        assert_eq!(
            error_code(&within_enum),
            error_code(&out_of_enum),
            "case {visibility}: expected the same error code as the out-of-enum refusal, \
             got {within_enum}"
        );
        assert_ne!(
            within_enum["message"], out_of_enum["message"],
            "case {visibility}: the two refusals must be distinguishable by message, \
             got {within_enum}"
        );
        assert_ne!(
            fields(&within_enum),
            fields(&out_of_enum),
            "case {visibility}: expected the field diagnosis to differ, got {within_enum}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2 — the discounted mode needs the organization's discounts visible
// ---------------------------------------------------------------------------

/// `savingsEstimationMode=AfterDiscounts` is refused while the organization has
/// not made its member accounts' discounts visible — whatever else the request
/// carries — and the stored mode stays where it was, while the unrestricted
/// value of the same member is accepted.
#[tokio::test]
async fn after_discounts_is_refused_without_visible_member_account_discounts() {
    let router = router();

    // The positive control: the unrestricted mode is written and read back.
    let accepted = ok(
        &router,
        "UpdatePreferences",
        &json!({"savingsEstimationMode": BEFORE_DISCOUNTS}),
    )
    .await;
    assert_eq!(
        accepted["savingsEstimationMode"].as_str(),
        Some(BEFORE_DISCOUNTS),
        "case the unrestricted mode: expected it stored, got {accepted}"
    );
    let before = read(&router).await;

    // Rows differ in what the discounted mode is asked for beside: nothing, the
    // commitment members, and each commitment member on its own.
    let cases: Vec<(&str, Value)> = vec![
        ("alone", json!({})),
        (
            "beside both commitment members",
            json!({"preferredCommitment": {"term": TERM_VALUES[0], "paymentOption": PAYMENT_OPTION_VALUES[0]}}),
        ),
        (
            "beside the term",
            json!({"preferredCommitment": {"term": TERM_VALUES[1]}}),
        ),
        (
            "beside the payment option",
            json!({"preferredCommitment": {"paymentOption": PAYMENT_OPTION_VALUES[1]}}),
        ),
    ];

    for (case, extra) in &cases {
        let mut request = extra.clone();
        request
            .as_object_mut()
            .expect("the request is an object")
            .insert(
                "savingsEstimationMode".to_string(),
                Value::String(AFTER_DISCOUNTS.to_string()),
            );

        refused(&router, case, &request, "savingsEstimationMode").await;

        let after = read(&router).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused write must leave the record at {before}, got {after}"
        );
    }
}

/// The discounted mode follows the organization's discount visibility in both
/// directions: refused while the preference is `None`, accepted once the
/// management account sets it to `All` — including in the one request that sets
/// both — and refused again once the preference goes back to `None`.
#[tokio::test]
async fn after_discounts_follows_the_organizations_discount_visibility() {
    let router = management_account_router();

    // (case, the visibility the organization is moved to first, whether the
    // discounted mode is then available). The preference is never set at all in
    // the first row, which is the state a fresh organization is in.
    let cases: [(&str, Option<&str>, bool); 4] = [
        ("no preference set", None, false),
        ("visibility None", Some(DISCOUNT_VISIBILITY_NONE), false),
        ("visibility All", Some(DISCOUNT_VISIBILITY_ALL), true),
        ("back to None", Some(DISCOUNT_VISIBILITY_NONE), false),
    ];

    for (case, visibility, available) in &cases {
        if let Some(visibility) = visibility {
            ok(
                &router,
                "UpdatePreferences",
                &json!({"memberAccountDiscountVisibility": visibility}),
            )
            .await;
        }
        // Start every row from the unrestricted mode, so what the row shows is
        // whether the move to the discounted mode is allowed.
        ok(
            &router,
            "UpdatePreferences",
            &json!({"savingsEstimationMode": BEFORE_DISCOUNTS}),
        )
        .await;

        let request = json!({"savingsEstimationMode": AFTER_DISCOUNTS});
        if *available {
            let updated = ok(&router, "UpdatePreferences", &request).await;
            assert_eq!(
                updated["savingsEstimationMode"].as_str(),
                Some(AFTER_DISCOUNTS),
                "case {case:?}: expected the discounted mode stored, got {updated}"
            );
            let fetched = read(&router).await;
            assert_eq!(
                fetched["savingsEstimationMode"].as_str(),
                Some(AFTER_DISCOUNTS),
                "case {case:?}: expected the read to report the discounted mode, got {fetched}"
            );
        } else {
            refused(&router, case, &request, "savingsEstimationMode").await;
            let fetched = read(&router).await;
            assert_eq!(
                fetched["savingsEstimationMode"].as_str(),
                Some(BEFORE_DISCOUNTS),
                "case {case:?}: expected the stored mode untouched, got {fetched}"
            );
        }
    }

    // And in one request: enabling the visibility and asking for the discounted
    // mode together is accepted, because the state the request leaves satisfies
    // the rule.
    let fresh = management_account_router();
    let together = ok(
        &fresh,
        "UpdatePreferences",
        &json!({
            "memberAccountDiscountVisibility": DISCOUNT_VISIBILITY_ALL,
            "savingsEstimationMode": AFTER_DISCOUNTS,
        }),
    )
    .await;
    assert_eq!(
        together["savingsEstimationMode"].as_str(),
        Some(AFTER_DISCOUNTS),
        "case both members at once: expected the discounted mode stored, got {together}"
    );
    // The other way round: asking for the discounted mode while turning the
    // visibility off in the same request is refused.
    refused(
        &fresh,
        "both members at once, visibility off",
        &json!({
            "memberAccountDiscountVisibility": DISCOUNT_VISIBILITY_NONE,
            "savingsEstimationMode": AFTER_DISCOUNTS,
        }),
        "savingsEstimationMode",
    )
    .await;
}
