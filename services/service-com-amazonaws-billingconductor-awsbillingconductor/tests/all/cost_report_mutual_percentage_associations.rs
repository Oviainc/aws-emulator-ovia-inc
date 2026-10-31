// SPDX-License-Identifier: CC0-1.0

//! What a billing group's cost report answers when its percentage custom line items are
//! associated to EACH OTHER, end to end over the wire: `CreateCustomLineItem` ->
//! `BatchAssociateResourcesToCustomLineItem` -> `GetBillingGroupCostReport` and
//! `ListBillingGroupCostReports`.
//!
//! RULE: a custom line item's charge is worked out from the item and the associations the
//! store holds, so one configuration is worth one amount whatever order the stored items
//! are walked in: a percentage item the associations lead back round to is a percentage of
//! its own charge, which nothing settles, so it charges nothing and lends nothing, while
//! every item whose associations do not lead back to it is its percentage of what they
//! lend it — and both cost reports answer the same `ProformaCost`, `Margin` and
//! `MarginPercentage` for the configuration however its items were created.
//!
//! The rows differ along the arrangement of the associations (a mutual pair with nothing
//! else to lend, a pair one or both of whose items are also lent a flat charge, a ring of
//! three, a cycle beside an acyclic chain off the same item), the percentages involved,
//! the charge type of a cyclic item (fee / credit) and the number of items a group holds;
//! each is driven in both creation orders, which is what the stored order follows.

use crate::percentage_custom_line_item_resource_associations::{reported, Emulator};
use serde_json::{json, Value};

/// How far two amounts may differ and still be the same amount: the reported strings are
/// decimals rounded well inside this, so a comparison at this tolerance pins the amount
/// without pinning the rendering.
const TOLERANCE: f64 = 1e-6;

/// The charge one item of a configuration carries: "It should contain only one of `Flat`
/// or `Percentage`" (`smithy_query(query="shape",shape="CustomLineItemChargeDetails")`).
#[derive(Debug, Clone, Copy)]
enum Charge {
    /// A flat `ChargeValue`.
    Flat(f64),
    /// A `PercentageValue`, taken of the combined value of the associated resources.
    Percentage(f64),
}

/// One custom line item of a configuration: its charge, its `FEE`/`CREDIT` type and the
/// other items of the same configuration associated to it, by index into the
/// configuration.
#[derive(Debug, Clone, Copy)]
struct Item {
    charge_type: &'static str,
    charge: Charge,
    associated: &'static [usize],
}

/// The `ChargeDetails` a create carries for one configured item.
fn charge_details(item: &Item) -> Value {
    match item.charge {
        Charge::Flat(charge_value) => json!({
            "Type": item.charge_type,
            "Flat": { "ChargeValue": charge_value },
        }),
        Charge::Percentage(percentage_value) => json!({
            "Type": item.charge_type,
            "Percentage": { "PercentageValue": percentage_value },
        }),
    }
}

/// One reported amount of a summary row, read as the decimal its string carries.
fn amount(row: &Value, member: &str, case: &str) -> f64 {
    let text = row
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a {member} member, got {row}"));
    text.parse::<f64>()
        .unwrap_or_else(|_| panic!("case {case:?}: expected a decimal {member}, got {text:?}"))
}

/// Assert the whole money side of one summary row: the pro forma cost is `proforma`, the
/// actual cost is the metered usage this emulator records (none), the margin is the
/// difference and the margin percentage is that margin as a share of the pro forma amount
/// (zero where there is no pro forma amount to take a share of).
fn assert_summary(row: &Value, proforma: f64, case: &str) {
    let expected_percentage = if proforma == 0.0 { 0.0 } else { 100.0 };
    for (member, expected) in [
        ("AWSCost", 0.0),
        ("ProformaCost", proforma),
        ("Margin", proforma),
        ("MarginPercentage", expected_percentage),
    ] {
        let actual = amount(row, member, case);
        assert!(
            (actual - expected).abs() < TOLERANCE,
            "case {case:?}: expected {member} {expected}, got {actual} in {row}"
        );
    }
}

