// SPDX-License-Identifier: CC0-1.0

//! The ONE ordered-`BillingPeriodRange` refusal, driven through every operation that
//! takes a `CustomLineItemBillingPeriodRange`: `CreateCustomLineItem`,
//! `UpdateCustomLineItem`, `DeleteCustomLineItem`,
//! `BatchAssociateResourcesToCustomLineItem` and
//! `BatchDisassociateResourcesFromCustomLineItem`.
//!
//! RULE: the range's start is inclusive and its end EXCLUSIVE
//! (`smithy_query(query="shape",shape="CustomLineItemBillingPeriodRange")`), so a range
//! whose end is at or before its start covers no billing period at all and the request
//! is refused with the modelled 400 `ValidationException`, `Reason`
//! `ILLEGAL_BILLING_PERIOD_RANGE` and `Fields` naming
//! `BillingPeriodRange.ExclusiveEndBillingPeriod` — the SAME answer whichever of those
//! operations was called, because they all decide it through the one shared check.
//!
//! The rows differ along how the end misses the start (equal, equal in the other
//! spelling the `BillingPeriod` pattern allows, one month earlier, a year earlier) and
//! along the operation asked, and the ordered rows along the shape of a range that does
//! cover a period (bounded, open-ended) — never along a renaming.

use crate::percentage_custom_line_item_resource_associations::Emulator;
use serde_json::{json, Value};

/// The billing period `months` away from the service's own current one, zero padded as
/// the modelled `BillingPeriod` pattern `^\d{4}-(0?[1-9]|1[012])$` allows.
///
/// The periods an operation bounded to "the current, or previous billing period" may act
/// in are decided by the wall clock, so the rows that expect a range to be ACCEPTED name
/// their periods relative to that rather than by a fixed month.
fn period(months: i64) -> String {
    let current =
        service_com_amazonaws_billingconductor_awsbillingconductor::state::current_billing_period();
    let (year, month) = current
        .split_once('-')
        .expect("the current billing period is spelled YYYY-MM");
    let total = year.parse::<i64>().expect("a four-digit year") * 12
        + month.parse::<i64>().expect("a month")
        - 1
        + months;
    format!("{:04}-{:02}", total.div_euclid(12), total.rem_euclid(12) + 1)
}

/// A `CustomLineItemBillingPeriodRange` with the given bounds.
fn range(start: &str, end: Option<&str>) -> Value {
    let mut range = json!({ "InclusiveStartBillingPeriod": start });
    if let Some(end) = end {
        range["ExclusiveEndBillingPeriod"] = json!(end);
    }
    range
}

/// Every operation that takes a `CustomLineItemBillingPeriodRange`, as the name it is
/// modelled under and the answer it gives to a request carrying `range`.
async fn answers(
    emulator: &Emulator,
    group: &str,
    item: &str,
    resource: &str,
    range: &Value,
    name_suffix: &str,
) -> Vec<(&'static str, u16, Value)> {
    let mut answers = Vec::new();

    let (status, body) = emulator
        .send(
            "POST",
            "/create-custom-line-item",
            &json!({
                "Name": format!("ranged-{name_suffix}"),
                "Description": "a custom line item with a billing period range",
                "BillingGroupArn": group,
                "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 1.0 } },
                "BillingPeriodRange": range,
            }),
        )
        .await;
    answers.push(("CreateCustomLineItem", status, body));

    let (status, body) = emulator
        .send(
            "POST",
            "/update-custom-line-item",
            &json!({ "Arn": item, "BillingPeriodRange": range }),
        )
        .await;
    answers.push(("UpdateCustomLineItem", status, body));

    let (status, body) = emulator
        .send(
            "PUT",
            "/batch-associate-resources-to-custom-line-item",
            &json!({
                "TargetArn": item,
                "ResourceArns": [resource],
                "BillingPeriodRange": range,
            }),
        )
        .await;
    answers.push(("BatchAssociateResourcesToCustomLineItem", status, body));

    let (status, body) = emulator
        .send(
            "PUT",
            "/batch-disassociate-resources-from-custom-line-item",
            &json!({
                "TargetArn": item,
                "ResourceArns": [resource],
                "BillingPeriodRange": range,
            }),
        )
        .await;
    answers.push(("BatchDisassociateResourcesFromCustomLineItem", status, body));

    let (status, body) = emulator
        .send(
            "POST",
            "/delete-custom-line-item",
            &json!({ "Arn": item, "BillingPeriodRange": range }),
        )
        .await;
    answers.push(("DeleteCustomLineItem", status, body));

    answers
}

