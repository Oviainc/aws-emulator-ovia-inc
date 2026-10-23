// SPDX-License-Identifier: CC0-1.0

//! The payer-account gate as a PRE-CHECK: what it runs AHEAD of.
//!
//! RULE under test: a caller that is not the payer (management) account of the
//! consolidated billing family is refused HTTP 403 `AccessDeniedException` — the shape the
//! model lists as a common error of every operation of this service (`errorKind` client,
//! `@httpError 403`, `@required Message`) — with the `Message` built from the request
//! (`Account <caller account id> is not authorized to call <OperationName>. Only payer
//! account is authorized`), by a check that depends on the CALLER and the store alone and
//! therefore precedes everything the request says: the operation's `@idempotencyToken`
//! lookup, its resource-existence resolution, its per-resource result assembly, and any
//! mutation of state. So a denied caller never observes a 200, a 400
//! `ValidationException`, a 404 `ResourceNotFoundException`, a 409 `ConflictException`,
//! a per-resource `Failed*Resources` entry or a created resource, whatever the request
//! names and however many callers race.
//!
//! `tests/all/payer_account_gate.rs` is the suite that asserts the gate covers every
//! modelled operation and that a modelled-constraint refusal (400) still precedes it;
//! these rows are about the STAGES the gate comes before, on the surfaces whose own
//! answer for the same request would be a different status class — the pricing-rule
//! create (200 + `Arn`), the four `Delete*` existence checks (400/404), the three
//! `Update*` existence checks (404), the two batch association operations (404 or a
//! `Failed*Resources` list inside a 200) and the two bidirectional pricing
//! cross-reference reads (404).
//!
//! The tables span what the rule generalises over rather than one recorded call: the
//! operation and the HTTP method its route is bound to, the stage the request would
//! otherwise have reached, whether an `@idempotencyToken` is absent / fresh / REUSED,
//! the form a conforming resource identifier takes (this account's full ARN, another
//! account's full ARN, the bare id — all three admitted by the members' modelled
//! `@pattern`), how many callers race, and which account the store records as the payer.
//!
//! Every assertion goes over the service's real `restJson1` wire through
//! [`MultiServiceRouter::dispatch`], the same path the production router serves.

use serde_json::{json, Value};

// The harness is the sibling suite's: one `Emulator`, one denial assertion and one
// concurrent-join helper for the gate, rather than a second copy of each. `account_id` is
// the account every request below is made by, read from that same one source.
use crate::payer_account_gate::{
    account_id, assert_denied, error_code, futures_lite_join, Emulator,
};

/// A resource identifier in the three forms this service's ARN members admit, each
/// naming a resource that exists nowhere.
///
/// Every form satisfies the members' modelled `@pattern` — e.g. `PricingRuleArn` is
/// `^(arn:aws(-cn)?:billingconductor::[0-9]{12}:pricingrule/)?[a-zA-Z0-9]{10}$`
/// (`smithy_query(query="shape",shape="PricingRuleArn,PricingPlanArn,CustomLineItemArn,
/// BillingGroupArn")`), so the ARN prefix is optional and the id is 10 alphanumerics
/// (10-12 for a billing group) — which is what keeps the boundary's constraint refusal
/// out of the way and leaves the gate as the only thing that can answer.
fn identifier_forms(resource: &str, id: &str) -> Vec<(&'static str, String)> {
    vec![
        (
            "this account's full ARN",
            format!(
                "arn:aws:billingconductor::{}:{resource}/{id}",
                account_id()
            ),
        ),
        (
            "another account's full ARN",
            format!("arn:aws:billingconductor::111122223333:{resource}/{id}"),
        ),
        ("the bare resource id", id.to_string()),
    ]
}

/// One request the gate must answer, and the stage it would otherwise have reached.
struct Row {
    /// The modelled operation name, which is also what the denial `Message` must name.
    operation: &'static str,
    method: &'static str,
    uri: &'static str,
    /// The resource type segment and the nonexistent id its ARN members take.
    resource: &'static str,
    id: &'static str,
    /// Build the request body around one form of that identifier.
    body: fn(&str) -> Value,
    /// The stage this operation's own code would have answered from, for the case name.
    otherwise: &'static str,
}

