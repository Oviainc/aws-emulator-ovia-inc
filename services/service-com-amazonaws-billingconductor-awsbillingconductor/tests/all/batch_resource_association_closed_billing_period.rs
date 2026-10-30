// SPDX-License-Identifier: CC0-1.0

//! The billing periods a BATCH resource association may write, end to end over the
//! wire: `CreateBillingGroup` -> `CreateCustomLineItem` ->
//! `BatchAssociateResourcesToCustomLineItem` /
//! `BatchDisassociateResourcesFromCustomLineItem` ->
//! `ListResourcesAssociatedToCustomLineItem`.
//!
//! RULE: a request that writes the billing periods a `CustomLineItemBillingPeriodRange`
//! names takes effect from its `InclusiveStartBillingPeriod` on, so that period's bill
//! must still be open: a start before the PREVIOUS billing period names a bill that has
//! already been issued, and no write of this store restates one, so the request is
//! refused with the modelled 400 `ValidationException` carrying the
//! `ValidationExceptionReason` the model enumerates for a period an operation may not act
//! in (`INVALID_BILLING_PERIOD_FOR_OPERATION`) and the member at fault, and NOTHING is
//! written. The periods from the previous one on have no issued bill, so how far the
//! range reaches — the previous period, the current one, a period that has not been
//! billed at all, or no end at all — is the request's to choose.
//!
//! The rows span what that rule generalises over rather than one request: which
//! operation writes (associate / disassociate), how far outside the open bills a refused
//! start lies (one month, a year, a decade, the epoch month, the earliest month the
//! modelled `BillingPeriod` pattern `^\d{4}-(0?[1-9]|1[012])$` admits), how an admitted
//! start lies inside them (the previous period, the current one, a later one, the latest
//! month the pattern admits), how the range is bounded (open end, bounded inside the open
//! bills, bounded far ahead) and how the month is spelled (one-digit / zero-padded) —
//! never along a renaming.
//!
//! The three operations the model documents as acting "in the current, or previous
//! billing period" bound their own start further, to those two periods alone
//! (`tests/all/create_custom_line_item_billing_period_window.rs` and
//! `tests/all/update_custom_line_item_billing_period_window.rs` pin that window); what
//! every operation taking the shape shares is the refusal of a start whose bill is
//! closed, which the second test here drives through all five of them.

use crate::percentage_custom_line_item_resource_associations::{period, reported, Emulator};
use serde_json::{json, Value};

/// A `CustomLineItemBillingPeriodRange`, as the model shapes it
/// (`smithy_query(query="shape",shape="CustomLineItemBillingPeriodRange")`: `@required
/// InclusiveStartBillingPeriod`, optional `ExclusiveEndBillingPeriod`).
fn range(inclusive_start: &str, exclusive_end: Option<&str>) -> Value {
    let mut value = json!({ "InclusiveStartBillingPeriod": inclusive_start });
    if let Some(end) = exclusive_end {
        value["ExclusiveEndBillingPeriod"] = json!(end);
    }
    value
}

/// The same billing period with its month spelled without a leading zero — the other
/// spelling the modelled `BillingPeriod` pattern allows. A month from October on has only
/// the one spelling, so for those this is the value it was given.
fn one_digit_month(billing_period: &str) -> String {
    let (year, month) = billing_period
        .split_once('-')
        .expect("a billing period is spelled YYYY-MM");
    format!("{year}-{}", month.trim_start_matches('0'))
}

/// `BatchDisassociateResourcesFromCustomLineItem` over one resource, with the range the
/// case names.
async fn disassociate(
    emulator: &Emulator,
    target: &str,
    resource: &str,
    billing_period_range: &Value,
) -> (u16, Value) {
    emulator
        .send(
            "PUT",
            "/batch-disassociate-resources-from-custom-line-item",
            &json!({
                "TargetArn": target,
                "ResourceArns": [resource],
                "BillingPeriodRange": billing_period_range,
            }),
        )
        .await
}