/// The names `ListCustomLineItems` reports, which is what says whether a refused
/// `CreateCustomLineItem` persisted an item.
async fn item_names(emulator: &Emulator, case: &str) -> Vec<String> {
    let (status, body) = emulator
        .send("POST", "/list-custom-line-items", &json!({}))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from the read back, got {status} with {body}"
    );
    body.get("CustomLineItems")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("Name").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn every_operation_refuses_a_range_that_covers_no_billing_period_the_same_way() {
    let emulator = Emulator::new();
    let group = emulator.billing_group("payer").await;
    let item = emulator.percentage_item(&group, "target").await;
    // A resource IN SCOPE of the billing group the target applies to — the group itself —
    // so the only thing that can refuse these calls is the range.
    let resource = group.clone();
    let before = item_names(&emulator, "before any refusal").await;

    // (case, start, end) — the end misses the start by being equal, equal in the other
    // spelling of the month, one month earlier, or a whole year earlier.
    let rows = [
        ("an end equal to the start", "2026-05", "2026-05"),
        ("equal bounds spelled both ways", "2026-5", "2026-05"),
        ("an end one month before the start", "2026-05", "2026-04"),
        ("an end a year before the start", "2026-01", "2025-12"),
        ("an end years before the start", "2026-05", "2019-12"),
    ];

    for (row, (case, start, end)) in rows.iter().enumerate() {
        let requested = range(start, Some(end));
        let answers = answers(
            &emulator,
            &group,
            &item,
            &resource,
            &requested,
            &format!("unordered-{row}"),
        )
        .await;

        for (operation, status, body) in &answers {
            let case = format!("{case} ({start}..{end}) at {operation}");
            assert_eq!(
                *status, 400,
                "case {case:?}: expected HTTP 400, got {status} with {body}"
            );
            assert_eq!(
                body.get("__type").and_then(Value::as_str),
                Some("ValidationException"),
                "case {case:?}: expected the modelled ValidationException, got {body}"
            );
            assert_eq!(
                body.get("Reason").and_then(Value::as_str),
                Some("ILLEGAL_BILLING_PERIOD_RANGE"),
                "case {case:?}: expected the modelled Reason, got {body}"
            );
            assert_eq!(
                body.get("Fields")
                    .and_then(Value::as_array)
                    .and_then(|fields| fields.first())
                    .and_then(|field| field.get("Name"))
                    .and_then(Value::as_str),
                Some("BillingPeriodRange.ExclusiveEndBillingPeriod"),
                "case {case:?}: expected Fields to name the member at fault, got {body}"
            );
            assert_eq!(
                body.get("Message").and_then(Value::as_str),
                Some(
                    "the exclusive end billing period must be later than the inclusive start \
                     billing period"
                ),
                "case {case:?}: expected the shared refusal sentence, got {body}"
            );
        }

        // One rule, one answer: the five operations differ in nothing but the operation
        // asked, which is what a single shared check guarantees and five copies do not.
        let (first_operation, first_status, first_body) = &answers[0];
        for (operation, status, body) in &answers[1..] {
            assert_eq!(
                (status, body),
                (first_status, first_body),
                "case {case:?} ({start}..{end}): {operation} must answer exactly as \
                 {first_operation} does, got {status} with {body}"
            );
        }

        // A refused call changes nothing: no item was created and none was deleted.
        assert_eq!(
            item_names(&emulator, case).await,
            before,
            "case {case:?} ({start}..{end}): a refused call must persist no change"
        );
    }
}

#[tokio::test]
async fn every_operation_accepts_a_range_that_does_cover_a_billing_period() {
    let emulator = Emulator::new();
    let group = emulator.billing_group("payer").await;
    // A resource IN SCOPE of the billing group the target applies to — the group itself —
    // so the only thing that can refuse these calls is the range.
    let resource = group.clone();

    // A range whose exclusive end is LATER than its inclusive start covers at least one
    // billing period, and so does a range with no end at all (the open future). The
    // starts are the current billing period, the one every one of these operations may
    // act in.
    let rows = [
        ("a one-period range", range(&period(0), Some(&period(1)))),
        ("a multi-period range", range(&period(0), Some(&period(6)))),
        ("an open-ended range", range(&period(0), None)),
    ];

    for (row, (case, requested)) in rows.iter().enumerate() {
        // A fresh target per row, so one row's delete cannot decide the next row's
        // answer.
        let item = emulator
            .percentage_item(&group, &format!("target-{row}"))
            .await;
        let created_name = format!("ranged-ordered-{row}");
        let answers = answers(
            &emulator,
            &group,
            &item,
            &resource,
            requested,
            &format!("ordered-{row}"),
        )
        .await;

        for (operation, status, body) in &answers {
            let case = format!("{case} at {operation}");
            assert_eq!(
                *status, 200,
                "case {case:?}: a range covering a billing period is accepted, got {status} \
                 with {body}"
            );
            // Accepted means the operation DID its work, not merely that it answered 200.
            match *operation {
                "CreateCustomLineItem" => assert!(
                    !body
                        .get("Arn")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .is_empty(),
                    "case {case:?}: expected the created item's Arn, got {body}"
                ),
                "UpdateCustomLineItem" | "DeleteCustomLineItem" => assert!(
                    body.get("Arn")
                        .and_then(Value::as_str)
                        .is_some_and(|reported| reported.ends_with(&bare_id(&item))),
                    "case {case:?}: expected the answer to name the item {item} it \
                     addressed, got {body}"
                ),
                "BatchAssociateResourcesToCustomLineItem" => assert!(
                    reports(body, "SuccessfullyAssociatedResources", &resource),
                    "case {case:?}: expected {resource} among the associated resources, got \
                     {body}"
                ),
                "BatchDisassociateResourcesFromCustomLineItem" => assert!(
                    reports(body, "SuccessfullyDisassociatedResources", &resource),
                    "case {case:?}: expected {resource} among the disassociated resources, got \
                     {body}"
                ),
                other => panic!("case {case:?}: no expectation is stated for {other}"),
            }
        }

        // The created item is readable back, so the accepted range was PERSISTED rather
        // than only answered.
        assert!(
            item_names(&emulator, case).await.contains(&created_name),
            "case {case:?}: expected a read to report the item {created_name} the accepted \
             range created"
        );
    }
}

/// The bare 10-character id inside a custom line item ARN — the form the `Arn` an answer
/// reports may also be spelled in (`CustomLineItemArn` admits both).
fn bare_id(arn: &str) -> String {
    arn.rsplit('/').next().unwrap_or(arn).to_string()
}

/// Does `body`'s `member` list report `arn`?
fn reports(body: &Value, member: &str, arn: &str) -> bool {
    body.get(member)
        .and_then(Value::as_array)
        .is_some_and(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry.get("Arn").and_then(Value::as_str))
                .any(|reported| reported == arn)
        })
}