/// The `Delete*`, `Update*`, batch-association and pricing cross-reference rows: every
/// one names a resource that exists nowhere, so the operation's own answer would come
/// from its existence check (a 400 or a 404) or from its per-resource assembly.
fn rows() -> Vec<Row> {
    vec![
        Row {
            operation: "DeletePricingRule",
            method: "POST",
            uri: "/delete-pricing-rule",
            resource: "pricingrule",
            id: "BOGUS00002",
            body: |arn| json!({ "Arn": arn }),
            otherwise: "the pricing-rule existence check",
        },
        Row {
            operation: "DeletePricingPlan",
            method: "POST",
            uri: "/delete-pricing-plan",
            resource: "pricingplan",
            id: "BOGUS00002",
            body: |arn| json!({ "Arn": arn }),
            otherwise: "the pricing-plan existence check",
        },
        Row {
            operation: "DeleteBillingGroup",
            method: "POST",
            uri: "/delete-billing-group",
            resource: "billinggroup",
            id: "BOGUS00002",
            body: |arn| json!({ "Arn": arn }),
            otherwise: "the billing-group existence check",
        },
        Row {
            operation: "DeleteCustomLineItem",
            method: "POST",
            uri: "/delete-custom-line-item",
            resource: "customlineitem",
            id: "BOGUS00002",
            body: |arn| json!({ "Arn": arn }),
            otherwise: "the custom-line-item existence check",
        },
        Row {
            operation: "UpdatePricingRule",
            method: "PUT",
            uri: "/update-pricing-rule",
            resource: "pricingrule",
            id: "BOGUS00008",
            body: |arn| json!({ "Arn": arn, "Description": "updated" }),
            otherwise: "the pricing-rule existence check",
        },
        Row {
            operation: "UpdateBillingGroup",
            method: "POST",
            uri: "/update-billing-group",
            resource: "billinggroup",
            id: "BOGUS00008",
            body: |arn| json!({ "Arn": arn, "Description": "updated" }),
            otherwise: "the billing-group existence check",
        },
        Row {
            operation: "UpdateCustomLineItem",
            method: "POST",
            uri: "/update-custom-line-item",
            resource: "customlineitem",
            id: "BOGUS00008",
            body: |arn| json!({ "Arn": arn, "Description": "updated" }),
            otherwise: "the custom-line-item existence check",
        },
        Row {
            operation: "BatchAssociateResourcesToCustomLineItem",
            method: "PUT",
            uri: "/batch-associate-resources-to-custom-line-item",
            resource: "customlineitem",
            id: "BOGUS00005",
            body: |arn| {
                json!({
                    "TargetArn": arn,
                    "ResourceArns": [format!(
                        "arn:aws:billingconductor::{}:billinggroup/BOGUS00005",
                        account_id()
                    )],
                })
            },
            otherwise: "the target resolution and the per-resource result assembly",
        },
        Row {
            operation: "BatchDisassociateResourcesFromCustomLineItem",
            method: "PUT",
            uri: "/batch-disassociate-resources-from-custom-line-item",
            resource: "customlineitem",
            id: "BOGUS00005",
            body: |arn| {
                json!({
                    "TargetArn": arn,
                    "ResourceArns": [format!(
                        "arn:aws:billingconductor::{}:billinggroup/BOGUS00005",
                        account_id()
                    )],
                })
            },
            otherwise: "the target resolution and the per-resource result assembly",
        },
        Row {
            operation: "ListPricingRulesAssociatedToPricingPlan",
            method: "POST",
            uri: "/list-pricing-rules-associated-to-pricing-plan",
            resource: "pricingplan",
            id: "BOGUS00004",
            body: |arn| json!({ "PricingPlanArn": arn }),
            otherwise: "the pricing-plan existence check of a read-only listing",
        },
        Row {
            operation: "ListPricingPlansAssociatedWithPricingRule",
            method: "POST",
            uri: "/list-pricing-plans-associated-with-pricing-rule",
            resource: "pricingrule",
            id: "BOGUS00004",
            body: |arn| json!({ "PricingRuleArn": arn }),
            otherwise: "the pricing-rule existence check of a read-only listing",
        },
    ]
}

