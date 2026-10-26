// SPDX-License-Identifier: CC0-1.0

//! The documented `Budget` rules as a SET: every rule holds at once, over the whole
//! cross-product of the members they are stated on.
//!
//! `tests/all/budget_validation_rules.rs` states each rule on its own, varying one
//! dimension at a time. This suite asserts the two things that only the rule SET can be
//! wrong about:
//!
//! * the limit rules over the FULL cross-product of `BudgetType` × `TimeUnit` × the
//!   member a budget's limit comes from — so a granularity or a budget type cannot be
//!   judged by the rule of another;
//! * that the rules are a CONJUNCTION: a request breaking one rule is refused whatever
//!   the other members carry, and a request breaking two is refused as well — no
//!   violation is masked by a second one, and a refused `CreateBudget` leaves neither a
//!   budget nor the notifications and subscribers it carried.
//!
//! Every refusal is the modelled `InvalidParameterException` (HTTP 400) of
//! `CreateBudget`, and every accepted budget reads back through `DescribeBudget` carrying
//! what was sent.
//!
//! The harness (`router`, `call`, `caller_account`, `is_error`, `carries`, `civil_secs`)
//! is the one `tests/all/budget_lifecycle.rs` builds.

use serde_json::{json, Value};

/// The `BudgetType` / `TimeUnit` catalogs are the model-derived constants the scaffold
/// generated, so a cross-product built from them cannot drift from the model.
use service_com_amazonaws_budgets_awsbudgetservicegateway::ops::types::{
    BUDGET_TYPE_VALUES, TIME_UNIT_VALUES,
};

use super::budget_lifecycle::{caller_account, call, carries, civil_secs, is_error, router};

/// The error a write that breaks one of these rules answers with — a modelled error of
/// both `CreateBudget` and `UpdateBudget`.
const REFUSAL: &str = "InvalidParameterException";

/// The budget types whose budget must carry a limit: "`BudgetLimit` is required for cost
/// or usage budgets, but optional for RI or Savings Plans utilization or coverage
/// budgets" (`Budget$BudgetLimit`,
/// `smithy_query(query="doc",shape="Budget",member="BudgetLimit")`). These are also the
/// only types a planned map is available for: "`PlannedBudgetLimits` is available for
/// cost or usage budget" (`Budget$PlannedBudgetLimits`).
const BUDGET_TYPES_REQUIRING_A_LIMIT: [&str; 2] = ["COST", "USAGE"];

/// The granularities a planned map is available for: "`PlannedBudgetLimits` ... supports
/// both monthly and quarterly `TimeUnit`" (`Budget$PlannedBudgetLimits`).
const PLANNED_LIMIT_TIME_UNITS: [&str; 2] = ["MONTHLY", "QUARTERLY"];

/// What a write of a given budget must answer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Verdict {
    /// Stored, and read back carrying what was sent.
    Accepted,
    /// Refused with [`REFUSAL`], with nothing persisted.
    Refused,
}

/// A `Spend` of `amount`, measured in `unit`.
fn spend(amount: &str, unit: &str) -> Value {
    json!({"Amount": amount, "Unit": unit})
}

/// The fixed `BudgetLimit` a budget of `budget_type` is written with.
///
/// A utilization or coverage budget's limit is a percentage with exactly one documented
/// value — "RI and Savings Plans utilization or coverage budgets default to `100`. This
/// is the only valid value for RI or Savings Plans utilization or coverage budgets"
/// (`Budget$BudgetLimit`) — so those rows send that value rather than a currency amount.
fn fixed_limit(budget_type: &str) -> Value {
    if BUDGET_TYPES_REQUIRING_A_LIMIT.contains(&budget_type) {
        spend("100", "USD")
    } else {
        spend("100", "PERCENTAGE")
    }
}

/// The midnight-UTC start of a period of `time_unit`, which is what a planned map's key
/// is: "the `key` is the start of the month, `UTC` in epoch seconds" / "the start of the
/// quarter, `UTC` in epoch seconds" (`Budget$PlannedBudgetLimits`). A granularity that
/// has no planned periods takes a month start, so a refusal on such a row is about the
/// granularity and not the key.
fn planned_period_start(time_unit: &str) -> i64 {
    match time_unit {
        // April is the start of a calendar quarter as well as of a month.
        "QUARTERLY" => civil_secs("2024-04-01T00:00:00Z"),
        _ => civil_secs("2024-02-01T00:00:00Z"),
    }
}

