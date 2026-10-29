// SPDX-License-Identifier: CC0-1.0

//! WHICH member a handler-raised `ValidationException` of this service names, driven end
//! to end over the real `restJson1` wire through [`MultiServiceRouter::dispatch`].
//!
//! RULE under test: a refusal a handler raises about an input member names that member
//! by the name the MODEL declares for it — PascalCase, one declared segment per step of a
//! nested path — in `ValidationException.Fields[0].Name`, and any
//! `… at '<member>' failed to satisfy constraint: …` clause its `Message` states names
//! the same member the same way. So the member is the ONE thing a client can read to
//! locate the offending value of its own request, and two refusals about one member never
//! spell it two different ways.
//!
//! The refusals the BOUNDARY walk produces are a different kind: they carry no `Fields` at
//! all and state the lowerCamel wire path AWS states there
//! (`tests/all/validation_messages.rs` and `tests/all/validation_exception_message.rs`
//! pin those), so nothing here asserts over them.
//!
//! The rows span the class rather than one refusal: the operation (both tagging writes,
//! two creates, two paginated reads, the cost report), the refusal's own
//! `ValidationExceptionReason` (`FIELD_VALIDATION_FAILED`, `CANNOT_PARSE`,
//! `MISSING_PRICINGPLAN`, `DUPLICATE_ACCOUNT`, `ILLEGAL_BILLING_PERIOD_RANGE`), the kind
//! of member it is about (a map, a list, a scalar, members one and two segments deep),
//! whether the member travels in the body or the query string, and — for the one member
//! that TWO refusals of a single operation are about (`TagResource`'s tag map: the
//! reserved namespace and the per-resource cap) — both of them.
//!
//! Every member asserted below is the declared name the model carries
//! (`smithy_query(query="operation",operation="TagResource,UntagResource,CreateBillingGroup,ListBillingGroups,GetBillingGroupCostReport")`:
//! `Tags`, `TagKeys`, `NextToken`, `ComputationPreference.PricingPlanArn`,
//! `AccountGrouping.LinkedAccountIds`, `BillingPeriodRange`).

use crate::tagging_surface::{error_code, tag_map, Emulator};
use serde_json::{json, Value};

/// The status the modelled `ValidationException` is served on
/// (`smithy_query(query="shape",shape="ValidationException")`: `httpError` 400).
const VALIDATION_STATUS: u16 = 400;

/// How many tags one resource holds — the modelled `TagMap` maximum
/// (`smithy_query(query="shape",shape="TagMap")`: len 1..200), applied to what a resource
/// accumulates, which is the cap one of the two `TagResource` refusals is about.
const MAX_TAGS: usize = 200;

/// The member a `… at '<member>' failed to satisfy constraint: …` clause names, or `None`
/// for a refusal whose sentence states no such clause.
fn clause_member(message: &str) -> Option<&str> {
    let (_, after) = message.split_once(" at '")?;
    after.split_once('\'').map(|(member, _)| member)
}

/// The member a refusal names in `Fields`, or `None` when it names none.
fn named_member(body: &Value) -> Option<&str> {
    body.get("Fields")?
        .as_array()?
        .first()?
        .get("Name")?
        .as_str()
}

/// Assert one refusal: the modelled 400 `ValidationException`, an enumerated reason, and
/// `member` named — by the model's declared spelling — both in `Fields` and in the clause
/// the message states, when it states one.
fn assert_names_declared_member(case: &str, member: &str, answered: &(u16, Value)) {
    let (status, body) = answered;
    assert_eq!(
        *status, VALIDATION_STATUS,
        "case {case:?}: expected HTTP {VALIDATION_STATUS}, got {status} with {body}"
    );
    assert_eq!(
        error_code(body),
        "ValidationException",
        "case {case:?}: expected the modelled ValidationException, got {body}"
    );
    let reason = body["Reason"].as_str().unwrap_or_default();
    assert!(
        !reason.is_empty(),
        "case {case:?}: expected a handler-raised refusal to name its \
         ValidationExceptionReason, got {body}"
    );
    assert_eq!(
        named_member(body),
        Some(member),
        "case {case:?}: expected Fields to name the member {member:?}, got {body}"
    );

    // A declared Smithy member name begins with a capital, in every segment of a nested
    // path — which is what tells a `Fields` name apart from the lowerCamel wire path the
    // boundary walk states.
    for segment in member.split('.') {
        let initial = segment.chars().next();
        assert!(
            initial.is_some_and(|letter| letter.is_ascii_uppercase()),
            "case {case:?}: expected every segment of {member:?} to be a declared member \
             name, got the segment {segment:?}"
        );
    }

    let message = body["Message"].as_str().unwrap_or_default();
    assert!(
        !message.is_empty(),
        "case {case:?}: the model marks Message @required, got {body}"
    );
    if let Some(stated) = clause_member(message) {
        assert_eq!(
            stated, member,
            "case {case:?}: the message clause names {stated:?} while Fields names \
             {member:?}, got {message:?}"
        );
    }
}

