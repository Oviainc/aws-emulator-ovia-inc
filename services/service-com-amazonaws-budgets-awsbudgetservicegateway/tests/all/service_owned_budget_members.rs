// SPDX-License-Identifier: CC0-1.0

//! The members of a `Budget` the SERVICE owns rather than the caller: the spend and
//! auto-adjust figures it derives from the cost it tracks (never taken from a request),
//! and the `BudgetLimit` a utilization or coverage budget defaults to.
//!
//! Every test states a RULE and asserts it over a table; the requests the issue recorded
//! are three of the rows. The harness (`router`, `call`, `account`, `carries`, …) is the
//! one `tests/all/budget_lifecycle.rs` already builds.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The model-derived catalog the tables below span, so they cannot drift from the model.
use service_com_amazonaws_budgets_awsbudgetservicegateway::ops::types::{
    BUDGET_TYPE_VALUES, TIME_UNIT_VALUES,
};
use service_com_amazonaws_budgets_awsbudgetservicegateway::state::{
    PERCENTAGE_LIMIT_AMOUNT, PERCENTAGE_LIMIT_BUDGET_TYPES, PERCENTAGE_LIMIT_UNIT,
};

use super::budget_lifecycle::{account, call, carries, is_error, router};

/// The `BudgetType` members `Budget$BudgetLimit` requires a limit of ("`BudgetLimit` is
/// required for cost or usage budgets"), which is therefore the complement of the
/// utilization/coverage types that default to `100`.
///
/// Deliberately a SECOND, independent copy of the service's own private constant of the
/// same name (`src/state.rs`), not a stale duplicate of it: the pin below is only worth
/// anything if the expected set is spelled out here from the documentation rather than
/// read back out of the code under test.
const LIMIT_REQUIRED_BUDGET_TYPES: [&str; 2] = ["COST", "USAGE"];

/// Drive one write operation (`CreateBudget` with `Budget`, `UpdateBudget` with
/// `NewBudget`) for `account_id` and return the `Budget` a following `DescribeBudget`
/// reports. The write must be accepted.
async fn write_and_read(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    account_id: &str,
    budget: &Value,
) -> Value {
    let request = match operation {
        "CreateBudget" => json!({"AccountId": account_id, "Budget": budget.clone()}),
        _ => json!({"AccountId": account_id, "NewBudget": budget.clone()}),
    };
    let (status, body) = call(router, operation, &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {operation} to answer 200, got {status} {body}"
    );

    let name = budget["BudgetName"]
        .as_str()
        .expect("every row names its budget");
    let (status, body) = call(
        router,
        "DescribeBudget",
        &json!({"AccountId": account_id, "BudgetName": name}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeBudget to answer 200, got {status} {body}"
    );
    body["Budget"].clone()
}

/// The value at `path` (a trail of member names) in `budget`, or `Value::Null`.
fn member<'a>(budget: &'a Value, path: &[&str]) -> &'a Value {
    let mut here = budget;
    for segment in path {
        match here.get(segment) {
            Some(next) => here = next,
            None => return &Value::Null,
        }
    }
    here
}

/// Assert that `path` is absent from (or null in) the budget a read reported.
fn assert_absent(case: &str, budget: &Value, path: &[&str]) {
    let actual = member(budget, path);
    assert!(
        actual.is_null(),
        "case {case:?}: expected the service to report no {}, got {actual} in {budget}",
        path.join(".")
    );
}

/// One row of the service-derived table: the case, the `Budget` to write, and the member
/// paths whose caller-sent values the write must discard.
struct Derived {
    case: &'static str,
    budget: Value,
    dropped: Vec<Vec<&'static str>>,
    /// Member paths the caller DOES own, which must survive the write unchanged.
    kept: Value,
}