/// THE RULE on the surfaces whose own answer for the same request is a resource-shaped
/// 4xx: the gate comes first, so a caller that is not the payer account is denied 403
/// `AccessDeniedException` and never 400 `ValidationException` or 404
/// `ResourceNotFoundException`, in every identifier form the member admits.
///
/// The same request made BY the payer account is asserted to reach the operation (it is
/// answered by something other than the gate), which is what makes the 403 the gate's
/// decision rather than a blanket refusal of the whole surface.
#[tokio::test]
async fn the_gate_precedes_resource_existence_resolution_on_every_gated_surface() {
    let standalone = Emulator::standalone();
    let payer = Emulator::payer();

    for row in rows() {
        for (form, identifier) in identifier_forms(row.resource, row.id) {
            let case = format!(
                "{} ({} {}) with {form}, ahead of {}",
                row.operation, row.method, row.uri, row.otherwise
            );
            let body = (row.body)(&identifier);

            let denied = standalone.call(row.method, row.uri, Some(&body)).await;
            assert_denied(&case, row.operation, &denied);

            let (status, served, _) = payer.call(row.method, row.uri, Some(&body)).await;
            assert_ne!(
                status, 403,
                "case {case:?}: expected the payer account to reach the operation, \
                 got {status} with {served}"
            );
            assert_ne!(
                error_code(&served),
                "AccessDeniedException",
                "case {case:?}: the payer account must not be denied, got {served}"
            );
        }
    }
}

/// The batch association denial is TOP-LEVEL: an authorization refusal is the whole
/// answer, never a per-resource entry in `FailedAssociatedResources` /
/// `FailedDisassociatedResources` inside a 200 — so neither the success nor the failure
/// list is in the body at all.
#[tokio::test]
async fn the_batch_association_denial_is_top_level_not_a_per_resource_failure_entry() {
    let em = Emulator::standalone();
    // (operation, uri, the output members `smithy_query(query="operation",
    // operation="BatchAssociateResourcesToCustomLineItem,
    // BatchDisassociateResourcesFromCustomLineItem")` lists for a served call)
    let surfaces = [
        (
            "BatchAssociateResourcesToCustomLineItem",
            "/batch-associate-resources-to-custom-line-item",
            [
                "SuccessfullyAssociatedResources",
                "FailedAssociatedResources",
            ],
        ),
        (
            "BatchDisassociateResourcesFromCustomLineItem",
            "/batch-disassociate-resources-from-custom-line-item",
            [
                "SuccessfullyDisassociatedResources",
                "FailedDisassociatedResources",
            ],
        ),
    ];

    // The resource list spans its modelled length: one entry, and several naming both
    // resource types `CustomLineItemAssociationElement`'s pattern admits.
    let resource_lists = [
        (
            "one billing group",
            vec![format!(
                "arn:aws:billingconductor::{}:billinggroup/BOGUS00005",
                account_id()
            )],
        ),
        (
            "three resources of both admitted types",
            vec![
                format!(
                    "arn:aws:billingconductor::{}:billinggroup/BOGUS00005",
                    account_id()
                ),
                format!(
                    "arn:aws:billingconductor::{}:customlineitem/BOGUS00006",
                    account_id()
                ),
                "BOGUS00007".to_string(),
            ],
        ),
    ];

    for (operation, uri, members) in &surfaces {
        for (list_case, resources) in &resource_lists {
            let case = format!("{operation} with {list_case}");
            let body = json!({
                "TargetArn": format!(
                    "arn:aws:billingconductor::{}:customlineitem/BOGUS00005",
                    account_id()
                ),
                "ResourceArns": resources,
            });
            let answered = em.call("PUT", uri, Some(&body)).await;
            assert_denied(&case, operation, &answered);
            let (_, served, _) = &answered;
            for member in members {
                assert!(
                    served.get(member).is_none(),
                    "case {case:?}: expected no {member} member in a top-level \
                     authorization refusal, got {served}"
                );
            }
        }
    }
}