/// Build one configuration into its own billing group, creating its items in the order
/// `creation_order` names and then associating each item's resources, and return the
/// billing group's ARN.
async fn configure(
    em: &Emulator,
    case: &str,
    group_name: &str,
    items: &[Item],
    creation_order: &[usize],
) -> String {
    let group = em.billing_group(group_name).await;
    let mut arns: Vec<String> = vec![String::new(); items.len()];
    for &index in creation_order {
        arns[index] = em
            .custom_line_item(
                &group,
                &format!("{group_name}item{index}"),
                charge_details(&items[index]),
            )
            .await;
    }
    for (index, item) in items.iter().enumerate() {
        if item.associated.is_empty() {
            continue;
        }
        let resources: Vec<String> = item
            .associated
            .iter()
            .map(|resource| arns[*resource].clone())
            .collect();
        let (status, body) = em.associate(&arns[index], &resources, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: associating {resources:?} to item {index}: expected HTTP 200, \
             got {status} with {body}"
        );
        assert_eq!(
            reported(&body, "SuccessfullyAssociatedResources"),
            resources,
            "case {case:?}: associating to item {index}: expected every resource associated, \
             got {body}"
        );
    }
    group
}

/// The ONE summary row `GetBillingGroupCostReport` answers for a group when the request
/// asks for no breakdown.
async fn report_row(em: &Emulator, case: &str, group_arn: &str) -> Value {
    let (status, body) = em
        .send(
            "POST",
            "/get-billing-group-cost-report",
            &json!({ "Arn": group_arn }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from the cost report, got {status} with {body}"
    );
    let rows = body
        .get("BillingGroupCostReportResults")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        rows.len(),
        1,
        "case {case:?}: a report with no GroupBy is one row for the group, got {body}"
    );
    rows[0].clone()
}

/// The summary row `ListBillingGroupCostReports` answers for the one group its filter
/// names — the OTHER report computed from the same charges.
async fn listed_row(em: &Emulator, case: &str, group_arn: &str) -> Value {
    let (status, body) = em
        .send(
            "POST",
            "/list-billing-group-cost-reports",
            &json!({ "Filters": { "BillingGroupArns": [group_arn] } }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from the cost report listing, got {status} with {body}"
    );
    let rows = body
        .get("BillingGroupCostReports")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        rows.len(),
        1,
        "case {case:?}: the filter names one stored group, so one report is due, got {body}"
    );
    rows[0].clone()
}