/// The table. Rows differ along what the rule generalises over: WHICH service-derived
/// member the request carries, its value (zero, small, the top of its modelled range),
/// the budget type and the time-unit granularity, and whether the budget is a fixed-limit,
/// planned-limit or auto-adjusting one.
fn derived_rows() -> Vec<Derived> {
    vec![
        Derived {
            // The request the issue recorded (step 1).
            case: "cost budget supplying an actual spend",
            budget: json!({
                "BudgetName": "echoed-spend",
                "TimeUnit": "MONTHLY",
                "BudgetType": "COST",
                "BudgetLimit": {"Amount": "100", "Unit": "USD"},
                "CalculatedSpend": {"ActualSpend": {"Amount": "777", "Unit": "USD"}}
            }),
            dropped: vec![vec!["CalculatedSpend"]],
            kept: json!({"BudgetLimit": {"Amount": "100", "Unit": "USD"}}),
        },
        Derived {
            case: "usage budget supplying an actual and a forecasted spend",
            budget: json!({
                "BudgetName": "echoed-forecast",
                "TimeUnit": "DAILY",
                "BudgetType": "USAGE",
                "BudgetLimit": {"Amount": "3.5", "Unit": "GB"},
                "CalculatedSpend": {
                    "ActualSpend": {"Amount": "0", "Unit": "GB"},
                    "ForecastedSpend": {"Amount": "12.25", "Unit": "GB"}
                }
            }),
            dropped: vec![vec!["CalculatedSpend"]],
            kept: json!({"BudgetLimit": {"Amount": "3.5", "Unit": "GB"}}),
        },
        Derived {
            case: "planned-limit budget supplying a spend",
            budget: json!({
                "BudgetName": "planned-with-spend",
                "TimeUnit": "QUARTERLY",
                "BudgetType": "COST",
                "PlannedBudgetLimits": {"1704067200": {"Amount": "10", "Unit": "USD"}},
                "CalculatedSpend": {"ActualSpend": {"Amount": "9999999", "Unit": "USD"}}
            }),
            dropped: vec![vec!["CalculatedSpend"]],
            kept: json!({"PlannedBudgetLimits": {"1704067200": {"Amount": "10", "Unit": "USD"}}}),
        },
        Derived {
            case: "RI coverage budget supplying a spend in percent",
            budget: json!({
                "BudgetName": "ri-coverage-with-spend",
                "TimeUnit": "ANNUALLY",
                "BudgetType": "RI_COVERAGE",
                "CalculatedSpend": {"ActualSpend": {"Amount": "42", "Unit": "PERCENTAGE"}}
            }),
            dropped: vec![vec!["CalculatedSpend"]],
            kept: json!({"BudgetType": "RI_COVERAGE"}),
        },
        Derived {
            // The request the issue recorded (step 3).
            case: "historical auto-adjust supplying both derived auto-adjust members",
            budget: json!({
                "BudgetName": "auto-adjust-echo",
                "TimeUnit": "MONTHLY",
                "BudgetType": "COST",
                "AutoAdjustData": {
                    "AutoAdjustType": "HISTORICAL",
                    "HistoricalOptions": {
                        "BudgetAdjustmentPeriod": 3,
                        "LookBackAvailablePeriods": 42
                    },
                    "LastAutoAdjustTime": 0
                }
            }),
            dropped: vec![
                vec!["AutoAdjustData", "LastAutoAdjustTime"],
                vec!["AutoAdjustData", "HistoricalOptions", "LookBackAvailablePeriods"],
            ],
            kept: json!({
                "AutoAdjustData": {
                    "AutoAdjustType": "HISTORICAL",
                    "HistoricalOptions": {"BudgetAdjustmentPeriod": 3}
                }
            }),
        },
        Derived {
            case: "daily historical auto-adjust at the top of the look-back range",
            budget: json!({
                "BudgetName": "auto-adjust-max",
                "TimeUnit": "DAILY",
                "BudgetType": "USAGE",
                "AutoAdjustData": {
                    "AutoAdjustType": "HISTORICAL",
                    "HistoricalOptions": {
                        "BudgetAdjustmentPeriod": 60,
                        "LookBackAvailablePeriods": 60
                    },
                    "LastAutoAdjustTime": 1_767_225_600
                }
            }),
            dropped: vec![
                vec!["AutoAdjustData", "LastAutoAdjustTime"],
                vec!["AutoAdjustData", "HistoricalOptions", "LookBackAvailablePeriods"],
            ],
            kept: json!({
                "AutoAdjustData": {
                    "AutoAdjustType": "HISTORICAL",
                    "HistoricalOptions": {"BudgetAdjustmentPeriod": 60}
                }
            }),
        },
        Derived {
            case: "forecast auto-adjust supplying an adjust time, plus a spend",
            budget: json!({
                "BudgetName": "forecast-auto-adjust",
                "TimeUnit": "CUSTOM",
                "BudgetType": "SAVINGS_PLANS_UTILIZATION",
                "AutoAdjustData": {
                    "AutoAdjustType": "FORECAST",
                    "LastAutoAdjustTime": 1_704_067_200
                },
                "CalculatedSpend": {
                    "ActualSpend": {"Amount": "1", "Unit": "PERCENTAGE"},
                    "ForecastedSpend": {"Amount": "2", "Unit": "PERCENTAGE"}
                }
            }),
            dropped: vec![
                vec!["CalculatedSpend"],
                vec!["AutoAdjustData", "LastAutoAdjustTime"],
            ],
            kept: json!({"AutoAdjustData": {"AutoAdjustType": "FORECAST"}}),
        },
    ]
}

