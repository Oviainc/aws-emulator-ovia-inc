// SPDX-License-Identifier: CC0-1.0

//! `BatchDisassociateResourcesFromCustomLineItem` end to end over the wire:
//! `CreateCustomLineItem` -> `BatchAssociateResourcesToCustomLineItem` ->
//! `BatchDisassociateResourcesFromCustomLineItem` ->
//! `ListResourcesAssociatedToCustomLineItem`.
//!
//! RULE: the operation REMOVES the stored association of each requested resource from
//! the percentage custom line item the `TargetArn` names and reports PER RESOURCE — a
//! resource it disassociated in `SuccessfullyDisassociatedResources`, one it could not
//! in `FailedDisassociatedResources` with the `AssociateResourceErrorReason` that
//! classifies the failure — while a fault of the request as a whole (an unknown target,
//! a target that is not a percentage item, a billing period range covering no period)
//! is the call's own modelled error. It is `@idempotent`, so a resource the item holds
//! no association for is already disassociated and its element succeeds unchanged. A
//! request that names no `BillingPeriodRange` disassociates the resource in every
//! billing period, so the association is dropped; one that names a range disassociates
//! it ONLY in the billing periods that half-open range covers, so the association keeps
//! every period outside it — the earlier ones it applied in, and the ones from the
//! range's exclusive end on — and is dropped when it would then cover no period at all.
//! What is left is what `ListResourcesAssociatedToCustomLineItem` reports back, with the
//! `EndBillingPeriod` of the association as it stands in the period the read asks about.
//!
//! The rows differ along the class of the resource (billing group / custom line item),
//! the form the caller names it in (full ARN / bare id), the number of resources in one
//! request, the way a resource fails, and where the disassociation's billing period
//! range falls relative to the association's own range (inside it, overlapping either
//! end, covering it, disjoint on either side) — never along a renaming.

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_billingconductor_awsbillingconductor::{
    ComAmazonawsBillingconductorAwsbillingconductorState as BillingConductorState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// An AWS-managed pricing plan every billing group here computes with: the
/// `PricingPlanFullArn` pattern enumerates it, so no plan has to be created first
/// (`smithy_query(query="shape",shape="PricingPlanFullArn")`).
const MANAGED_PRICING_PLAN: &str = "arn:aws:billingconductor::aws:pricingplan/BasicPricingPlan";

/// A live emulator holding only this service, with the emulated account designated the
/// organization's payer account — Billing Conductor's control plane is the payer
/// account's alone, so that is the world in which these associations are reachable.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        engine
            .state::<BillingConductorState>()
            .lock()
            .payer_account_id = Some(::engine::emulator_account_id().to_string());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// Send a JSON body to one of this service's routes and return the status and the
    /// parsed body. The method matters: the batch association routes are `PUT`
    /// (`smithy_query(query="operation",operation=
    /// "BatchDisassociateResourcesFromCustomLineItem")`).
    async fn send(&self, method: &str, path: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method(method)
            .uri(format!(
                "https://billingconductor.us-east-1.amazonaws.com{path}"
            ))
            .header("host", "billingconductor.us-east-1.amazonaws.com")
            .header("content-type", "application/json")
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/us-east-1/billingconductor/aws4_request, \
                 SignedHeaders=host, Signature=00",
            )
            .body(Bytes::from(
                serde_json::to_vec(body).expect("serialize the request body"),
            ))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Create a billing group and return its ARN.
    async fn billing_group(&self, name: &str) -> String {
        let (status, body) = self
            .send(
                "POST",
                "/create-billing-group",
                &json!({
                    "Name": name,
                    "AccountGrouping": { "LinkedAccountIds": [] },
                    "ComputationPreference": { "PricingPlanArn": MANAGED_PRICING_PLAN },
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case billing group {name:?}: expected HTTP 200, got {status} with {body}"
        );
        arn_of(&body, &format!("billing group {name:?}"))
    }

    /// Create a custom line item with the given charge details and return its ARN.
    async fn custom_line_item(&self, group_arn: &str, name: &str, charge: Value) -> String {
        let (status, body) = self
            .send(
                "POST",
                "/create-custom-line-item",
                &json!({
                    "Name": name,
                    "Description": "a custom line item",
                    "BillingGroupArn": group_arn,
                    "ChargeDetails": charge,
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case custom line item {name:?}: expected HTTP 200, got {status} with {body}"
        );
        arn_of(&body, &format!("custom line item {name:?}"))
    }

    /// A percentage custom line item: the only kind resources are associated to.
    async fn percentage_item(&self, group_arn: &str, name: &str) -> String {
        self.custom_line_item(
            group_arn,
            name,
            json!({ "Type": "FEE", "Percentage": { "PercentageValue": 10.0 } }),
        )
        .await
    }

    /// A flat custom line item: a fixed charge value, with no associated resources.
    async fn flat_item(&self, group_arn: &str, name: &str) -> String {
        self.custom_line_item(
            group_arn,
            name,
            json!({ "Type": "CREDIT", "Flat": { "ChargeValue": 7.5 } }),
        )
        .await
    }

    /// `BatchAssociateResourcesToCustomLineItem`, asserted to succeed: the state these
    /// disassociations act on.
    async fn associate(&self, target: &str, resources: &[String], range: Option<Value>) {
        let mut body = json!({ "TargetArn": target, "ResourceArns": resources });
        if let Some(range) = range {
            body["BillingPeriodRange"] = range;
        }
        let (status, answered) = self
            .send("PUT", "/batch-associate-resources-to-custom-line-item", &body)
            .await;
        assert_eq!(
            status, 200,
            "case associating {resources:?} to {target}: expected HTTP 200, got {status} with \
             {answered}"
        );
        assert!(
            failures(&answered, "FailedAssociatedResources").is_empty(),
            "case associating {resources:?} to {target}: expected no failed resource, got \
             {answered}"
        );
    }

    /// `BatchDisassociateResourcesFromCustomLineItem` with the resources named exactly
    /// as the caller spells them, and an optional `BillingPeriodRange`.
    async fn disassociate(
        &self,
        target: &str,
        resources: &[String],
        range: Option<Value>,
    ) -> (u16, Value) {
        let mut body = json!({ "TargetArn": target, "ResourceArns": resources });
        if let Some(range) = range {
            body["BillingPeriodRange"] = range;
        }
        self.send(
            "PUT",
            "/batch-disassociate-resources-from-custom-line-item",
            &body,
        )
        .await
    }

    /// `ListResourcesAssociatedToCustomLineItem`, with whatever extra members the
    /// caller wants on the request.
    async fn associated(&self, target: &str, extra: Value) -> (u16, Value) {
        let mut body = json!({ "Arn": target });
        for (member, value) in extra.as_object().cloned().unwrap_or_default() {
            body[member] = value;
        }
        self.send(
            "POST",
            "/list-resources-associated-to-custom-line-item",
            &body,
        )
        .await
    }

    /// The `(Arn, Relationship)` pairs a read reports, sorted.
    async fn association_pairs(&self, target: &str, extra: Value) -> Vec<(String, String)> {
        let (status, body) = self.associated(target, extra).await;
        assert_eq!(
            status, 200,
            "case read of {target}: expected HTTP 200, got {status} with {body}"
        );
        let mut pairs: Vec<(String, String)> = elements(&body)
            .iter()
            .map(|(arn, relationship, _)| (arn.clone(), relationship.clone()))
            .collect();
        pairs.sort();
        pairs
    }

    /// The `EndBillingPeriod` a read reports for one associated resource, and whether
    /// the association is reported at all.
    async fn end_billing_period(&self, target: &str, resource: &str) -> Option<Option<String>> {
        self.in_period(target, resource, json!({})).await
    }

    /// Whether a read of `target` for one billing period reports `resource` at all —
    /// `None` when it does not — and, when it does, the `EndBillingPeriod` it carries
    /// there.
    async fn in_billing_period(
        &self,
        target: &str,
        resource: &str,
        billing_period: &str,
    ) -> Option<Option<String>> {
        self.in_period(target, resource, json!({ "BillingPeriod": billing_period }))
            .await
    }

    /// The reported-ness and `EndBillingPeriod` of one resource in whatever read the
    /// `extra` members describe.
    async fn in_period(
        &self,
        target: &str,
        resource: &str,
        extra: Value,
    ) -> Option<Option<String>> {
        let (status, body) = self.associated(target, extra).await;
        assert_eq!(
            status, 200,
            "case read of {target}: expected HTTP 200, got {status} with {body}"
        );
        elements(&body)
            .into_iter()
            .find(|(arn, _, _)| arn == resource)
            .map(|(_, _, end)| end)
    }
}

/// The `Arn` a create answered with.
fn arn_of(body: &Value, label: &str) -> String {
    body.get("Arn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {label}: expected an Arn member, got {body}"))
        .to_string()
}

/// The `(Arn, Relationship, EndBillingPeriod)` triples of an `AssociatedResources`
/// list.
fn elements(body: &Value) -> Vec<(String, String, Option<String>)> {
    body.get("AssociatedResources")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|element| {
            (
                element
                    .get("Arn")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                element
                    .get("Relationship")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                element
                    .get("EndBillingPeriod")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            )
        })
        .collect()
}

/// The ARNs of one of a batch response's lists, in response order.
fn reported(body: &Value, member: &str) -> Vec<String> {
    body.get(member)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|element| element.get("Arn").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

/// The `(Arn, Error.Reason)` pairs of a batch response's failure list.
fn failures(body: &Value, member: &str) -> Vec<(String, String)> {
    body.get(member)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|element| {
            (
                element
                    .get("Arn")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                element
                    .get("Error")
                    .and_then(|error| error.get("Reason"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}

/// The bare resource id of a full ARN — the OTHER form every association member
/// admits (`^(arn:...:(customlineitem|billinggroup)/)?[a-zA-Z0-9]{10,12}$`).
fn bare_id(arn: &str) -> String {
    arn.rsplit('/').next().unwrap_or(arn).to_string()
}

/// Every class of resource an association can name, in both the forms the member
/// admits, in requests of one and of several resources: what is disassociated is the
/// resource, not the spelling, and a request with no billing period range disassociates
/// it in every period.
#[tokio::test]
async fn a_disassociation_drops_the_association_however_the_resource_is_named() {
    // (case, how many billing groups to use, how many custom line items to use, name
    // them by bare id rather than by full ARN)
    let cases: [(&str, usize, usize, bool); 7] = [
        ("one billing group by ARN", 1, 0, false),
        ("one billing group by bare id", 1, 0, true),
        ("one custom line item by ARN", 0, 1, false),
        ("one custom line item by bare id", 0, 1, true),
        ("two of each class by ARN", 2, 2, false),
        ("two of each class by bare id", 2, 2, true),
        ("three billing groups by ARN", 3, 0, false),
    ];

    for (case, groups, items, by_id) in cases {
        let emulator = Emulator::new();
        let home = emulator.billing_group("home").await;
        let target = emulator.percentage_item(&home, "target").await;

        let mut requested: Vec<String> = Vec::new();
        let mut associated: Vec<(String, String)> = Vec::new();
        for index in 0..groups {
            let arn = emulator.billing_group(&format!("group{index}")).await;
            requested.push(if by_id { bare_id(&arn) } else { arn.clone() });
            associated.push((arn, "PARENT".to_string()));
        }
        for index in 0..items {
            let arn = emulator
                .percentage_item(&home, &format!("item{index}"))
                .await;
            requested.push(if by_id { bare_id(&arn) } else { arn.clone() });
            associated.push((arn, "CHILD".to_string()));
        }
        emulator.associate(&target, &requested, None).await;
        associated.sort();
        assert_eq!(
            emulator.association_pairs(&target, json!({})).await,
            associated,
            "case {case:?}: expected the associations to be in place before the disassociation"
        );

        let (status, body) = emulator.disassociate(&target, &requested, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {body}"
        );
        assert_eq!(
            reported(&body, "SuccessfullyDisassociatedResources"),
            requested,
            "case {case:?}: expected every resource reported disassociated as the caller named \
             it, got {body}"
        );
        assert!(
            failures(&body, "FailedDisassociatedResources").is_empty(),
            "case {case:?}: expected no failed resource, got {:?}",
            failures(&body, "FailedDisassociatedResources")
        );

        let left = emulator.association_pairs(&target, json!({})).await;
        assert!(
            left.is_empty(),
            "case {case:?}: expected no association left, got {left:?}"
        );
    }
}

/// Only the resources the request names are disassociated: the item keeps the rest.
#[tokio::test]
async fn a_disassociation_leaves_the_associations_it_does_not_name() {
    let emulator = Emulator::new();
    let home = emulator.billing_group("home").await;
    let target = emulator.percentage_item(&home, "target").await;
    let kept = emulator.billing_group("kept").await;
    let dropped = emulator.percentage_item(&home, "dropped").await;
    emulator
        .associate(&target, &[kept.clone(), dropped.clone()], None)
        .await;

    // The resource to drop, in each form the member admits.
    for named in [dropped.clone(), bare_id(&dropped)] {
        let (status, body) = emulator
            .disassociate(&target, std::slice::from_ref(&named), None)
            .await;
        assert_eq!(
            status, 200,
            "case dropping {named:?}: expected HTTP 200, got {status} with {body}"
        );
        assert_eq!(
            reported(&body, "SuccessfullyDisassociatedResources"),
            vec![named.clone()],
            "case dropping {named:?}: expected the resource reported disassociated, got {body}"
        );
        assert_eq!(
            emulator.association_pairs(&target, json!({})).await,
            vec![(kept.clone(), "PARENT".to_string())],
            "case dropping {named:?}: expected the association the request did not name to be \
             kept"
        );
    }
}

/// `@idempotent`: a resource the item holds no association for is already
/// disassociated, so its element succeeds and nothing changes — whether it was never
/// associated, was named twice in one request, or the same request is sent again.
#[tokio::test]
async fn disassociating_a_resource_the_item_does_not_hold_succeeds_and_changes_nothing() {
    let emulator = Emulator::new();
    let home = emulator.billing_group("home").await;
    let target = emulator.percentage_item(&home, "target").await;
    let kept = emulator.billing_group("kept").await;
    let never = emulator.billing_group("never").await;
    let once = emulator.percentage_item(&home, "once").await;
    emulator
        .associate(&target, &[kept.clone(), once.clone()], None)
        .await;

    // (case, the resources the request names, the associations left afterwards)
    let rows: [(&str, Vec<String>, Vec<String>); 5] = [
        (
            "a resource that was never associated",
            vec![never.clone()],
            vec![kept.clone(), once.clone()],
        ),
        (
            "the same resource twice in one request",
            vec![once.clone(), once.clone()],
            vec![kept.clone()],
        ),
        (
            "the same request again",
            vec![once.clone()],
            vec![kept.clone()],
        ),
        (
            "the same resource by bare id",
            vec![bare_id(&once)],
            vec![kept.clone()],
        ),
        (
            "a never-associated resource beside an already-disassociated one",
            vec![never.clone(), once.clone()],
            vec![kept.clone()],
        ),
    ];

    for (case, resources, left) in rows {
        let (status, body) = emulator.disassociate(&target, &resources, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {body}"
        );
        assert_eq!(
            reported(&body, "SuccessfullyDisassociatedResources"),
            resources,
            "case {case:?}: expected every resource reported disassociated, got {body}"
        );
        assert!(
            failures(&body, "FailedDisassociatedResources").is_empty(),
            "case {case:?}: an association that does not hold is not a failure, got {:?}",
            failures(&body, "FailedDisassociatedResources")
        );
        let mut expected: Vec<(String, String)> = left
            .iter()
            .map(|arn| {
                let relationship = if arn.contains(":billinggroup/") {
                    "PARENT"
                } else {
                    "CHILD"
                };
                (arn.clone(), relationship.to_string())
            })
            .collect();
        expected.sort();
        assert_eq!(
            emulator.association_pairs(&target, json!({})).await,
            expected,
            "case {case:?}: expected the associations the request did not disassociate to be left"
        );
    }
}

/// A resource that cannot be disassociated fails as ITS OWN element, classified by the
/// modelled `AssociateResourceErrorReason`, while the other resources of the same
/// request are still disassociated.
#[tokio::test]
async fn a_resource_that_cannot_be_disassociated_fails_as_its_own_element() {
    // `AssociateResourceErrorReason` values from
    // `smithy_query(query="shape",shape="AssociateResourceErrorReason")`.
    // (case, the resource the request names, the reason its element reports)
    let cases: [(&str, Option<&str>, &str); 5] = [
        ("an unknown bare id", Some("ZZZZZZZZZZ"), "INVALID_ARN"),
        (
            "an unknown custom line item ARN",
            Some("arn:aws:billingconductor::000000000000:customlineitem/ZZZZZZZZZZ"),
            "INVALID_ARN",
        ),
        (
            "an unknown billing group ARN",
            Some("arn:aws:billingconductor::000000000000:billinggroup/ZZZZZZZZZZZZ"),
            "INVALID_ARN",
        ),
        // `None` stands for the target itself, which is minted per case.
        ("the target itself", None, "ILLEGAL_CUSTOMLINEITEM"),
        ("the target itself by bare id", None, "ILLEGAL_CUSTOMLINEITEM"),
    ];

    for (case, resource, reason) in cases {
        let emulator = Emulator::new();
        let home = emulator.billing_group("home").await;
        let target = emulator.percentage_item(&home, "target").await;
        let good = emulator.billing_group("good").await;
        let other = emulator.billing_group("other").await;
        emulator
            .associate(&target, &[good.clone(), other.clone()], None)
            .await;

        let resource = match resource {
            Some(resource) => resource.to_string(),
            None if case.ends_with("bare id") => bare_id(&target),
            None => target.clone(),
        };

        let (status, body) = emulator
            .disassociate(&target, &[resource.clone(), good.clone()], None)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a per-resource failure is still a 200 batch answer, got {status} \
             with {body}"
        );
        assert_eq!(
            failures(&body, "FailedDisassociatedResources"),
            vec![(resource.clone(), reason.to_string())],
            "case {case:?}: expected {resource:?} reported failed with {reason}, got {body}"
        );
        assert_eq!(
            reported(&body, "SuccessfullyDisassociatedResources"),
            vec![good.clone()],
            "case {case:?}: the other resource of the same request is still disassociated, got \
             {body}"
        );
        assert_eq!(
            emulator.association_pairs(&target, json!({})).await,
            vec![(other.clone(), "PARENT".to_string())],
            "case {case:?}: expected only the resource the request succeeded on to be \
             disassociated"
        );
    }
}

/// A fault of the REQUEST is the call's own modelled error, not a per-resource one: an
/// unknown target is `ResourceNotFoundException` (404), a target that is not a
/// percentage custom line item and a billing period range that covers no period are
/// `ValidationException` (400) — and a refused call persists nothing.
#[tokio::test]
async fn a_request_level_fault_is_the_calls_own_modelled_error() {
    let emulator = Emulator::new();
    let home = emulator.billing_group("home").await;
    let target = emulator.percentage_item(&home, "target").await;
    let flat = emulator.flat_item(&home, "flat").await;
    let group = emulator.billing_group("group").await;
    emulator
        .associate(&target, std::slice::from_ref(&group), None)
        .await;

    // An unknown target, in both the forms `CustomLineItemArn` admits.
    for unknown in [
        "ZZZZZZZZZZ".to_string(),
        "arn:aws:billingconductor::000000000000:customlineitem/ZZZZZZZZZZ".to_string(),
    ] {
        let (status, body) = emulator
            .disassociate(&unknown, std::slice::from_ref(&group), None)
            .await;
        assert_eq!(
            status, 404,
            "case target {unknown:?}: expected HTTP 404, got {status} with {body}"
        );
        assert_eq!(
            body.get("__type").and_then(Value::as_str),
            Some("ResourceNotFoundException"),
            "case target {unknown:?}: expected the modelled not-found error, got {body}"
        );
    }

    // A flat custom line item has no associated resources to disassociate.
    for flat_target in [flat.clone(), bare_id(&flat)] {
        let (status, body) = emulator
            .disassociate(&flat_target, std::slice::from_ref(&group), None)
            .await;
        assert_eq!(
            status, 400,
            "case flat target {flat_target:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            body.get("__type").and_then(Value::as_str),
            Some("ValidationException"),
            "case flat target {flat_target:?}: expected the modelled validation error, got {body}"
        );
        assert_eq!(
            body.get("Reason").and_then(Value::as_str),
            Some("ILLEGAL_CUSTOMLINEITEM"),
            "case flat target {flat_target:?}: expected the modelled reason, got {body}"
        );
    }

    // The start is inclusive and the end exclusive, so a range whose end is at or
    // before its start covers no billing period — in either spelling of a month.
    let empty_ranges = [
        ("the same period", "2026-3", "2026-3"),
        ("the same period, padded", "2026-03", "2026-3"),
        ("an earlier end", "2026-3", "2026-2"),
        ("an earlier year", "2026-1", "2025-12"),
    ];
    for (case, start, end) in empty_ranges {
        let (status, body) = emulator
            .disassociate(
                &target,
                std::slice::from_ref(&group),
                Some(json!({
                    "InclusiveStartBillingPeriod": start,
                    "ExclusiveEndBillingPeriod": end,
                })),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?} ({start}..{end}): expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            body.get("Reason").and_then(Value::as_str),
            Some("ILLEGAL_BILLING_PERIOD_RANGE"),
            "case {case:?} ({start}..{end}): expected the modelled reason, got {body}"
        );
    }

    // Nothing was disassociated by any refused call.
    assert_eq!(
        emulator.association_pairs(&target, json!({})).await,
        vec![(group.clone(), "PARENT".to_string())],
        "case after the refusals: a refused call persists nothing"
    );
}

/// One row of the billing-period-range table: what the association was made with,
/// where the disassociation takes effect, and what is left.
struct RangeRow {
    case: &'static str,
    /// The `BillingPeriodRange` of the ASSOCIATING call, if any.
    associated: Option<Value>,
    /// `InclusiveStartBillingPeriod` of the DISASSOCIATING call.
    disassociated_from: &'static str,
    /// The association the read reports afterwards: `None` when it was dropped,
    /// `Some(end)` with the `EndBillingPeriod` it now carries.
    left: Option<Option<&'static str>>,
    /// Billing periods a read asks for, and whether the association is in effect in
    /// each: each published bound of what is left, and one period past it.
    probes: &'static [(&'static str, bool)],
}

/// A disassociation bounded to a billing period range takes effect FROM its inclusive
/// start: the association keeps the earlier periods it applied in and ends there, and
/// is dropped only when it would otherwise cover no period at all.
#[tokio::test]
async fn a_ranged_disassociation_ends_the_association_at_its_inclusive_start() {
    let rows: [RangeRow; 7] = [
        RangeRow {
            case: "an association with no range at all",
            associated: None,
            disassociated_from: "2026-6",
            left: Some(Some("2026-6")),
            probes: &[("2026-5", true), ("2026-6", false), ("2026-7", false)],
        },
        RangeRow {
            case: "an open-ended association ended later",
            associated: Some(json!({ "InclusiveStartBillingPeriod": "2026-1" })),
            disassociated_from: "2026-4",
            left: Some(Some("2026-4")),
            probes: &[
                ("2025-12", false),
                ("2026-1", true),
                ("2026-3", true),
                ("2026-4", false),
            ],
        },
        RangeRow {
            case: "a disassociation inside the association's own range",
            associated: Some(json!({
                "InclusiveStartBillingPeriod": "2026-1",
                "ExclusiveEndBillingPeriod": "2026-6",
            })),
            disassociated_from: "2026-3",
            left: Some(Some("2026-3")),
            probes: &[("2026-2", true), ("2026-3", false), ("2026-5", false)],
        },
        RangeRow {
            case: "a disassociation in the padded spelling of a month",
            associated: Some(json!({ "InclusiveStartBillingPeriod": "2026-1" })),
            disassociated_from: "2026-04",
            left: Some(Some("2026-04")),
            probes: &[("2026-3", true), ("2026-4", false), ("2026-04", false)],
        },
        RangeRow {
            case: "a disassociation at the association's own start",
            associated: Some(json!({ "InclusiveStartBillingPeriod": "2026-4" })),
            disassociated_from: "2026-4",
            left: None,
            probes: &[("2026-4", false), ("2026-5", false)],
        },
        RangeRow {
            case: "a disassociation before the association's start",
            associated: Some(json!({ "InclusiveStartBillingPeriod": "2026-5" })),
            disassociated_from: "2026-2",
            left: None,
            probes: &[("2026-5", false), ("2026-6", false)],
        },
        RangeRow {
            case: "a disassociation after the association already ends",
            associated: Some(json!({
                "InclusiveStartBillingPeriod": "2026-1",
                "ExclusiveEndBillingPeriod": "2026-3",
            })),
            disassociated_from: "2026-6",
            left: Some(Some("2026-3")),
            probes: &[("2026-2", true), ("2026-3", false), ("2026-6", false)],
        },
    ];

    for row in rows {
        let emulator = Emulator::new();
        let home = emulator.billing_group("home").await;
        let target = emulator.percentage_item(&home, "target").await;
        let resource = emulator.billing_group("resource").await;
        emulator
            .associate(
                &target,
                std::slice::from_ref(&resource),
                row.associated.clone(),
            )
            .await;

        // The same disassociation TWICE: `@idempotent`, so the second call leaves what
        // the first one did.
        for attempt in 1..=2 {
            let case = format!("{} (attempt {attempt})", row.case);
            let (status, body) = emulator
                .disassociate(
                    &target,
                    std::slice::from_ref(&resource),
                    Some(json!({ "InclusiveStartBillingPeriod": row.disassociated_from })),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with {body}"
            );
            assert_eq!(
                reported(&body, "SuccessfullyDisassociatedResources"),
                vec![resource.clone()],
                "case {case:?}: expected the resource reported disassociated, got {body}"
            );

            let end = emulator.end_billing_period(&target, &resource).await;
            assert_eq!(
                end,
                row.left.map(|end| end.map(str::to_string)),
                "case {case:?}: expected the association left as {:?}, got {end:?}",
                row.left
            );

            for (period, in_effect) in row.probes {
                let reported_back = emulator
                    .association_pairs(&target, json!({ "BillingPeriod": period }))
                    .await;
                let arns: Vec<String> = reported_back
                    .iter()
                    .map(|(arn, _)| arn.clone())
                    .collect();
                assert_eq!(
                    arns.contains(&resource),
                    *in_effect,
                    "case {case:?}: expected the association to be {} in {period}, got {arns:?}",
                    if *in_effect { "in effect" } else { "gone" }
                );
            }
        }
    }
}

/// One row of the bounded-range table: the billing periods the association was made
/// over, the half-open range the disassociation names, and what a read of each probed
/// billing period reports afterwards.
struct BoundedRow {
    case: &'static str,
    /// The `BillingPeriodRange` of the ASSOCIATING call, if any — `None` associates the
    /// resource in every billing period.
    associated: Option<Value>,
    /// `InclusiveStartBillingPeriod` of the DISASSOCIATING call.
    disassociated_from: &'static str,
    /// `ExclusiveEndBillingPeriod` of the DISASSOCIATING call — the bound this table is
    /// about.
    disassociated_until: &'static str,
    /// Per billing period: `None` when a read of that period does not report the
    /// resource at all, `Some(end)` when it does, carrying that `EndBillingPeriod`.
    probes: &'static [(&'static str, Option<Option<&'static str>>)],
    /// Is the resource still associated AT ALL, i.e. does a read that names no billing
    /// period report it?
    left: bool,
}

/// A disassociation bounded by a `BillingPeriodRange` applies ONLY to the billing
/// periods that half-open range covers: the association keeps every period outside it,
/// so it resumes at the range's `ExclusiveEndBillingPeriod`, and it is dropped only when
/// nothing of it is left.
///
/// The rows differ along where the request's window falls relative to the association's
/// own range — strictly inside it, overlapping either end, covering it, disjoint on
/// either side — along the association's own bounds (open, open-ended, bounded), along
/// the width of the window (one period, several, across the turn of a year) and along
/// the spelling of the months (`2026-4` / `2026-04`), with each row run for a billing
/// group named by full ARN and a custom line item named by bare id in ONE request.
#[tokio::test]
async fn a_bounded_disassociation_removes_only_the_periods_its_range_covers() {
    let rows: [BoundedRow; 11] = [
        BoundedRow {
            case: "an open association, a window inside it",
            associated: None,
            disassociated_from: "2026-1",
            disassociated_until: "2026-3",
            probes: &[
                ("2025-12", Some(Some("2026-1"))),
                ("2026-1", None),
                ("2026-2", None),
                ("2026-3", Some(None)),
                ("2026-4", Some(None)),
            ],
            left: true,
        },
        BoundedRow {
            case: "an open-ended association, a window inside it",
            associated: Some(json!({ "InclusiveStartBillingPeriod": "2026-1" })),
            disassociated_from: "2026-3",
            disassociated_until: "2026-5",
            probes: &[
                ("2025-12", None),
                ("2026-1", Some(Some("2026-3"))),
                ("2026-2", Some(Some("2026-3"))),
                ("2026-3", None),
                ("2026-4", None),
                ("2026-5", Some(None)),
                ("2026-11", Some(None)),
            ],
            left: true,
        },
        BoundedRow {
            case: "a bounded association, a window inside it",
            associated: Some(json!({
                "InclusiveStartBillingPeriod": "2026-1",
                "ExclusiveEndBillingPeriod": "2026-6",
            })),
            disassociated_from: "2026-2",
            disassociated_until: "2026-4",
            probes: &[
                ("2026-1", Some(Some("2026-2"))),
                ("2026-2", None),
                ("2026-3", None),
                ("2026-4", Some(Some("2026-6"))),
                ("2026-5", Some(Some("2026-6"))),
                ("2026-6", None),
            ],
            left: true,
        },
        BoundedRow {
            case: "a window overlapping the association's start",
            associated: Some(json!({
                "InclusiveStartBillingPeriod": "2026-3",
                "ExclusiveEndBillingPeriod": "2026-9",
            })),
            disassociated_from: "2026-1",
            disassociated_until: "2026-5",
            probes: &[
                ("2026-2", None),
                ("2026-3", None),
                ("2026-4", None),
                ("2026-5", Some(Some("2026-9"))),
                ("2026-8", Some(Some("2026-9"))),
                ("2026-9", None),
            ],
            left: true,
        },
        BoundedRow {
            case: "a window overlapping the association's end",
            associated: Some(json!({
                "InclusiveStartBillingPeriod": "2026-3",
                "ExclusiveEndBillingPeriod": "2026-9",
            })),
            disassociated_from: "2026-7",
            disassociated_until: "2026-12",
            probes: &[
                ("2026-3", Some(Some("2026-7"))),
                ("2026-6", Some(Some("2026-7"))),
                ("2026-7", None),
                ("2026-8", None),
                ("2026-11", None),
            ],
            left: true,
        },
        BoundedRow {
            case: "a window covering the whole association",
            associated: Some(json!({
                "InclusiveStartBillingPeriod": "2026-3",
                "ExclusiveEndBillingPeriod": "2026-5",
            })),
            disassociated_from: "2026-1",
            disassociated_until: "2026-9",
            probes: &[
                ("2026-1", None),
                ("2026-3", None),
                ("2026-4", None),
                ("2026-9", None),
            ],
            left: false,
        },
        BoundedRow {
            case: "a window entirely before the association",
            associated: Some(json!({ "InclusiveStartBillingPeriod": "2026-5" })),
            disassociated_from: "2026-1",
            disassociated_until: "2026-3",
            probes: &[
                ("2026-1", None),
                ("2026-4", None),
                ("2026-5", Some(None)),
                ("2026-9", Some(None)),
            ],
            left: true,
        },
        BoundedRow {
            case: "a window entirely after the association ends",
            associated: Some(json!({
                "InclusiveStartBillingPeriod": "2026-1",
                "ExclusiveEndBillingPeriod": "2026-3",
            })),
            disassociated_from: "2026-6",
            disassociated_until: "2026-8",
            probes: &[
                ("2026-1", Some(Some("2026-3"))),
                ("2026-2", Some(Some("2026-3"))),
                ("2026-3", None),
                ("2026-6", None),
            ],
            left: true,
        },
        BoundedRow {
            case: "a window in the padded spelling of its months",
            associated: None,
            disassociated_from: "2026-04",
            disassociated_until: "2026-06",
            probes: &[
                ("2026-3", Some(Some("2026-04"))),
                ("2026-03", Some(Some("2026-04"))),
                ("2026-4", None),
                ("2026-04", None),
                ("2026-5", None),
                ("2026-6", Some(None)),
                ("2026-06", Some(None)),
            ],
            left: true,
        },
        BoundedRow {
            case: "a window of one billing period",
            associated: None,
            disassociated_from: "2026-5",
            disassociated_until: "2026-6",
            probes: &[
                ("2026-4", Some(Some("2026-5"))),
                ("2026-5", None),
                ("2026-6", Some(None)),
            ],
            left: true,
        },
        BoundedRow {
            case: "a window across the turn of the year",
            associated: None,
            disassociated_from: "2025-12",
            disassociated_until: "2026-2",
            probes: &[
                ("2025-11", Some(Some("2025-12"))),
                ("2025-12", None),
                ("2026-1", None),
                ("2026-2", Some(None)),
            ],
            left: true,
        },
    ];

    for row in rows {
        let emulator = Emulator::new();
        let home = emulator.billing_group("home").await;
        let target = emulator.percentage_item(&home, "target").await;
        // Both classes of resource an association can name, each in one of the two
        // forms the member admits, disassociated in ONE request.
        let group = emulator.billing_group("resource").await;
        let child = emulator.percentage_item(&home, "child").await;
        let requested = vec![group.clone(), bare_id(&child)];
        emulator
            .associate(&target, &requested, row.associated.clone())
            .await;

        // The same bounded disassociation TWICE: `@idempotent`, so the second call
        // leaves exactly what the first one did.
        for attempt in 1..=2 {
            let case = format!("{} (attempt {attempt})", row.case);
            let (status, body) = emulator
                .disassociate(
                    &target,
                    &requested,
                    Some(json!({
                        "InclusiveStartBillingPeriod": row.disassociated_from,
                        "ExclusiveEndBillingPeriod": row.disassociated_until,
                    })),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with {body}"
            );
            assert_eq!(
                reported(&body, "SuccessfullyDisassociatedResources"),
                requested,
                "case {case:?}: expected both resources reported disassociated as the caller \
                 named them, got {body}"
            );

            for resource in [&group, &child] {
                let whole = emulator.end_billing_period(&target, resource).await;
                assert_eq!(
                    whole.is_some(),
                    row.left,
                    "case {case:?} ({resource}): expected the association to be {} after the \
                     bounded disassociation, got {whole:?}",
                    if row.left { "left" } else { "dropped" }
                );

                for (period, expected) in row.probes {
                    let reported_back = emulator
                        .in_billing_period(&target, resource, period)
                        .await;
                    assert_eq!(
                        reported_back,
                        expected.map(|end| end.map(str::to_string)),
                        "case {case:?} ({resource}) in {period}: expected {expected:?}, got \
                         {reported_back:?}"
                    );
                }
            }
        }
    }
}

/// The `BillingPeriod` a month index names, in the form the modelled pattern admits
/// (`^\d{4}-(0?[1-9]|1[012])$`): consecutive indices are consecutive months, so a
/// window `[period(k), period(k + 1))` is exactly one billing period.
fn period(month_index: usize) -> String {
    format!("{}-{}", 2000 + month_index / 12, month_index % 12 + 1)
}

/// Each bounded disassociation can carve one more separate stretch of billing periods
/// out of an association, and a caller decides how many to make — so the store holds at
/// most `MAX_BILLING_PERIOD_SPANS_PER_ASSOCIATION` of them, and the call that would
/// carve one too many fails THAT resource with the modelled `SERVICE_LIMIT_EXCEEDED`
/// reason and writes nothing, rather than growing without bound or coarsening what a
/// later read reports.
#[tokio::test]
async fn the_separate_ranges_of_one_association_are_bounded() {
    use service_com_amazonaws_billingconductor_awsbillingconductor::state::MAX_BILLING_PERIOD_SPANS_PER_ASSOCIATION as MAX_RANGES;

    let emulator = Emulator::new();
    let home = emulator.billing_group("home").await;
    let target = emulator.percentage_item(&home, "target").await;
    let resource = emulator.billing_group("resource").await;
    // Open-ended, so it starts as ONE stretch of billing periods.
    emulator
        .associate(&target, std::slice::from_ref(&resource), None)
        .await;

    // Every other month is removed, so each call punches a hole strictly inside a
    // stretch the association already has and splits it in two: the call made while the
    // association is in effect over `n` stretches would leave it in `n + 1`, so the one
    // that would take it past `MAX_RANGES` is refused.
    for hole in 0..MAX_RANGES {
        let start = period(hole * 2);
        let end = period(hole * 2 + 1);
        let at_limit = hole + 2 > MAX_RANGES;
        let case = format!("hole {hole} ({start}..{end})");
        let (status, body) = emulator
            .disassociate(
                &target,
                std::slice::from_ref(&resource),
                Some(json!({
                    "InclusiveStartBillingPeriod": start,
                    "ExclusiveEndBillingPeriod": end,
                })),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {body}"
        );
        if at_limit {
            assert_eq!(
                failures(&body, "FailedDisassociatedResources"),
                vec![(resource.clone(), "SERVICE_LIMIT_EXCEEDED".to_string())],
                "case {case:?}: expected the resource past the bound of {MAX_RANGES} ranges to \
                 fail with the modelled reason, got {body}"
            );
            assert!(
                reported(&body, "SuccessfullyDisassociatedResources").is_empty(),
                "case {case:?}: expected no resource reported disassociated, got {body}"
            );
            // Nothing was written, so the period the refused call named is still one
            // the association is in effect in.
            let reported_back = emulator.in_billing_period(&target, &resource, &start).await;
            assert_eq!(
                reported_back,
                Some(None),
                "case {case:?}: expected the refused call to write nothing, got {reported_back:?}"
            );
        } else {
            assert_eq!(
                reported(&body, "SuccessfullyDisassociatedResources"),
                vec![resource.clone()],
                "case {case:?}: expected the resource reported disassociated, got {body}"
            );
            assert!(
                failures(&body, "FailedDisassociatedResources").is_empty(),
                "case {case:?}: expected no failed resource, got {body}"
            );
            let removed = emulator.in_billing_period(&target, &resource, &start).await;
            assert_eq!(
                removed, None,
                "case {case:?}: expected the hole's own period to be disassociated, got \
                 {removed:?}"
            );
            let kept = emulator.in_billing_period(&target, &resource, &end).await;
            assert_eq!(
                kept,
                Some(None),
                "case {case:?}: expected the association to resume at the hole's exclusive end, \
                 got {kept:?}"
            );
        }
    }
}
