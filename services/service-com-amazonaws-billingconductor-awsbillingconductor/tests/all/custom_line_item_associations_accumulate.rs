// SPDX-License-Identifier: CC0-1.0

//! What a percentage custom line item's resource membership ACCUMULATES to, end to
//! end over the wire: `CreateCustomLineItem` ->
//! `BatchAssociateResourcesToCustomLineItem` (once per call) ->
//! `ListResourcesAssociatedToCustomLineItem` and `ListCustomLineItems`.
//!
//! RULE: the resources one percentage custom line item holds accumulate across calls
//! and are bounded only by how many resources there are in scope of its billing group
//! to name — the `@length` maximum of an input list bounds the resources of ONE
//! request (`ResourceArns`, 1..30) or of ONE create
//! (`ChargeDetails.Percentage.AssociatedValues`, up to 5), never the membership the
//! item builds up, so no in-scope resource is refused with `SERVICE_LIMIT_EXCEEDED`
//! for a membership that is merely larger than one request can carry. What the
//! associating calls persisted is what `ListResourcesAssociatedToCustomLineItem`
//! reports back and what the derived `AssociationSize` counts.
//!
//! The rows differ along how the membership is built up: the number of resources the
//! create itself associates, how many later calls add resources, and how many each of
//! them adds — including a full `ResourceArns` request and a total past both input
//! lists' maxima.

use crate::percentage_custom_line_item_resource_associations::{
    failures, reported, Emulator, MAX_RESOURCES_PER_REQUEST,
};
use serde_json::{json, Value};

/// How many resources ONE `CreateCustomLineItem` can associate: the
/// `CustomLineItemAssociationsList` `@length` maximum on
/// `ChargeDetails.Percentage.AssociatedValues`
/// (`smithy_query(query="shape",shape="CustomLineItemAssociationsList")`: len 0..5).
const MAX_RESOURCES_PER_CREATE: usize = 5;

/// The membership one percentage custom line item accumulates is the union of every
/// call that associated to it, whatever each call carried.
#[tokio::test]
async fn resource_associations_accumulate_across_calls() {
    // (case, resources the CREATE associates, resources each later call adds)
    let cases: [(&str, usize, &[usize]); 5] = [
        ("one resource per call", 0, &[1, 1, 1]),
        (
            "the create's maximum, then one more call",
            MAX_RESOURCES_PER_CREATE,
            &[1],
        ),
        ("two calls of the create's maximum", 0, &[5, 5]),
        (
            "a full request, then one more call",
            0,
            &[MAX_RESOURCES_PER_REQUEST, 5],
        ),
        (
            "the create's maximum, then a full request",
            MAX_RESOURCES_PER_CREATE,
            &[MAX_RESOURCES_PER_REQUEST],
        ),
    ];

    for (case, created, added) in cases {
        let total = created + added.iter().sum::<usize>();
        let emulator = Emulator::new();
        let home = emulator.billing_group("home").await;

        // The resources in scope of the billing group the target applies to: that
        // group itself, and custom line items of that same group.
        let mut pool: Vec<String> = vec![home.clone()];
        while pool.len() < total {
            pool.push(
                emulator
                    .percentage_item(&home, &format!("item{}", pool.len()))
                    .await,
            );
        }

        // The target: a percentage item created with the first `created` resources
        // already associated, which is the membership the later calls add to.
        let target = emulator
            .custom_line_item(
                &home,
                "target",
                json!({
                    "Type": "FEE",
                    "Percentage": {
                        "PercentageValue": 10.0,
                        "AssociatedValues": &pool[..created],
                    },
                }),
            )
            .await;

        let mut next = created;
        for (call, size) in added.iter().enumerate() {
            let batch = pool[next..next + size].to_vec();
            next += size;
            let (status, body) = emulator.associate(&target, &batch, None).await;
            assert_eq!(
                status, 200,
                "case {case:?} call {call}: expected HTTP 200, got {status} with {body}"
            );
            assert_eq!(
                reported(&body, "SuccessfullyAssociatedResources"),
                batch,
                "case {case:?} call {call}: expected every resource of the request \
                 associated, got {body}"
            );
            assert_eq!(
                failures(&body),
                Vec::new(),
                "case {case:?} call {call}: expected no resource to fail, got {body}"
            );
        }

        // The membership a read reports is the union of the create and every call.
        let mut reported_back: Vec<String> = emulator
            .association_pairs(&target, json!({ "MaxResults": 100 }))
            .await
            .into_iter()
            .map(|(arn, _)| arn)
            .collect();
        reported_back.sort();
        let mut expected = pool[..total].to_vec();
        expected.sort();
        assert_eq!(
            reported_back, expected,
            "case {case:?}: expected {total} associations to be stored, got \
             {reported_back:?}"
        );

        // `AssociationSize` — "The number of resources that are associated to the
        // custom line item" — counts the same membership.
        assert_eq!(
            association_size(&emulator, &target).await,
            json!(total),
            "case {case:?}: expected an AssociationSize of {total}"
        );
    }
}

/// The `AssociationSize` `ListCustomLineItems` reports for one item.
///
/// `pub(crate)` so the sibling suites about the same membership count it rather than
/// reading the derived member a second way (see
/// `tests/all/resource_re_association_restores_billing_periods.rs`).
pub(crate) async fn association_size(emulator: &Emulator, target: &str) -> Value {
    let (status, body) = emulator
        .send(
            "POST",
            "/list-custom-line-items",
            &json!({ "MaxResults": 100, "Filters": { "Arns": [target] } }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case the read of {target}: expected HTTP 200, got {status} with {body}"
    );
    let items = body
        .get("CustomLineItems")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        items.len(),
        1,
        "case the read of {target}: expected exactly the target, got {body}"
    );
    items[0]
        .get("AssociationSize")
        .cloned()
        .unwrap_or(Value::Null)
}