/// The member a budget's limit comes from — the dimension the limit rules generalise
/// over.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum LimitSource {
    /// No limit member at all.
    Absent,
    /// A single fixed `BudgetLimit`.
    Fixed,
    /// A `PlannedBudgetLimits` map keyed on the start of a period of the budget's own
    /// time unit, which a read derives the current `BudgetLimit` from.
    Planned,
    /// `AutoAdjustData`, "the parameters that determine the budget amount for an
    /// auto-adjusting budget" (`Budget$AutoAdjustData`), whose amount the service adjusts
    /// rather than the caller setting it.
    AutoAdjusting,
}

impl LimitSource {
    /// The members this source contributes to a budget of `budget_type` / `time_unit`.
    fn members(self, budget_type: &str, time_unit: &str) -> Value {
        match self {
            Self::Absent => json!({}),
            Self::Fixed => json!({"BudgetLimit": fixed_limit(budget_type)}),
            Self::Planned => json!({
                "PlannedBudgetLimits": {
                    planned_period_start(time_unit).to_string(): spend("100", "USD")
                }
            }),
            Self::AutoAdjusting => json!({"AutoAdjustData": {"AutoAdjustType": "FORECAST"}}),
        }
    }
}

/// The verdict the documented rules give a budget of `budget_type` / `time_unit` whose
/// limit comes from `source`, computed from the rules themselves rather than read off the
/// implementation:
///
/// * a cost or usage budget with no member that determines its limit breaks
///   "`BudgetLimit` is required for cost or usage budgets";
/// * a planned map on any other budget type or granularity breaks "`PlannedBudgetLimits`
///   is available for cost or usage budget and supports both monthly and quarterly
///   `TimeUnit`".
fn documented_verdict(budget_type: &str, time_unit: &str, source: LimitSource) -> Verdict {
    let requires_limit = BUDGET_TYPES_REQUIRING_A_LIMIT.contains(&budget_type);
    let planned_available = requires_limit && PLANNED_LIMIT_TIME_UNITS.contains(&time_unit);
    match source {
        LimitSource::Absent if requires_limit => Verdict::Refused,
        LimitSource::Planned if !planned_available => Verdict::Refused,
        _ => Verdict::Accepted,
    }
}

/// Drive one `CreateBudget` over a store of its own and assert `expect`: an accepted
/// budget answers 200 and reads back carrying what was sent, a refused one answers
/// [`REFUSAL`] at 400 and leaves nothing behind.
async fn assert_create(case: &str, request: &Value, expect: Verdict) {
    let store = router();
    let account_id = caller_account();
    let name = request["Budget"]["BudgetName"]
        .as_str()
        .expect("every case names its budget")
        .to_string();
    let (status, body) = call(&store, "CreateBudget", request).await;
    let (read_status, read) = call(
        &store,
        "DescribeBudget",
        &json!({"AccountId": &account_id, "BudgetName": &name}),
    )
    .await;
    match expect {
        Verdict::Accepted => {
            assert_eq!(
                status, 200,
                "case {case:?}: expected CreateBudget to answer 200, got {status} {body}"
            );
            assert_eq!(
                read_status, 200,
                "case {case:?}: expected the created budget to read back, got {read_status} \
                 {read}"
            );
            assert!(
                carries(&request["Budget"], &read["Budget"]),
                "case {case:?}: expected the stored budget to carry {}, got {read}",
                request["Budget"]
            );
        }
        Verdict::Refused => {
            assert_eq!(
                status, 400,
                "case {case:?}: expected CreateBudget to answer 400, got {status} {body}"
            );
            assert!(
                is_error(&body, REFUSAL),
                "case {case:?}: expected {REFUSAL} from CreateBudget, got {body}"
            );
            assert!(
                is_error(&read, "NotFoundException"),
                "case {case:?}: expected a refused CreateBudget to store no budget, got {read}"
            );
            // "Creates a budget and, if included, notifications and subscribers"
            // (`CreateBudget`) — a refused create establishes none of the three, so the
            // notifications it carried cannot be read either.
            let (_, notifications) = call(
                &store,
                "DescribeNotificationsForBudget",
                &json!({"AccountId": &account_id, "BudgetName": &name}),
            )
            .await;
            assert!(
                is_error(&notifications, "NotFoundException"),
                "case {case:?}: expected a refused CreateBudget to store no notifications, \
                 got {notifications}"
            );
        }
    }
}