/// Fill `plan` to the per-resource tag cap and return what ONE further key earns.
///
/// One request carries at most as many pairs as the modelled map allows, so the cap is
/// reached in two calls and the third is the refusal.
async fn refusal_past_the_tag_cap(em: &Emulator, plan: &str) -> (u16, Value) {
    let filling: Vec<(String, String)> = (0..MAX_TAGS - 1)
        .map(|index| (format!("key-{index:03}"), index.to_string()))
        .collect();
    let borrowed: Vec<(&str, &str)> = filling
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let (status, body) = em.tag(plan, &tag_map(&borrowed)).await;
    assert_eq!(
        status, 204,
        "setup: tagging {} pairs onto a fresh resource expected 204, got {status} with {body}",
        MAX_TAGS - 1
    );
    let (status, body) = em.tag(plan, &tag_map(&[("key-at-the-cap", "x")])).await;
    assert_eq!(
        status, 204,
        "setup: the key that reaches the cap expected 204, got {status} with {body}"
    );
    em.tag(plan, &tag_map(&[("one-too-many", "x")])).await
}

/// RULE: every handler-raised refusal names the member at fault by its declared name, in
/// `Fields` and in the clause its message states.
#[tokio::test]
async fn every_handler_raised_refusal_names_the_declared_input_member() {
    let em = Emulator::new();
    let plan = em.create_plan("FieldNamesPlan", None).await;
    let group = em.create_group("FieldNamesGroup", &plan, None).await;
    let capped = em.create_plan("FieldNamesCappedPlan", None).await;

    // A syntactically valid pricing-plan ARN naming no stored plan: the member is at
    // fault for what it REFERS to, not for its form.
    let missing_plan = format!(
        "arn:aws:billingconductor::{}:pricingplan/MISSINGPLN",
        ::engine::emulator_account_id()
    );

    // (case, the declared member the refusal must name, the answer it gave)
    let rows: Vec<(&str, &str, (u16, Value))> = vec![
        (
            "TagResource, a key in the reserved namespace",
            "Tags",
            em.tag(&plan, &tag_map(&[("aws:cost-center", "reserved")]))
                .await,
        ),
        (
            "TagResource, one key past the per-resource cap",
            "Tags",
            refusal_past_the_tag_cap(&em, &capped).await,
        ),
        (
            "UntagResource, a reserved key in the query string",
            "TagKeys",
            em.untag(&plan, &["aws:cost-center"]).await,
        ),
        (
            "CreatePricingPlan, a reserved tag key at create time",
            "Tags",
            em.call(
                "POST",
                "/create-pricing-plan",
                Some(&json!({
                    "Name": "FieldNamesRefusedPlan",
                    "Tags": tag_map(&[("aws:owner", "reserved")]),
                })),
            )
            .await,
        ),
        (
            "CreateCustomLineItem, a reserved tag key at create time",
            "Tags",
            em.call(
                "POST",
                "/create-custom-line-item",
                Some(&json!({
                    "Name": "FieldNamesRefusedItem",
                    "Description": "the refused custom line item",
                    "BillingGroupArn": group.clone(),
                    "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 10.0 } },
                    "Tags": tag_map(&[("aws:owner", "reserved")]),
                })),
            )
            .await,
        ),
        (
            "CreateBillingGroup, a computation preference naming no pricing plan",
            "ComputationPreference.PricingPlanArn",
            em.call(
                "POST",
                "/create-billing-group",
                Some(&json!({
                    "Name": "FieldNamesMissingPlanGroup",
                    "AccountGrouping": { "LinkedAccountIds": ["444455556666"] },
                    "ComputationPreference": { "PricingPlanArn": missing_plan },
                    "PrimaryAccountId": "444455556666",
                })),
            )
            .await,
        ),
        (
            "CreateBillingGroup, one linked account listed twice",
            "AccountGrouping.LinkedAccountIds",
            em.call(
                "POST",
                "/create-billing-group",
                Some(&json!({
                    "Name": "FieldNamesDuplicateAccountGroup",
                    "AccountGrouping": {
                        "LinkedAccountIds": ["444455556666", "444455556666"],
                    },
                    "ComputationPreference": { "PricingPlanArn": plan.clone() },
                    "PrimaryAccountId": "444455556666",
                })),
            )
            .await,
        ),
        (
            "ListBillingGroups, a NextToken this service did not mint",
            "NextToken",
            em.call(
                "POST",
                "/list-billing-groups",
                Some(&json!({ "NextToken": "not-a-token-this-service-minted" })),
            )
            .await,
        ),
        (
            "ListPricingPlans, a NextToken this service did not mint",
            "NextToken",
            em.call(
                "POST",
                "/list-pricing-plans",
                Some(&json!({ "NextToken": "0123456789abcdef" })),
            )
            .await,
        ),
        (
            "GetBillingGroupCostReport, a range covering no billing period",
            "BillingPeriodRange.ExclusiveEndBillingPeriod",
            em.call(
                "POST",
                "/get-billing-group-cost-report",
                Some(&json!({
                    "Arn": group.clone(),
                    "BillingPeriodRange": {
                        "InclusiveStartBillingPeriod": "2026-05",
                        "ExclusiveEndBillingPeriod": "2026-05",
                    },
                })),
            )
            .await,
        ),
    ];

    for (case, member, answered) in &rows {
        assert_names_declared_member(case, member, answered);
    }
}

