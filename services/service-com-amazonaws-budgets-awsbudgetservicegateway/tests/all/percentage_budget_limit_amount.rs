// SPDX-License-Identifier: CC0-1.0

//! The one amount a utilization or coverage budget's `BudgetLimit` may name.
//!
//! The RULE asserted here: "RI and Savings Plans utilization or coverage budgets default
//! to `100`. This is the only valid value for RI or Savings Plans utilization or coverage
//! budgets" (`Budget$BudgetLimit`) — so a budget of one of those four types is accepted
//! with no `BudgetLimit` at all, or with one whose amount names that single value, and
//! refused any other amount, on `CreateBudget` and `UpdateBudget` alike. A budget of any
//! other type is not touched by the rule: its limit is the caller's own figure.
//!
//! The tables span the class rather than the recorded request: rows differ along the
//! budget type (every member of the model's generated catalog), along the AMOUNT (the one
//! valid value in several of the spellings `NumericValue`'s `^([0-9]*\.)?[0-9]+$` pattern
//! allows, values either side of it, zero, a fractional value and a large one), and along
//! the `Spend$Unit`, which that sentence does not constrain.
//!
//! The write harness (`Harness`, `Verdict`) is the one
//! `tests/all/budget_validation_rules.rs` builds for the other documented `Budget` rules,
//! so each row is driven through both write paths and a refused write is checked to leave
//! the stored budget exactly as it was.

use serde_json::{json, Value};

/// The model-derived catalog the tables span, so they cannot drift from the model.
use service_com_amazonaws_budgets_awsbudgetservicegateway::ops::types::BUDGET_TYPE_VALUES;

use super::budget_lifecycle::{call, caller_account, router};
use super::budget_validation_rules::{Harness, Verdict};

/// The `BudgetType` members the rule holds for, and the one amount it allows them.
///
/// Deliberately written out here from `Budget$BudgetLimit` ("RI and Savings Plans
/// utilization or coverage budgets default to `100`. This is the only valid value for RI
/// or Savings Plans utilization or coverage budgets") rather than read out of the
/// service's own constants: an expectation taken from the code under test cannot fail
/// with it.
const UTILIZATION_OR_COVERAGE_BUDGET_TYPES: [&str; 4] = [
    "RI_UTILIZATION",
    "RI_COVERAGE",
    "SAVINGS_PLANS_UTILIZATION",
    "SAVINGS_PLANS_COVERAGE",
];

/// The one valid amount, as the number it names.
const ONLY_VALID_AMOUNT: f64 = 100.0;

/// Amounts inside `NumericValue`'s pattern, each with the number it names: several
/// spellings of the one valid value, and values on either side of it — zero, a fraction,
/// one just below, one just above, and one an order of magnitude larger.
fn amounts() -> Vec<(&'static str, f64)> {
    vec![
        ("100", 100.0),
        ("100.0", 100.0),
        ("100.000", 100.0),
        ("0100", 100.0),
        ("0", 0.0),
        (".5", 0.5),
        ("50", 50.0),
        ("99.999", 99.999),
        ("100.001", 100.001),
        ("101", 101.0),
        ("1000", 1000.0),
    ]
}

/// RULE: a utilization or coverage budget's `BudgetLimit` may name only the one documented
/// value; a budget of any other type may name any amount. Asserted over the model's whole
/// `BudgetType` catalog × the amount table × two units, on both write paths.
#[tokio::test]
async fn only_the_documented_amount_is_valid_for_a_utilization_or_coverage_budget() {
    let harness = Harness::new();
    for budget_type in BUDGET_TYPE_VALUES {
        let rule_applies = UTILIZATION_OR_COVERAGE_BUDGET_TYPES.contains(budget_type);
        for (amount, value) in amounts() {
            // The unit is not part of the rule: the documentation constrains the amount
            // only, and `UnitValue`'s `.*` pattern accepts any unit — so each amount is
            // sent under a currency unit and under a percentage one, and both rows must
            // answer the same way.
            for unit in ["USD", "PERCENTAGE"] {
                let expect = if rule_applies && value != ONLY_VALID_AMOUNT {
                    Verdict::Refused
                } else {
                    Verdict::Accepted
                };
                harness
                    .check(
                        &format!("{budget_type} with a BudgetLimit of {amount} {unit}"),
                        &json!({
                            "BudgetName": "limit-amount",
                            "TimeUnit": "MONTHLY",
                            "BudgetType": budget_type,
                            "BudgetLimit": {"Amount": amount, "Unit": unit}
                        }),
                        expect,
                    )
                    .await;
            }
        }

        // A utilization or coverage budget needs no limit at all (it takes the documented
        // default), while a cost or usage budget is refused for want of one — the rule
        // above refuses an amount, it does not require the member.
        harness
            .check(
                &format!("{budget_type} with no BudgetLimit"),
                &json!({
                    "BudgetName": "limit-absent",
                    "TimeUnit": "MONTHLY",
                    "BudgetType": budget_type
                }),
                if rule_applies {
                    Verdict::Accepted
                } else {
                    Verdict::Refused
                },
            )
            .await;
    }
}

/// RULE (the stored consequence): whatever a write carried, a stored utilization or
/// coverage budget reads back with a limit naming the one valid value — which is what a
/// `PERCENTAGE` notification threshold is measured against, so no budget this service
/// holds can report a percentage of an amount the model says cannot exist.
#[tokio::test]
async fn a_stored_utilization_or_coverage_budget_reports_the_documented_amount() {
    let account_id = caller_account();
    // Every way such a budget can be written: with no limit at all, and with each
    // accepted spelling of the valid amount under an unconstrained unit.
    let written: Vec<(&str, Value)> = vec![
        ("no BudgetLimit", json!(null)),
        ("the amount as digits", json!({"Amount": "100", "Unit": "PERCENTAGE"})),
        ("the amount with a fraction", json!({"Amount": "100.0", "Unit": "PERCENT"})),
        ("the amount with a leading zero", json!({"Amount": "0100", "Unit": "USD"})),
    ];
    for budget_type in UTILIZATION_OR_COVERAGE_BUDGET_TYPES {
        for (sent, limit) in &written {
            let case = format!("{budget_type} written with {sent}");
            let router = router();
            let mut budget = json!({
                "BudgetName": "stored-limit",
                "TimeUnit": "MONTHLY",
                "BudgetType": budget_type
            });
            if !limit.is_null() {
                budget["BudgetLimit"] = limit.clone();
            }
            let (status, body) = call(
                &router,
                "CreateBudget",
                &json!({"AccountId": &account_id, "Budget": budget}),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected CreateBudget to answer 200, got {status} {body}"
            );

            let (status, body) = call(
                &router,
                "DescribeBudget",
                &json!({"AccountId": &account_id, "BudgetName": "stored-limit"}),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected DescribeBudget to answer 200, got {status} {body}"
            );
            let reported = body["Budget"]["BudgetLimit"].clone();
            let amount = reported["Amount"]
                .as_str()
                .and_then(|amount| amount.parse::<f64>().ok());
            assert_eq!(
                amount,
                Some(ONLY_VALID_AMOUNT),
                "case {case:?}: expected a stored limit naming {ONLY_VALID_AMOUNT}, got \
                 {reported}"
            );
            // The unit stays the caller's own, and a budget written without a limit gets
            // one: the rule is about the amount, and the member is always reported.
            assert!(
                reported["Unit"].as_str().is_some_and(|unit| !unit.is_empty()),
                "case {case:?}: expected the stored limit to name a unit, got {reported}"
            );
        }
    }
}