/// RULE: a budget's limit rules are stated over its `BudgetType` and its `TimeUnit`
/// together — a cost or usage budget must carry a limit from some member that determines
/// it, and a `PlannedBudgetLimits` map is available only for a cost or usage budget on a
/// monthly or quarterly granularity — so every combination of the two catalogs is judged
/// by those rules and by no other.
#[tokio::test]
async fn the_limit_rules_hold_over_every_budget_type_and_granularity() {
    let account_id = caller_account();
    let sources = [
        LimitSource::Absent,
        LimitSource::Fixed,
        LimitSource::Planned,
        LimitSource::AutoAdjusting,
    ];
    for budget_type in BUDGET_TYPE_VALUES {
        for time_unit in TIME_UNIT_VALUES {
            for source in sources {
                let mut budget = json!({
                    "BudgetName": "cross-product",
                    "TimeUnit": time_unit,
                    "BudgetType": budget_type
                });
                for (member, value) in source
                    .members(budget_type, time_unit)
                    .as_object()
                    .expect("a member map")
                {
                    budget[member] = value.clone();
                }
                let case = format!("{budget_type} {time_unit} with its limit from {source:?}");
                assert_create(
                    &case,
                    &json!({"AccountId": &account_id, "Budget": budget}),
                    documented_verdict(budget_type, time_unit, source),
                )
                .await;
            }
        }
    }
}

/// The members a violation rewrites. Two violations are combined only when their groups
/// differ, so neither can cancel what the other gets wrong.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Group {
    /// `BudgetLimit` and `PlannedBudgetLimits`.
    Limit,
    /// `TimePeriod`.
    Period,
    /// `AutoAdjustData`.
    AutoAdjust,
    /// The filter and metric selections (`CostTypes` / `FilterExpression`).
    Filters,
    /// The subscribers attached with `NotificationsWithSubscribers`.
    Subscribers,
}

/// One way a request breaks a documented rule, with the members it rewrites.
struct Violation {
    case: &'static str,
    group: Group,
    /// Rewrites the whole `CreateBudget` request in place.
    apply: fn(&mut Value),
}

/// The valid request every row below starts from: a monthly cost budget with a fixed
/// limit and one notification whose subscriber is an email address.
fn base_request(account_id: &str, name: &str) -> Value {
    json!({
        "AccountId": account_id,
        "Budget": {
            "BudgetName": name,
            "TimeUnit": "MONTHLY",
            "BudgetType": "COST",
            "BudgetLimit": {"Amount": "100", "Unit": "USD"}
        },
        "NotificationsWithSubscribers": [{
            "Notification": {
                "NotificationType": "ACTUAL",
                "ComparisonOperator": "GREATER_THAN",
                "Threshold": 80.0
            },
            "Subscribers": [{"SubscriptionType": "EMAIL", "Address": "ops@example.com"}]
        }]
    })
}