/// THE RULE ahead of the `@idempotencyToken` lookup: `CreatePricingRule`'s `ClientToken`
/// is modelled `@idempotencyToken` bound to header `X-Amzn-Client-Token`
/// (`smithy_query(query="operation",operation="CreatePricingRule")`), and the gate runs
/// before it — so an absent token, a fresh token and a REUSED token are all the identical
/// denial, no cached "already succeeded" answer can be laundered through the idempotency
/// path, and nothing is created.
#[tokio::test]
async fn the_gate_precedes_the_idempotency_token_lookup_and_creates_nothing() {
    let em = Emulator::standalone();
    // (case, the ClientToken header to send, the rule to create) — the rows differ along
    // whether an idempotency token is sent, whether it REPEATS an earlier one, and along
    // the rule itself (its scope, type and modifier), with one Name repeated across rows
    // so a cached create would show up.
    let rows: Vec<(&str, Option<&str>, Value)> = vec![
        (
            "no ClientToken at all",
            None,
            json!({
                "Name": "GatedRule",
                "Scope": "GLOBAL",
                "Type": "MARKUP",
                "ModifierPercentage": 10.0,
            }),
        ),
        (
            "a fresh ClientToken, first use",
            Some("token-a"),
            json!({
                "Name": "GatedRule",
                "Scope": "GLOBAL",
                "Type": "MARKUP",
                "ModifierPercentage": 10.0,
            }),
        ),
        (
            "the SAME ClientToken reused",
            Some("token-a"),
            json!({
                "Name": "GatedRule",
                "Scope": "GLOBAL",
                "Type": "MARKUP",
                "ModifierPercentage": 10.0,
            }),
        ),
        (
            "a different ClientToken for the same Name",
            Some("token-b"),
            json!({
                "Name": "GatedRule",
                "Scope": "GLOBAL",
                "Type": "MARKUP",
                "ModifierPercentage": 10.0,
            }),
        ),
        (
            "a service-scoped discount rule with a fresh ClientToken",
            Some("token-c"),
            json!({
                "Name": "GatedServiceRule",
                "Scope": "SERVICE",
                "Type": "DISCOUNT",
                "Service": "AmazonEC2",
                "ModifierPercentage": 2.5,
            }),
        ),
        (
            "a tiering rule with no ClientToken",
            None,
            json!({
                "Name": "GatedTieringRule",
                "Scope": "GLOBAL",
                "Type": "TIERING",
                "Tiering": { "FreeTier": { "Activated": true } },
            }),
        ),
    ];

    for (case, token, body) in &rows {
        let headers: Vec<(&str, &str)> = token
            .map(|value| vec![("x-amzn-client-token", value)])
            .unwrap_or_default();
        let answered = em
            .call_with_headers("POST", "/create-pricing-rule", Some(body), &headers)
            .await;
        assert_denied(case, "CreatePricingRule", &answered);
        let (_, served, _) = &answered;
        assert!(
            served.get("Arn").is_none(),
            "case {case:?}: a denial must carry no pricing-rule Arn, got {served}"
        );
    }

    // Nothing was created: designating this caller the payer account makes the admitted
    // arm of the gate reachable, and the read it is observable through reports an empty
    // page.
    em.designate_payer(Some(account_id()));
    let (status, body, _) = em.post("/list-pricing-rules", &json!({})).await;
    assert_eq!(
        status, 200,
        "expected the payer account to be served ListPricingRules, got {status} with {body}"
    );
    assert_eq!(
        body["PricingRules"].as_array().map(Vec::len),
        Some(0),
        "expected a denied CreatePricingRule to have created no rule, got {body}"
    );
}

/// THE RULE is DETERMINISTIC under concurrency on the CREATE surface too: N racers on ONE
/// Name are all denied identically — the eligibility check never races into the name
/// conflict stage or into a success, because it reads the caller and the store and
/// nothing else. Zero winners, and no rule in state afterwards.
#[tokio::test]
async fn concurrent_create_pricing_rule_racers_on_one_name_have_no_winner() {
    // The racer counts span one caller, the recorded six, and more callers than that.
    for racers in [1_usize, 6, 12] {
        let em = Emulator::standalone();
        let body = json!({
            "Name": "RacedRule",
            "Scope": "GLOBAL",
            "Type": "MARKUP",
            "ModifierPercentage": 10.0,
        });
        let calls = (0..racers).map(|_| em.post("/create-pricing-rule", &body));
        let answers = futures_lite_join(calls).await;
        assert_eq!(
            answers.len(),
            racers,
            "case {racers} racers: expected one answer per racer, got {}",
            answers.len()
        );
        let winners: Vec<&Value> = answers
            .iter()
            .filter(|(status, _, _)| *status < 300)
            .map(|(_, body, _)| body)
            .collect();
        assert!(
            winners.is_empty(),
            "case {racers} racers: expected zero winners on a non-payer account, \
             got {winners:?}"
        );
        for (racer, answered) in answers.iter().enumerate() {
            assert_denied(
                &format!("racer {racer} of {racers} on one Name"),
                "CreatePricingRule",
                answered,
            );
        }

        em.designate_payer(Some(account_id()));
        let (status, listed, _) = em.post("/list-pricing-rules", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {racers} racers: expected the payer account to be served, \
             got {status} with {listed}"
        );
        assert_eq!(
            listed["PricingRules"].as_array().map(Vec::len),
            Some(0),
            "case {racers} racers: expected no rule to have been created, got {listed}"
        );
    }
}