/// RULE: a member whose value the service derives from the cost and usage it tracks is
/// not caller-settable — the write path discards whatever the request carried for it and
/// a read reports only what the service itself derived, which for this emulator (it
/// tracks no spend) is nothing. The caller-owned members of the same shapes survive.
#[tokio::test]
async fn a_write_discards_the_members_the_service_derives_from_tracked_cost() {
    let router = router();
    // One account per row, so rows cannot see each other's budgets.
    for (index, row) in derived_rows().into_iter().enumerate() {
        let account_id = account(700_000_000 + index as u64);
        let read =
            write_and_read(&router, row.case, "CreateBudget", &account_id, &row.budget).await;
        for path in &row.dropped {
            assert_absent(row.case, &read, path);
        }
        assert!(
            carries(&row.kept, &read),
            "case {:?}: expected the read to carry the caller's {}, got {read}",
            row.case,
            row.kept
        );
    }
}

/// RULE: the same holds on the UPDATE write path — `UpdateBudget` documents that "you can
/// change every part of a budget except for the `budgetName` and the `calculatedSpend`",
/// so a budget that reads back without the derived members keeps reading back without
/// them however many times a caller resends them.
#[tokio::test]
async fn an_update_discards_them_too() {
    let router = router();
    for (index, row) in derived_rows().into_iter().enumerate() {
        let account_id = account(710_000_000 + index as u64);
        let created =
            write_and_read(&router, row.case, "CreateBudget", &account_id, &row.budget).await;
        for path in &row.dropped {
            assert_absent(row.case, &created, path);
        }
        // The same budget sent again as the update, so the only thing under test is
        // whether the update honours the rule the create does.
        let updated =
            write_and_read(&router, row.case, "UpdateBudget", &account_id, &row.budget).await;
        for path in &row.dropped {
            assert_absent(row.case, &updated, path);
        }
        assert!(
            carries(&row.kept, &updated),
            "case {:?}: expected the update to keep the caller's {}, got {updated}",
            row.case,
            row.kept
        );
    }
}

/// RULE: "RI and Savings Plans utilization or coverage budgets default to `100`"
/// (`Budget$BudgetLimit`) — a utilization or coverage budget written without a
/// `BudgetLimit` is stored with that limit and reads it back; a cost or usage budget has
/// no such default (its limit is required), and a limit the caller sent is never
/// replaced.
///
/// Spanned over the generated `BUDGET_TYPE_VALUES` × `TIME_UNIT_VALUES` catalogs, so
/// every budget type the model declares is exercised — including the three the issue did
/// not cite.
#[tokio::test]
async fn a_utilization_or_coverage_budget_defaults_to_the_documented_limit() {
    let router = router();
    let expected_default = json!({
        "Amount": PERCENTAGE_LIMIT_AMOUNT,
        "Unit": PERCENTAGE_LIMIT_UNIT
    });

    for (index, budget_type) in BUDGET_TYPE_VALUES.iter().enumerate() {
        let defaults = PERCENTAGE_LIMIT_BUDGET_TYPES.contains(budget_type);
        for (unit_index, time_unit) in TIME_UNIT_VALUES.iter().enumerate() {
            let case = format!("{budget_type} / {time_unit} created without a BudgetLimit");
            let account_id = account(720_000_000 + (index * 100 + unit_index) as u64);
            let budget = json!({
                "BudgetName": "no-limit",
                "TimeUnit": time_unit,
                "BudgetType": budget_type
            });
            let (status, body) = call(
                &router,
                "CreateBudget",
                &json!({"AccountId": &account_id, "Budget": budget}),
            )
            .await;

            if !defaults {
                // "BudgetLimit is required for cost or usage budgets" — no default is
                // invented for the types the model requires one of.
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected a budget that requires a limit to be refused, \
                     got {status} {body}"
                );
                assert!(
                    is_error(&body, "InvalidParameterException"),
                    "case {case:?}: expected InvalidParameterException, got {body}"
                );
                continue;
            }

            assert_eq!(
                status, 200,
                "case {case:?}: expected CreateBudget to answer 200, got {status} {body}"
            );
            let (status, body) = call(
                &router,
                "DescribeBudget",
                &json!({"AccountId": &account_id, "BudgetName": "no-limit"}),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected DescribeBudget to answer 200, got {status} {body}"
            );
            let actual = &body["Budget"]["BudgetLimit"];
            assert!(
                carries(&expected_default, actual),
                "case {case:?}: expected the documented default limit {expected_default}, \
                 got {actual}"
            );
        }
    }
}

