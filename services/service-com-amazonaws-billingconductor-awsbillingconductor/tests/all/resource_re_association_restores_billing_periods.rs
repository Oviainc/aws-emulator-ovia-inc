// SPDX-License-Identifier: CC0-1.0

//! What a SECOND `BatchAssociateResourcesToCustomLineItem` for a resource the item
//! already holds does to the billing periods that association is in effect over, end to
//! end over the wire: `CreateBillingGroup` -> `CreateCustomLineItem` ->
//! `BatchAssociateResourcesToCustomLineItem` ->
//! `BatchDisassociateResourcesFromCustomLineItem` ->
//! `BatchAssociateResourcesToCustomLineItem` ->
//! `ListResourcesAssociatedToCustomLineItem`.
//!
//! RULE: a resource's association is in effect over the UNION of the billing period
//! ranges the calls that associated it named — `BillingPeriodRange` is "the billing
//! period range in which the custom line item request will be applied", so a call puts
//! the periods it names in effect whether or not the item already holds a record for the
//! resource, and a call naming only periods already in effect writes nothing. The
//! resource is held by ONE record either way, so `AssociationSize` counts it once, and
//! `ListResourcesAssociatedToCustomLineItem` reports it exactly in the periods the union
//! covers, with the `EndBillingPeriod` of the stretch covering the period read.
//!
//! The rows differ along where the re-association's range falls relative to the stretch
//! a bounded disassociation took out of the association: over exactly it, over part of
//! it, over periods already in effect, over periods before the association began, open
//! ended, and absent altogether — and along whether anything of the association is left
//! for the re-association to extend.

use crate::custom_line_item_associations_accumulate::association_size;
use crate::percentage_custom_line_item_resource_associations::{
    failures, period, reported, Emulator,
};
use serde_json::{json, Value};

/// `BatchDisassociateResourcesFromCustomLineItem` over one resource, asserted to
/// succeed: the bounded call that takes billing periods OUT of an association, which is
/// the state a later association has to be able to put back.
async fn disassociate(
    emulator: &Emulator,
    case: &str,
    target: &str,
    resource: &str,
    range: Option<Value>,
) {
    let mut body = json!({ "TargetArn": target, "ResourceArns": [resource] });
    if let Some(range) = range {
        body["BillingPeriodRange"] = range;
    }
    let (status, answered) = emulator
        .send(
            "PUT",
            "/batch-disassociate-resources-from-custom-line-item",
            &body,
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the disassociation to answer HTTP 200, got {status} with \
         {answered}"
    );
    assert_eq!(
        answered.get("FailedDisassociatedResources"),
        Some(&json!([])),
        "case {case:?}: expected no failed resource, got {answered}"
    );
}