/// RULE: an item the associations lead back round to charges nothing, every other item
/// charges its percentage of what its associated resources lend it, and the cost reports
/// of one configuration therefore answer the same amounts in either creation order.
#[tokio::test]
async fn an_item_standing_on_its_own_charge_charges_nothing_in_either_creation_order() {
    let em = Emulator::new();

    // (case, the configuration's items, the pro forma cost it is worth). An item whose
    // associations lead back round to it charges nothing; the rest are settled from the
    // items they are a percentage of.
    let cases: [(&str, &[Item], f64); 7] = [
        (
            // Neither has a settled value to lend the other, so the pair is worth nothing.
            "a mutual pair with nothing else to lend",
            &[
                Item {
                    charge_type: "FEE",
                    charge: Charge::Percentage(50.0),
                    associated: &[1],
                },
                Item {
                    charge_type: "FEE",
                    charge: Charge::Percentage(50.0),
                    associated: &[0],
                },
            ],
            0.0,
        ),
        (
            // The pair stands on its own charge even though the second item is also lent
            // a flat 100, so only that flat charge is on the bill.
            "a mutual pair, the second of them also lent a flat charge",
            &[
                Item {
                    charge_type: "FEE",
                    charge: Charge::Percentage(100.0),
                    associated: &[1],
                },
                Item {
                    charge_type: "FEE",
                    charge: Charge::Percentage(100.0),
                    associated: &[0, 2],
                },
                Item {
                    charge_type: "FEE",
                    charge: Charge::Flat(100.0),
                    associated: &[],
                },
            ],
            100.0,
        ),
        (
            // Lent from both sides or from one, the pair is still a percentage of itself.
            "a mutual pair, both of them lent the same flat charge",
            &[
                Item {
                    charge_type: "FEE",
                    charge: Charge::Percentage(100.0),
                    associated: &[1, 2],
                },
                Item {
                    charge_type: "FEE",
                    charge: Charge::Percentage(100.0),
                    associated: &[0, 2],
                },
                Item {
                    charge_type: "FEE",
                    charge: Charge::Flat(100.0),
                    associated: &[],
                },
            ],
            100.0,
        ),
        (
            // Whatever the two percentages are, neither has a charge to take them of.
            "a mutual pair of unequal percentages",
            &[
                Item {
                    charge_type: "FEE",
                    charge: Charge::Percentage(50.0),
                    associated: &[1],
                },
                Item {
                    charge_type: "FEE",
                    charge: Charge::Percentage(20.0),
                    associated: &[0, 2],
                },
                Item {
                    charge_type: "FEE",
                    charge: Charge::Flat(200.0),
                    associated: &[],
                },
            ],
            200.0,
        ),
        (
            // A credit with no charge to be a percentage of takes nothing off the bill.
            "a mutual pair whose first item is a CREDIT",
            &[
                Item {
                    charge_type: "CREDIT",
                    charge: Charge::Percentage(100.0),
                    associated: &[1],
                },
                Item {
                    charge_type: "FEE",
                    charge: Charge::Percentage(100.0),
                    associated: &[0, 2],
                },
                Item {
                    charge_type: "FEE",
                    charge: Charge::Flat(100.0),
                    associated: &[],
                },
            ],
            100.0,
        ),
        (
            // A ring is as much a percentage of itself as a pair is, however long it is.
            "a ring of three percentage items, one of them lent a flat charge",
            &[
                Item {
                    charge_type: "FEE",
                    charge: Charge::Percentage(100.0),
                    associated: &[1],
                },
                Item {
                    charge_type: "FEE",
                    charge: Charge::Percentage(100.0),
                    associated: &[2],
                },
                Item {
                    charge_type: "FEE",
                    charge: Charge::Percentage(100.0),
                    associated: &[0, 3],
                },
                Item {
                    charge_type: "FEE",
                    charge: Charge::Flat(100.0),
                    associated: &[],
                },
            ],
            100.0,
        ),
        (
            // The chain off the flat item does not lead back to itself, so it IS settled:
            // 100 flat plus 10% of it, beside the pair that charges nothing.
            "a mutual pair beside an acyclic chain off the same flat charge",
            &[
                Item {
                    charge_type: "FEE",
                    charge: Charge::Flat(100.0),
                    associated: &[],
                },
                Item {
                    charge_type: "FEE",
                    charge: Charge::Percentage(10.0),
                    associated: &[0],
                },
                Item {
                    charge_type: "FEE",
                    charge: Charge::Percentage(100.0),
                    associated: &[3],
                },
                Item {
                    charge_type: "FEE",
                    charge: Charge::Percentage(100.0),
                    associated: &[2, 0],
                },
            ],
            110.0,
        ),
    ];

    for (row, (case, items, proforma)) in cases.into_iter().enumerate() {
        let forward: Vec<usize> = (0..items.len()).collect();
        let backward: Vec<usize> = (0..items.len()).rev().collect();
        // The stored items are ordered by ARN, and an ARN carries the monotonic id this
        // service mints, so creating the same configuration the other way round is what
        // stores it the other way round.
        for (order_name, creation_order) in [("created in order", &forward), ("reversed", &backward)]
        {
            let case = format!("{case} ({order_name})");
            let group = configure(
                &em,
                &case,
                &format!("mutual{row}{}", creation_order[0]),
                items,
                creation_order,
            )
            .await;
            assert_summary(&report_row(&em, &case, &group).await, proforma, &case);
            assert_summary(&listed_row(&em, &case, &group).await, proforma, &case);
        }
    }
}