/// RULE (the other half): a `BudgetLimit` the caller DID send is stored as sent, whatever
/// the budget type — the default fills an absent limit, it does not overwrite one.
#[tokio::test]
async fn a_supplied_budget_limit_is_never_replaced_by_the_default() {
    let router = router();
    // Amounts spanning the `NumericValue` pattern `^([0-9]*\.)?[0-9]+$`: an integer, a
    // decimal, and one with no leading digit, each under a different `Spend$Unit`.
    let cost_or_usage_limits = [
        json!({"Amount": "1", "Unit": "USD"}),
        json!({"Amount": "99.5", "Unit": "GB"}),
        json!({"Amount": ".25", "Unit": "PERCENT"}),
    ];
    // A utilization or coverage budget's limit has only one valid AMOUNT ("RI and Savings
    // Plans utilization or coverage budgets default to `100`. This is the only valid
    // value", `Budget$BudgetLimit`), so those rows vary the unit — which that sentence
    // does not constrain and `UnitValue`'s `.*` pattern accepts — rather than pinning
    // acceptance of an amount the documentation calls invalid.
    let percentage_limits = [
        json!({"Amount": PERCENTAGE_LIMIT_AMOUNT, "Unit": PERCENTAGE_LIMIT_UNIT}),
        json!({"Amount": PERCENTAGE_LIMIT_AMOUNT, "Unit": "PERCENT"}),
    ];
    for (index, budget_type) in BUDGET_TYPE_VALUES.iter().enumerate() {
        let limits: &[Value] = if PERCENTAGE_LIMIT_BUDGET_TYPES.contains(budget_type) {
            &percentage_limits
        } else {
            &cost_or_usage_limits
        };
        for (limit_index, limit) in limits.iter().enumerate() {
            let case = format!("{budget_type} created with the limit {limit}");
            let account_id = account(730_000_000 + (index * 100 + limit_index) as u64);
            let read = write_and_read(
                &router,
                &case,
                "CreateBudget",
                &account_id,
                &json!({
                    "BudgetName": "own-limit",
                    "TimeUnit": "MONTHLY",
                    "BudgetType": budget_type,
                    "BudgetLimit": limit
                }),
            )
            .await;
            let actual = &read["BudgetLimit"];
            assert!(
                carries(limit, actual),
                "case {case:?}: expected the caller's limit {limit}, got {actual}"
            );
        }
    }
}

/// The service's own catalog of utilization/coverage budget types must BE the complement
/// of the types `Budget$BudgetLimit` requires a limit of, within the model's generated
/// `BUDGET_TYPE_VALUES` — so the default above cannot be applied to a type the model adds
/// later, and cannot miss one it already declares.
#[test]
fn the_percentage_limit_types_are_the_modelled_types_that_need_no_limit() {
    let mut expected: Vec<&str> = BUDGET_TYPE_VALUES
        .iter()
        .copied()
        .filter(|value| !LIMIT_REQUIRED_BUDGET_TYPES.contains(value))
        .collect();
    expected.sort_unstable();
    let mut actual: Vec<&str> = PERCENTAGE_LIMIT_BUDGET_TYPES.to_vec();
    actual.sort_unstable();
    assert_eq!(
        actual, expected,
        "case percentage-limit-catalog: expected the BudgetType members that are not \
         {LIMIT_REQUIRED_BUDGET_TYPES:?}, got {actual:?}"
    );
}