/// Every `EndBillingPeriod` a read of `billing_period` reports for `resource`: one entry
/// per association the item holds for it there — so an item holding the resource twice
/// is two entries, and one holding it in no stretch covering that period is none.
async fn ends_in_period(
    emulator: &Emulator,
    case: &str,
    target: &str,
    resource: &str,
    billing_period: &str,
) -> Vec<Option<String>> {
    let (status, answered) = emulator
        .send(
            "POST",
            "/list-resources-associated-to-custom-line-item",
            &json!({ "Arn": target, "BillingPeriod": billing_period, "MaxResults": 100 }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the read of {billing_period} to answer HTTP 200, got {status} \
         with {answered}"
    );
    answered
        .get("AssociatedResources")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter(|element| element.get("Arn").and_then(Value::as_str) == Some(resource))
        .map(|element| {
            element
                .get("EndBillingPeriod")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect()
}

/// One row: the bounded disassociation that takes periods out of the association, the
/// re-association that follows it, and what a read of each probed billing period reports
/// afterwards.
struct Row {
    case: &'static str,
    /// The `BillingPeriodRange` of the disassociating call — `None` for the call that
    /// names none, which ends the association in every period.
    disassociated: Option<Value>,
    /// The `BillingPeriodRange` of the re-associating call — `None` for the call that
    /// names none, which is in effect from the period it lands in on.
    re_associated: Option<Value>,
    /// Billing periods as offsets from the current one, and what the read of each
    /// reports once the re-association has landed: `None` for no association at all,
    /// `Some(None)` for one with no `EndBillingPeriod`, `Some(Some(offset))` for one
    /// ending at the period `offset` months out.
    probes: &'static [(i64, Option<Option<i64>>)],
}

/// A re-association puts the billing periods its range names back in effect, on the one
/// record the item already holds for the resource.
#[tokio::test]
async fn a_re_association_puts_the_billing_periods_it_names_in_effect() {
    let rows: Vec<Row> = vec![
        Row {
            case: "the current period taken, re-associated with no range at all",
            disassociated: Some(json!({
                "InclusiveStartBillingPeriod": period(0),
                "ExclusiveEndBillingPeriod": period(1),
            })),
            re_associated: None,
            probes: &[
                (-1, None),
                (0, Some(None)),
                (1, Some(None)),
                (3, Some(None)),
            ],
        },
        Row {
            case: "the current period taken, re-associated over exactly it",
            disassociated: Some(json!({
                "InclusiveStartBillingPeriod": period(0),
                "ExclusiveEndBillingPeriod": period(1),
            })),
            re_associated: Some(json!({
                "InclusiveStartBillingPeriod": period(0),
                "ExclusiveEndBillingPeriod": period(1),
            })),
            // The restored stretch runs on into the one that was left, so the
            // association has no end again.
            probes: &[(-1, None), (0, Some(None)), (1, Some(None))],
        },
        Row {
            case: "a later stretch taken, re-associated over its tail only",
            disassociated: Some(json!({
                "InclusiveStartBillingPeriod": period(1),
                "ExclusiveEndBillingPeriod": period(3),
            })),
            re_associated: Some(json!({
                "InclusiveStartBillingPeriod": period(2),
                "ExclusiveEndBillingPeriod": period(3),
            })),
            probes: &[
                (0, Some(Some(1))),
                (1, None),
                (2, Some(None)),
                (3, Some(None)),
            ],
        },
        Row {
            case: "a later stretch taken, re-associated over all of it",
            disassociated: Some(json!({
                "InclusiveStartBillingPeriod": period(1),
                "ExclusiveEndBillingPeriod": period(3),
            })),
            re_associated: Some(json!({
                "InclusiveStartBillingPeriod": period(1),
                "ExclusiveEndBillingPeriod": period(3),
            })),
            probes: &[
                (-1, None),
                (0, Some(None)),
                (1, Some(None)),
                (2, Some(None)),
                (3, Some(None)),
            ],
        },
        Row {
            case: "a later stretch taken, re-associated over periods already in effect",
            disassociated: Some(json!({
                "InclusiveStartBillingPeriod": period(2),
                "ExclusiveEndBillingPeriod": period(4),
            })),
            re_associated: Some(json!({
                "InclusiveStartBillingPeriod": period(1),
                "ExclusiveEndBillingPeriod": period(2),
            })),
            // Nothing to add, so the stretch the disassociation took stays taken.
            probes: &[
                (0, Some(Some(2))),
                (1, Some(Some(2))),
                (2, None),
                (3, None),
                (4, Some(None)),
            ],
        },
        Row {
            case: "the current period taken, re-associated over periods before the association",
            disassociated: Some(json!({
                "InclusiveStartBillingPeriod": period(0),
                "ExclusiveEndBillingPeriod": period(1),
            })),
            re_associated: Some(json!({
                "InclusiveStartBillingPeriod": period(-2),
                "ExclusiveEndBillingPeriod": period(-1),
            })),
            // A range sharing no period with what is left is a stretch of its own, and
            // it restores only the periods it names.
            probes: &[
                (-3, None),
                (-2, Some(Some(-1))),
                (-1, None),
                (0, None),
                (1, Some(None)),
            ],
        },
        Row {
            case: "every period taken, re-associated open ended from a later period",
            disassociated: None,
            re_associated: Some(json!({ "InclusiveStartBillingPeriod": period(2) })),
            probes: &[
                (0, None),
                (1, None),
                (2, Some(None)),
                (3, Some(None)),
            ],
        },
        Row {
            case: "every period taken, re-associated with no range at all",
            disassociated: None,
            re_associated: None,
            probes: &[(-1, None), (0, Some(None)), (1, Some(None))],
        },
    ];

    for row in rows {
        let emulator = Emulator::new();
        let home = emulator.billing_group("home").await;
        let target = emulator.percentage_item(&home, "target").await;
        // The resource: the billing group the target applies to, which is in scope of it.
        let resource = home.clone();

        // The association the first call makes: no range of its own, so it is in effect
        // from the current billing period on.
        let (status, body) = emulator
            .associate(&target, std::slice::from_ref(&resource), None)
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the first association to answer HTTP 200, got {status} with \
             {body}",
            row.case
        );

        disassociate(
            &emulator,
            row.case,
            &target,
            &resource,
            row.disassociated.clone(),
        )
        .await;

        // The periods the disassociation took are really out of effect before the
        // re-association: the start of its range, or the current period for the call
        // that named no range at all.
        let taken = row
            .disassociated
            .as_ref()
            .and_then(|range| range.get("InclusiveStartBillingPeriod"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| period(0));
        let left = ends_in_period(&emulator, row.case, &target, &resource, &taken).await;
        assert!(
            left.is_empty(),
            "case {:?}: expected {taken} to be out of effect after the disassociation, got \
             {left:?}",
            row.case
        );

        let (status, body) = emulator
            .associate(
                &target,
                std::slice::from_ref(&resource),
                row.re_associated.clone(),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the re-association to answer HTTP 200, got {status} with {body}",
            row.case
        );
        assert_eq!(
            reported(&body, "SuccessfullyAssociatedResources"),
            vec![resource.clone()],
            "case {:?}: expected the resource reported associated, got {body}",
            row.case
        );
        assert_eq!(
            failures(&body),
            Vec::new(),
            "case {:?}: expected no failed resource, got {body}",
            row.case
        );

        for &(offset, expected) in row.probes {
            let billing_period = period(offset);
            let reported_back =
                ends_in_period(&emulator, row.case, &target, &resource, &billing_period).await;
            assert!(
                reported_back.len() <= 1,
                "case {:?} ({billing_period}): expected the resource to be held by ONE \
                 association, got {reported_back:?}",
                row.case
            );
            let expected_end = expected.map(|end| end.map(period));
            let actual = reported_back.first().cloned();
            assert_eq!(
                actual, expected_end,
                "case {:?} ({billing_period}): expected the read to report {expected_end:?}, got \
                 {actual:?}",
                row.case
            );
        }

        // One record, so the derived `AssociationSize` — "the number of resources that
        // are associated to the custom line item" — counts the resource once.
        let size = association_size(&emulator, &target).await;
        assert_eq!(
            size,
            json!(1),
            "case {:?}: expected an AssociationSize of 1, got {size}",
            row.case
        );
    }
}

/// Each bounded disassociation can carve one more separate stretch of billing periods out
/// of an association, and the store holds at most `MAX_BILLING_PERIOD_SPANS` of them — so
/// a re-association whose range would be one stretch too many fails THAT resource with
/// the modelled `SERVICE_LIMIT_EXCEEDED` reason and writes nothing, while one that runs on
/// from a stretch the association already has adds no stretch and still succeeds.
#[tokio::test]
async fn a_re_association_past_the_bound_on_separate_ranges_is_refused() {
    use service_com_amazonaws_billingconductor_awsbillingconductor::state::MAX_BILLING_PERIOD_SPANS as MAX_RANGES;

    let emulator = Emulator::new();
    let home = emulator.billing_group("home").await;
    let target = emulator.percentage_item(&home, "target").await;
    let resource = home.clone();
    let (status, body) = emulator
        .associate(&target, std::slice::from_ref(&resource), None)
        .await;
    assert_eq!(
        status, 200,
        "case the first association: expected HTTP 200, got {status} with {body}"
    );

    // Every other month from the one after the current period is disassociated, so each
    // call punches a hole strictly inside a stretch the association already has and
    // splits it in two: after `MAX_RANGES - 1` holes the association is in effect over
    // exactly `MAX_RANGES` separate stretches.
    for hole in 0..MAX_RANGES - 1 {
        let offset = i64::try_from(hole).expect("a stretch index fits in i64");
        disassociate(
            &emulator,
            &format!("hole {hole}"),
            &target,
            &resource,
            Some(json!({
                "InclusiveStartBillingPeriod": period(1 + offset * 2),
                "ExclusiveEndBillingPeriod": period(2 + offset * 2),
            })),
        )
        .await;
    }

    // A range sharing no billing period with any stretch the association has would be one
    // stretch too many.
    let case = "a disjoint range past the bound";
    let (status, body) = emulator
        .associate(
            &target,
            std::slice::from_ref(&resource),
            Some(json!({
                "InclusiveStartBillingPeriod": period(-4),
                "ExclusiveEndBillingPeriod": period(-3),
            })),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {body}"
    );
    assert_eq!(
        failures(&body),
        vec![(resource.clone(), "SERVICE_LIMIT_EXCEEDED".to_string())],
        "case {case:?}: expected the resource past the bound of {MAX_RANGES} ranges to fail with \
         the modelled reason, got {body}"
    );
    assert!(
        reported(&body, "SuccessfullyAssociatedResources").is_empty(),
        "case {case:?}: expected no resource reported associated, got {body}"
    );
    let refused = ends_in_period(&emulator, case, &target, &resource, &period(-4)).await;
    assert!(
        refused.is_empty(),
        "case {case:?}: expected the refused call to write nothing, got {refused:?}"
    );

    // A range that fills the first hole joins the stretches on either side of it, so it
    // adds no stretch and is in effect afterwards.
    let case = "a range filling a hole at the bound";
    let (status, body) = emulator
        .associate(
            &target,
            std::slice::from_ref(&resource),
            Some(json!({
                "InclusiveStartBillingPeriod": period(1),
                "ExclusiveEndBillingPeriod": period(2),
            })),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {body}"
    );
    assert_eq!(
        failures(&body),
        Vec::new(),
        "case {case:?}: expected no failed resource, got {body}"
    );
    let filled = ends_in_period(&emulator, case, &target, &resource, &period(1)).await;
    assert_eq!(
        filled,
        vec![Some(period(3))],
        "case {case:?}: expected the filled hole to be in effect, ending where the next hole \
         begins, got {filled:?}"
    );
}