/// RULE, where it bites: the TWO refusals one operation raises about ONE input member —
/// `TagResource`'s tag map, refused for the reserved namespace and refused for the
/// per-resource cap — name that member identically, so a client reading `Fields` to
/// locate the offending member of its own request is told the same thing by either.
///
/// Asserted by COMPARING the two answers rather than against a literal: whatever the
/// member is called, both refusals have to call it that.
#[tokio::test]
async fn both_tag_resource_refusals_about_the_tag_map_name_one_member() {
    let em = Emulator::new();
    let capped = em.create_plan("OneMemberCappedPlan", None).await;
    let fresh = em.create_plan("OneMemberFreshPlan", None).await;

    let past_the_cap = refusal_past_the_tag_cap(&em, &capped).await;
    let reserved = em
        .tag(&fresh, &tag_map(&[("aws:cost-center", "reserved")]))
        .await;

    let (cap_status, cap_body) = &past_the_cap;
    let (reserved_status, reserved_body) = &reserved;
    assert_eq!(
        cap_status, reserved_status,
        "both refusals are the modelled ValidationException, so both are served on one \
         status: the cap answered {cap_status} with {cap_body}, the reserved key \
         {reserved_status} with {reserved_body}"
    );

    let capped_member = named_member(cap_body);
    let reserved_member = named_member(reserved_body);
    assert!(
        capped_member.is_some(),
        "the cap refusal must name the member at fault in Fields, got {cap_body}"
    );
    assert_eq!(
        capped_member, reserved_member,
        "both refusals are about the same input member, so both must name it the same \
         way: the cap named {capped_member:?}, the reserved key {reserved_member:?}"
    );

    // And the clause each builds names what its own `Fields` entry does, so the sentence
    // and the structured member cannot drift apart either.
    for (case, body) in [("the cap", cap_body), ("a reserved key", reserved_body)] {
        let member = named_member(body).unwrap_or_default();
        let message = body["Message"].as_str().unwrap_or_default();
        assert_eq!(
            clause_member(message),
            Some(member),
            "case {case:?}: expected the clause to name {member:?}, got {message:?}"
        );
    }

    // The cap refusal still states the bound it is about, taken from the limit rather
    // than from the request.
    let message = cap_body["Message"].as_str().unwrap_or_default();
    assert!(
        message.contains(&MAX_TAGS.to_string()),
        "expected the cap refusal to state the bound {MAX_TAGS}, got {message:?}"
    );
}
