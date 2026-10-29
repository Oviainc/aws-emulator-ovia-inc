// SPDX-License-Identifier: CC0-1.0

//! The ONE ordered-billing-period-range rule, driven through operations that carry the
//! range under DIFFERENT shapes: `GetBillingGroupCostReport` (a `BillingPeriodRange`,
//! both bounds required) and `CreateCustomLineItem` / `DeleteCustomLineItem` (a
//! `CustomLineItemBillingPeriodRange`).
//!
//! RULE: a billing period range's start bound is inclusive and its end bound EXCLUSIVE
//! (`smithy_query(query="shape",shape="BillingPeriodRange,CustomLineItemBillingPeriodRange")`),
//! so a range whose end is at or before its start covers no billing period at all and
//! the request is refused with the modelled 400 `ValidationException`, `Reason`
//! `ILLEGAL_BILLING_PERIOD_RANGE` and `Fields` naming the end member of the shape that
//! carried it — the SAME answer whichever operation was asked, whatever shape the range
//! arrived in, because they all decide it through the crate's one bounds-taking check.
//! A range whose end IS later than its start covers a period and is served.
//!
//! The rows differ along how the end misses the start (equal, equal in the other
//! spelling the `BillingPeriod` pattern allows, one month earlier, a year earlier, years
//! earlier) and along the operation asked — never along a renaming.

use crate::percentage_custom_line_item_resource_associations::{period, Emulator};
use serde_json::{json, Value};

/// The member path every one of these operations names in `Fields` for an empty range:
/// the end bound of the range the request carried.
const END_MEMBER: &str = "BillingPeriodRange.ExclusiveEndBillingPeriod";

/// The sentence the one shared check refuses an empty range with.
const EMPTY_RANGE_MESSAGE: &str =
    "the exclusive end billing period must be later than the inclusive start billing period";

/// A range with both bounds, the form `BillingPeriodRange` requires and
/// `CustomLineItemBillingPeriodRange` admits.
fn range(start: &str, end: &str) -> Value {
    json!({
        "InclusiveStartBillingPeriod": start,
        "ExclusiveEndBillingPeriod": end,
    })
}

/// How each operation that takes a billing period range answers a request carrying
/// `range`: the name it is modelled under, the status and the body.
async fn answers(
    emulator: &Emulator,
    group: &str,
    item: &str,
    range: &Value,
    name_suffix: &str,
) -> Vec<(&'static str, u16, Value)> {
    let mut answers = Vec::new();

    let (status, body) = emulator
        .send(
            "POST",
            "/get-billing-group-cost-report",
            &json!({ "Arn": group, "BillingPeriodRange": range }),
        )
        .await;
    answers.push(("GetBillingGroupCostReport", status, body));

    let (status, body) = emulator
        .send(
            "POST",
            "/create-custom-line-item",
            &json!({
                "Name": format!("shared-range-{name_suffix}"),
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
            "/delete-custom-line-item",
            &json!({ "Arn": item, "BillingPeriodRange": range }),
        )
        .await;
    answers.push(("DeleteCustomLineItem", status, body));

    answers
}

#[tokio::test]
async fn an_empty_range_earns_the_same_refusal_whatever_shape_carried_it() {
    let emulator = Emulator::new();
    let group = emulator.billing_group("shared").await;
    let item = emulator.percentage_item(&group, "target").await;

    // (case, start, end) — the end misses the start by being equal, equal in the other
    // spelling the `BillingPeriod` pattern `^\d{4}-(0?[1-9]|1[012])$` allows, one month
    // earlier, a year earlier, or years earlier.
    let rows = [
        ("an end equal to the start", "2026-05", "2026-05"),
        ("equal bounds spelled both ways", "2026-5", "2026-05"),
        ("an end one month before the start", "2026-05", "2026-04"),
        ("an end a year before the start", "2026-01", "2025-12"),
        ("an end years before the start", "2026-05", "2019-12"),
    ];

    for (row, (case, start, end)) in rows.iter().enumerate() {
        let requested = range(start, end);
        let answers = answers(
            &emulator,
            &group,
            &item,
            &requested,
            &format!("empty-{row}"),
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
                Some(END_MEMBER),
                "case {case:?}: expected Fields to name the member at fault, got {body}"
            );
            assert_eq!(
                body.get("Message").and_then(Value::as_str),
                Some(EMPTY_RANGE_MESSAGE),
                "case {case:?}: expected the shared refusal sentence, got {body}"
            );
        }

        // One rule, one refusal: the operations differ in nothing but the operation
        // asked, which is what deciding it in one place guarantees and copies do not.
        let (first_operation, first_status, first_body) = &answers[0];
        for (operation, status, body) in &answers[1..] {
            assert_eq!(
                (status, body),
                (first_status, first_body),
                "case {case:?} ({start}..{end}): {operation} must answer exactly as \
                 {first_operation} does, got {status} with {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_range_that_covers_a_billing_period_is_served_by_every_shape() {
    let emulator = Emulator::new();
    let group = emulator.billing_group("shared").await;

    // A range whose exclusive end is LATER than its inclusive start covers at least one
    // billing period. The starts are the current billing period, the one the operations
    // bounded to "the current, or previous billing period" may also act in, so the only
    // thing deciding these answers is the range.
    let rows = [
        ("a one-period range", period(0), period(1)),
        ("a two-period range", period(0), period(2)),
        (
            "a range starting in the previous period",
            period(-1),
            period(1),
        ),
    ];

    for (row, (case, start, end)) in rows.iter().enumerate() {
        let item = emulator
            .percentage_item(&group, &format!("target-{row}"))
            .await;
        let requested = range(start, end);
        let answers = answers(
            &emulator,
            &group,
            &item,
            &requested,
            &format!("covering-{row}"),
        )
        .await;

        for (operation, status, body) in &answers {
            let case = format!("{case} ({start}..{end}) at {operation}");
            assert_eq!(
                *status, 200,
                "case {case:?}: a range covering a billing period is served, got {status} \
                 with {body}"
            );
        }

        // Served means the operation DID its work: the report carries the rows of the
        // group it is of, and the writes name the item they addressed.
        let (_, _, report) = &answers[0];
        assert!(
            report
                .get("BillingGroupCostReportResults")
                .and_then(Value::as_array)
                .is_some_and(|results| !results.is_empty()),
            "case {case:?}: expected the report to carry a margin summary row, got {report}"
        );
        let (_, _, created) = &answers[1];
        assert!(
            !created
                .get("Arn")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .is_empty(),
            "case {case:?}: expected the created item's Arn, got {created}"
        );
    }
}