/// Does a read scoped to `billing_period` report `resource` among the item's
/// associations?
async fn in_period(emulator: &Emulator, target: &str, resource: &str, billing_period: &str) -> bool {
    emulator
        .association_pairs(target, json!({ "BillingPeriod": billing_period }))
        .await
        .iter()
        .any(|(arn, _)| arn == resource)
}

/// The modelled refusal a range whose start names a closed bill earns, asserted in full:
/// the status, the error shape, the `Reason` the model enumerates for a period the
/// operation may not act in, the member at fault, and the sentence — which names the
/// period the request carried and the current billing period the service computed it
/// against.
fn assert_closed_bill_refusal(case: &str, status: u16, body: &Value, start: &str) {
    let current =
        service_com_amazonaws_billingconductor_awsbillingconductor::state::current_billing_period();
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {body}"
    );
    assert_eq!(
        body.get("__type").and_then(Value::as_str),
        Some("ValidationException"),
        "case {case:?}: expected the modelled ValidationException, got {body}"
    );
    assert_eq!(
        body.get("Reason").and_then(Value::as_str),
        Some("INVALID_BILLING_PERIOD_FOR_OPERATION"),
        "case {case:?}: expected the modelled Reason, got {body}"
    );
    assert_eq!(
        body.get("Fields")
            .and_then(Value::as_array)
            .and_then(|fields| fields.first())
            .and_then(|field| field.get("Name"))
            .and_then(Value::as_str),
        Some("BillingPeriodRange.InclusiveStartBillingPeriod"),
        "case {case:?}: expected Fields to name the member at fault, got {body}"
    );
    let expected = format!(
        "billing period {start} is neither the current billing period ({current}) nor the \
         previous one"
    );
    assert_eq!(
        body.get("Message").and_then(Value::as_str),
        Some(expected.as_str()),
        "case {case:?}: expected the refusal sentence {expected:?}, got {body}"
    );
}

/// One row: the range a batch association writes, and whether the bill of the period it
/// starts in is still open.
struct Row {
    case: &'static str,
    inclusive_start: String,
    exclusive_end: Option<String>,
    /// Is the start's bill still open, so that the write may take effect from it?
    open_bill: bool,
}