/// Every violation, one per documented rule and per way of breaking it. Each value is
/// inside the constraints the MODEL declares (the enums, the `@range(1, 60)` on
/// `AdjustmentPeriod`, the `SubscriberAddress` pattern), so each one reaches the handler
/// and is answered with [`REFUSAL`] rather than at request validation.
fn violations() -> Vec<Violation> {
    vec![
        Violation {
            // "BudgetLimit is required for cost or usage budgets" (`Budget$BudgetLimit`).
            case: "a cost budget carrying no limit at all",
            group: Group::Limit,
            apply: |request| {
                request["Budget"]
                    .as_object_mut()
                    .expect("the budget is an object")
                    .remove("BudgetLimit");
            },
        },
        Violation {
            // "You can't use BudgetLimit with PlannedBudgetLimits for CreateBudget and
            // UpdateBudget actions" (`Budget$BudgetLimit`).
            case: "a BudgetLimit beside a PlannedBudgetLimits map",
            group: Group::Limit,
            apply: |request| {
                request["Budget"]["PlannedBudgetLimits"] = json!({
                    civil_secs("2024-02-01T00:00:00Z").to_string(): {
                        "Amount": "100", "Unit": "USD"
                    }
                });
            },
        },
        Violation {
            // "The key is the start of the month, UTC in epoch seconds"
            // (`Budget$PlannedBudgetLimits`).
            case: "a PlannedBudgetLimits key that is not the start of a month",
            group: Group::Limit,
            apply: |request| {
                let budget = request["Budget"]
                    .as_object_mut()
                    .expect("the budget is an object");
                budget.remove("BudgetLimit");
                budget.insert(
                    "PlannedBudgetLimits".to_string(),
                    json!({
                        civil_secs("2024-02-15T00:00:00Z").to_string(): {
                            "Amount": "100", "Unit": "USD"
                        }
                    }),
                );
            },
        },
        Violation {
            // "The end date must come before 06/15/87 00:00 UTC" (`Budget$TimePeriod`).
            case: "an End after the last instant a budget may end at",
            group: Group::Period,
            apply: |request| {
                request["Budget"]["TimePeriod"] = json!({
                    "Start": civil_secs("2030-01-01T00:00:00Z"),
                    "End": civil_secs("2100-01-01T00:00:00Z")
                });
            },
        },
        Violation {
            // "The start date must come before the end date" (`Budget$TimePeriod`).
            case: "a Start after the End",
            group: Group::Period,
            apply: |request| {
                request["Budget"]["TimePeriod"] = json!({
                    "Start": civil_secs("2030-01-01T00:00:00Z"),
                    "End": civil_secs("2029-01-01T00:00:00Z")
                });
            },
        },
        Violation {
            // `HistoricalOptions` are "the parameters that define or describe the
            // historical data that your auto-adjusting budget is based on"
            // (`AutoAdjustData$HistoricalOptions`), so the HISTORICAL type needs them.
            case: "a HISTORICAL auto-adjust type with no HistoricalOptions",
            group: Group::AutoAdjust,
            apply: |request| {
                request["Budget"]["AutoAdjustData"] = json!({"AutoAdjustType": "HISTORICAL"});
            },
        },
        Violation {
            case: "a FORECAST auto-adjust type carrying HistoricalOptions",
            group: Group::AutoAdjust,
            apply: |request| {
                request["Budget"]["AutoAdjustData"] = json!({
                    "AutoAdjustType": "FORECAST",
                    "HistoricalOptions": {"BudgetAdjustmentPeriod": 3}
                });
            },
        },
        Violation {
            // "For the MONTHLY granularity, the maximum value is 12"
            // (`HistoricalOptions$BudgetAdjustmentPeriod`); the base budget is monthly,
            // and 13 is still inside the modelled `@range(1, 60)`.
            case: "a BudgetAdjustmentPeriod past the maximum its granularity allows",
            group: Group::AutoAdjust,
            apply: |request| {
                request["Budget"]["AutoAdjustData"] = json!({
                    "AutoAdjustType": "HISTORICAL",
                    "HistoricalOptions": {"BudgetAdjustmentPeriod": 13}
                });
            },
        },
        Violation {
            // "Either FilterExpression and Metrics or CostFilters and CostTypes, not
            // both or a different combination" (`CreateBudget`).
            case: "a FilterExpression beside the CostTypes syntax",
            group: Group::Filters,
            apply: |request| {
                request["Budget"]["CostTypes"] = json!({"IncludeTax": true});
                request["Budget"]["FilterExpression"] =
                    json!({"Dimensions": {"Key": "REGION", "Values": ["us-east-1"]}});
            },
        },
        Violation {
            // A subscriber "consists of a subscription type and either an Amazon SNS
            // topic or an email address" (`Subscriber`).
            case: "an EMAIL subscriber whose address is not an email address",
            group: Group::Subscribers,
            apply: |request| {
                request["NotificationsWithSubscribers"][0]["Subscribers"] =
                    json!([{"SubscriptionType": "EMAIL", "Address": "not-an-email"}]);
            },
        },
        Violation {
            case: "an SNS subscriber whose address is not an SNS topic ARN",
            group: Group::Subscribers,
            apply: |request| {
                request["NotificationsWithSubscribers"][0]["Subscribers"] =
                    json!([{"SubscriptionType": "SNS", "Address": "budget-alerts"}]);
            },
        },
    ]
}

/// RULE: the rules a `Budget` must satisfy are a CONJUNCTION — a request that breaks any
/// one of them is refused however the rest of its members are written, so a second
/// violation never buys a request an acceptance, and a refused `CreateBudget` persists
/// neither the budget nor the notifications and subscribers supplied with it.
#[tokio::test]
async fn the_documented_budget_rules_are_a_conjunction() {
    let account_id = caller_account();

    // The request every row starts from is itself accepted, so a refusal below is about
    // the violation applied to it and not about the base.
    assert_create(
        "the base request, breaking no rule",
        &base_request(&account_id, "conjunction-base"),
        Verdict::Accepted,
    )
    .await;

    let rows = violations();
    for (index, row) in rows.iter().enumerate() {
        // One violation on its own.
        let mut request = base_request(&account_id, "conjunction-single");
        (row.apply)(&mut request);
        assert_create(row.case, &request, Verdict::Refused).await;

        // The same violation beside every violation of another group: each pair gets
        // both rules wrong, so each pair is refused too.
        for other in rows.iter().skip(index + 1) {
            if other.group == row.group {
                continue;
            }
            let mut request = base_request(&account_id, "conjunction-pair");
            (row.apply)(&mut request);
            (other.apply)(&mut request);
            let case = format!("{} together with {}", row.case, other.case);
            assert_create(&case, &request, Verdict::Refused).await;
        }
    }
}