/// THE RULE: a batch association or disassociation whose range starts in a billing period
/// whose bill is closed is refused and writes nothing, while one starting in a period
/// whose bill is still open is served and is readable back for the periods its range
/// covers.
#[tokio::test]
async fn a_batch_association_whose_range_starts_in_a_closed_bill_is_refused_and_writes_nothing() {
    let rows: Vec<Row> = vec![
        Row {
            case: "one month before the previous billing period",
            inclusive_start: period(-2),
            exclusive_end: None,
            open_bill: false,
        },
        Row {
            // The start alone decides this: a range reaching into the open bills is
            // refused for where it BEGINS, so no period of it is written.
            case: "one month before the previous billing period, reaching into the open bills",
            inclusive_start: period(-2),
            exclusive_end: Some(period(1)),
            open_bill: false,
        },
        Row {
            case: "a year before the current billing period",
            inclusive_start: period(-12),
            exclusive_end: Some(period(-11)),
            open_bill: false,
        },
        Row {
            case: "a decade before the current billing period",
            inclusive_start: period(-120),
            exclusive_end: None,
            open_bill: false,
        },
        Row {
            case: "a closed month spelled without a leading zero",
            inclusive_start: one_digit_month(&period(-3)),
            exclusive_end: None,
            open_bill: false,
        },
        Row {
            // The month the `Instant` members this service records count from.
            case: "the epoch month",
            inclusive_start: "1970-01".to_string(),
            exclusive_end: Some("1971-01".to_string()),
            open_bill: false,
        },
        Row {
            // The earliest month the modelled `BillingPeriod` pattern admits.
            case: "the first month of year zero",
            inclusive_start: "0000-01".to_string(),
            exclusive_end: None,
            open_bill: false,
        },
        Row {
            case: "the previous billing period, the earliest bill still open",
            inclusive_start: period(-1),
            exclusive_end: None,
            open_bill: true,
        },
        Row {
            case: "the previous billing period, bounded to the current one",
            inclusive_start: period(-1),
            exclusive_end: Some(period(0)),
            open_bill: true,
        },
        Row {
            case: "the previous billing period spelled without a leading zero",
            inclusive_start: one_digit_month(&period(-1)),
            exclusive_end: None,
            open_bill: true,
        },
        Row {
            case: "the current billing period",
            inclusive_start: period(0),
            exclusive_end: Some(period(1)),
            open_bill: true,
        },
        Row {
            case: "a period that has not been billed yet",
            inclusive_start: period(3),
            exclusive_end: None,
            open_bill: true,
        },
        Row {
            case: "a year ahead of the current billing period",
            inclusive_start: period(12),
            exclusive_end: Some(period(24)),
            open_bill: true,
        },
        Row {
            // The latest month the modelled `BillingPeriod` pattern admits.
            case: "the last month of year 9999",
            inclusive_start: "9999-12".to_string(),
            exclusive_end: None,
            open_bill: true,
        },
    ];

    for row in rows {
        let emulator = Emulator::new();
        let group = emulator.billing_group("payer").await;
        let target = emulator.percentage_item(&group, "target").await;
        // A second resource in scope of the target's own billing group, so the only
        // thing that can refuse the association of it is the range.
        let child = emulator.percentage_item(&group, "child").await;
        let requested = range(&row.inclusive_start, row.exclusive_end.as_deref());
        let case = format!(
            "{} ({}..{})",
            row.case,
            row.inclusive_start,
            row.exclusive_end.clone().unwrap_or_else(|| "open".to_string())
        );

        // The state the refusals must leave untouched: the group associated from the
        // previous billing period on, the earliest period a write may name.
        let (status, body) = emulator
            .associate(
                &target,
                std::slice::from_ref(&group),
                Some(range(&period(-1), None)),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the setup association to answer HTTP 200, got {status} \
             with {body}"
        );
        for billing_period in [period(-1), period(0)] {
            assert!(
                in_period(&emulator, &target, &group, &billing_period).await,
                "case {case:?}: expected the setup association to be in effect in \
                 {billing_period}"
            );
        }

        // The DISASSOCIATION half: it rewrites the periods the range names, so a start
        // whose bill is closed is refused and the stored association is left as it was.
        let (status, body) = disassociate(&emulator, &target, &group, &requested).await;
        if row.open_bill {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the disassociation to answer HTTP 200, got {status} \
                 with {body}"
            );
            assert_eq!(
                reported(&body, "SuccessfullyDisassociatedResources"),
                vec![group.clone()],
                "case {case:?}: expected the resource reported disassociated, got {body}"
            );
        } else {
            assert_closed_bill_refusal(&case, status, &body, &row.inclusive_start);
            for billing_period in [period(-1), period(0)] {
                assert!(
                    in_period(&emulator, &target, &group, &billing_period).await,
                    "case {case:?}: expected the refused disassociation to leave the \
                     association in effect in {billing_period}"
                );
            }
        }

        // The ASSOCIATION half: it puts the periods the range names in effect, so a start
        // whose bill is closed is refused and no association is written at all.
        let (status, body) = emulator
            .associate(
                &target,
                std::slice::from_ref(&child),
                Some(requested.clone()),
            )
            .await;
        if row.open_bill {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the association to answer HTTP 200, got {status} with \
                 {body}"
            );
            assert_eq!(
                reported(&body, "SuccessfullyAssociatedResources"),
                vec![child.clone()],
                "case {case:?}: expected the resource reported associated, got {body}"
            );
            // ROUND TRIP: the written association is readable back for the period its
            // range starts in.
            assert!(
                in_period(&emulator, &target, &child, &row.inclusive_start).await,
                "case {case:?}: expected a read of {} to report the association the call \
                 wrote",
                row.inclusive_start
            );
        } else {
            assert_closed_bill_refusal(&case, status, &body, &row.inclusive_start);
            for billing_period in [
                row.inclusive_start.clone(),
                period(-1),
                period(0),
                period(1),
            ] {
                assert!(
                    !in_period(&emulator, &target, &child, &billing_period).await,
                    "case {case:?}: expected the refused association to write nothing, yet a \
                     read of {billing_period} reports it"
                );
            }
        }
    }
}

/// Every operation that takes a `CustomLineItemBillingPeriodRange`, as the name it is
/// modelled under and the answer it gives to a request carrying `requested`.
async fn answers(
    emulator: &Emulator,
    group: &str,
    item: &str,
    resource: &str,
    requested: &Value,
) -> Vec<(&'static str, u16, Value)> {
    let mut answers = Vec::new();

    let (status, body) = emulator
        .send(
            "POST",
            "/create-custom-line-item",
            &json!({
                "Name": "closed-bill",
                "Description": "a custom line item with a billing period range",
                "BillingGroupArn": group,
                "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 1.0 } },
                "BillingPeriodRange": requested,
            }),
        )
        .await;
    answers.push(("CreateCustomLineItem", status, body));

    let (status, body) = emulator
        .send(
            "POST",
            "/update-custom-line-item",
            &json!({ "Arn": item, "BillingPeriodRange": requested }),
        )
        .await;
    answers.push(("UpdateCustomLineItem", status, body));

    let (status, body) = emulator
        .associate(item, std::slice::from_ref(&resource.to_string()), Some(requested.clone()))
        .await;
    answers.push(("BatchAssociateResourcesToCustomLineItem", status, body));

    let (status, body) = disassociate(emulator, item, resource, requested).await;
    answers.push(("BatchDisassociateResourcesFromCustomLineItem", status, body));

    let (status, body) = emulator
        .send(
            "POST",
            "/delete-custom-line-item",
            &json!({ "Arn": item, "BillingPeriodRange": requested }),
        )
        .await;
    answers.push(("DeleteCustomLineItem", status, body));

    answers
}

/// ONE closed bill, ONE refusal: every operation that takes the range decides the start's
/// bill through the same check, so the five answer a start whose bill is closed
/// identically — which is what deciding it in one place guarantees and copies do not.
#[tokio::test]
async fn every_operation_taking_the_range_refuses_a_closed_bill_the_same_way() {
    // The starts differ in how far back the closed bill lies and how the month is
    // spelled; the ends in whether the range is open or bounded.
    let rows: Vec<(&str, String, Option<String>)> = vec![
        (
            "one month before the previous billing period",
            period(-2),
            None,
        ),
        (
            "a year before the current billing period",
            period(-12),
            Some(period(-11)),
        ),
        (
            "a closed month spelled without a leading zero",
            one_digit_month(&period(-3)),
            None,
        ),
        ("the epoch month", "1970-01".to_string(), None),
    ];

    for (case, start, end) in rows {
        let emulator = Emulator::new();
        let group = emulator.billing_group("payer").await;
        let item = emulator.percentage_item(&group, "target").await;
        // A resource in scope of the billing group the target applies to — the group
        // itself — so the only thing that can refuse these calls is the range.
        let requested = range(&start, end.as_deref());
        let answers = answers(&emulator, &group, &item, &group, &requested).await;

        for (operation, status, body) in &answers {
            let case = format!("{case} ({start}) at {operation}");
            assert_closed_bill_refusal(&case, *status, body, &start);
        }

        let (first_operation, first_status, first_body) = &answers[0];
        for (operation, status, body) in &answers[1..] {
            assert_eq!(
                (status, body),
                (first_status, first_body),
                "case {case:?} ({start}): {operation} must answer exactly as {first_operation} \
                 does, got {status} with {body}"
            );
        }
    }
}
