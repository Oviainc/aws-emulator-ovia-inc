// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the ALARM STATE a notification is reported in:
//! `Notification$NotificationState` is the SERVICE's own reading of the spend behind the
//! budget, not a value a request chooses.
//!
//! The RULE asserted here: `NotificationState` "specifies whether this notification is in
//! alarm. If a budget notification is in the `ALARM` state, you passed the set threshold
//! for the budget", so a write stores nothing a caller sent for it and every read reports
//! the state the service computes — `ALARM` exactly while the spend the notification
//! watches (`NotificationType`: actual or forecasted) stands to the amount its `Threshold`
//! names (read through `ThresholdType`: an absolute amount, or that percentage of the
//! budgeted amount) as its `ComparisonOperator` says, and `OK` otherwise, including
//! wherever the budget reports no such figure to compare at all.
//!
//! The table spans the class rather than one recorded request: its rows differ along the
//! notification type, the comparison operator (every member of the model's catalog), the
//! threshold (both published `@range` bounds and fractional values in between), the
//! threshold type (each `ThresholdType` the model lists, plus absent), and the budget the
//! threshold is measured against — a fixed limit, a usage limit in another unit, the
//! documented utilization default, the planned entry in force, and an auto-adjusting
//! budget that reports no spend at all. Each row is then crossed with the
//! `NotificationState` the REQUEST carried (absent, `OK`, `ALARM`) and with the write that
//! establishes the notification (`CreateBudget` and `CreateNotification`), because the
//! answer may not depend on either.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// The catalogs are the model-derived constants the scaffold generated, so a table built
/// from them cannot drift from the model.
use service_com_amazonaws_budgets_awsbudgetservicegateway::ops::types::NOTIFICATION_STATE_VALUES;
use service_com_amazonaws_budgets_awsbudgetservicegateway::state::{
    ComAmazonawsBudgetsAwsbudgetservicegatewayState, ATTRIBUTED_SPEND_AMOUNT,
    NOTIFICATION_STATE_ALARM, NOTIFICATION_STATE_OK,
};

use super::budget_lifecycle::{call, caller_account, router_with_engine, same};
use super::budget_read_projection::month_start_offset;

/// `Notification$Threshold` is `@range(min: 0, max: 15000000000000)`
/// (`smithy_query(query="shape",shape="NotificationThreshold")`) — both bounds appear in
/// the table below.
const THRESHOLD_MAX: f64 = 15_000_000_000_000.0;

/// One row: the budget the threshold is measured against, what the notification alerts
/// on, and the alarm state the service reports for that pairing.
struct Row {
    case: &'static str,
    /// The `Budget` to create, without its `BudgetName` (the test names each one).
    budget: Value,
    /// The notification's identity members — no `NotificationState`: the crossing below
    /// adds every value a request can carry for it.
    notification: Value,
    /// The `NotificationState` every read must report.
    expected: &'static str,
}

/// A `COST` budget on a monthly period with a 100 USD limit — the budget the rows that
/// vary only the notification measure their thresholds against.
fn cost_budget() -> Value {
    json!({
        "TimeUnit": "MONTHLY",
        "BudgetType": "COST",
        "BudgetLimit": {"Amount": "100", "Unit": "USD"}
    })
}

/// A `COST` budget whose amount the service auto-adjusts from cost data this emulator
/// does not hold, so its read reports no limit and no `CalculatedSpend`: a budget with
/// nothing to compare a threshold against.
fn auto_adjusting_budget() -> Value {
    json!({
        "TimeUnit": "MONTHLY",
        "BudgetType": "COST",
        "AutoAdjustData": {
            "AutoAdjustType": "HISTORICAL",
            "HistoricalOptions": {"BudgetAdjustmentPeriod": 3}
        }
    })
}

/// The table.
///
/// The spend every row is measured against is the cost or usage this emulator has
/// attributed to the budget — nothing spends behind a budget here, so that figure is
/// [`ATTRIBUTED_SPEND_AMOUNT`], which each row's assertion reads back off the budget
/// itself — and no forecast at all, which is why the `FORECASTED` rows have nothing to
/// compare however their threshold is set.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "actual spend over a percentage threshold",
            budget: cost_budget(),
            notification: json!({
                "NotificationType": "ACTUAL",
                "ComparisonOperator": "GREATER_THAN",
                "Threshold": 80,
                "ThresholdType": "PERCENTAGE"
            }),
            // 80% of 100 USD is 80 USD, and the budget has spent none of it.
            expected: NOTIFICATION_STATE_OK,
        },
        Row {
            case: "actual spend under a percentage threshold",
            budget: cost_budget(),
            notification: json!({
                "NotificationType": "ACTUAL",
                "ComparisonOperator": "LESS_THAN",
                "Threshold": 80,
                "ThresholdType": "PERCENTAGE"
            }),
            // The same 80 USD, and nothing spent is below it.
            expected: NOTIFICATION_STATE_ALARM,
        },
        Row {
            case: "a percentage threshold at the published minimum, compared below",
            budget: cost_budget(),
            // No `ThresholdType`: "thresholds are always a percentage".
            notification: json!({
                "NotificationType": "ACTUAL",
                "ComparisonOperator": "LESS_THAN",
                "Threshold": 0
            }),
            // 0% of the limit is 0, and nothing spent is not below it.
            expected: NOTIFICATION_STATE_OK,
        },
        Row {
            case: "a percentage threshold at the published minimum, compared equal",
            budget: cost_budget(),
            notification: json!({
                "NotificationType": "ACTUAL",
                "ComparisonOperator": "EQUAL_TO",
                "Threshold": 0
            }),
            expected: NOTIFICATION_STATE_ALARM,
        },
        Row {
            case: "an absolute threshold the spend is equal to",
            budget: cost_budget(),
            notification: json!({
                "NotificationType": "ACTUAL",
                "ComparisonOperator": "EQUAL_TO",
                "Threshold": 0,
                "ThresholdType": "ABSOLUTE_VALUE"
            }),
            expected: NOTIFICATION_STATE_ALARM,
        },
        Row {
            case: "a fractional absolute threshold the spend is under",
            budget: cost_budget(),
            notification: json!({
                "NotificationType": "ACTUAL",
                "ComparisonOperator": "LESS_THAN",
                "Threshold": 0.5,
                "ThresholdType": "ABSOLUTE_VALUE"
            }),
            expected: NOTIFICATION_STATE_ALARM,
        },
        Row {
            case: "an absolute threshold at the published maximum, compared above",
            budget: cost_budget(),
            notification: json!({
                "NotificationType": "ACTUAL",
                "ComparisonOperator": "GREATER_THAN",
                "Threshold": THRESHOLD_MAX,
                "ThresholdType": "ABSOLUTE_VALUE"
            }),
            expected: NOTIFICATION_STATE_OK,
        },
        Row {
            case: "a percentage threshold at the published maximum, compared below",
            budget: cost_budget(),
            notification: json!({
                "NotificationType": "ACTUAL",
                "ComparisonOperator": "LESS_THAN",
                "Threshold": THRESHOLD_MAX,
                "ThresholdType": "PERCENTAGE"
            }),
            expected: NOTIFICATION_STATE_ALARM,
        },
        Row {
            case: "a forecasted notification under a percentage threshold, with no \
                   forecast to compare",
            budget: cost_budget(),
            notification: json!({
                "NotificationType": "FORECASTED",
                "ComparisonOperator": "LESS_THAN",
                "Threshold": 80,
                "ThresholdType": "PERCENTAGE"
            }),
            expected: NOTIFICATION_STATE_OK,
        },
        Row {
            case: "a forecasted notification at the absolute threshold an actual one \
                   would be in alarm at",
            budget: cost_budget(),
            notification: json!({
                "NotificationType": "FORECASTED",
                "ComparisonOperator": "EQUAL_TO",
                "Threshold": 0,
                "ThresholdType": "ABSOLUTE_VALUE"
            }),
            // The same row as "an absolute threshold the spend is equal to", on the
            // forecast instead: no forecast is reported for a budget here, so there is
            // nothing that passed the threshold.
            expected: NOTIFICATION_STATE_OK,
        },
        Row {
            case: "a utilization budget's percentage threshold, taken of the documented \
                   default limit",
            budget: json!({"TimeUnit": "MONTHLY", "BudgetType": "RI_UTILIZATION"}),
            notification: json!({
                "NotificationType": "ACTUAL",
                "ComparisonOperator": "LESS_THAN",
                "Threshold": 75
            }),
            // "RI and Savings Plans utilization or coverage budgets default to `100`"
            // (`Budget$BudgetLimit`), so the threshold is 75 and no utilization is below
            // it.
            expected: NOTIFICATION_STATE_ALARM,
        },
        Row {
            case: "a utilization budget's percentage threshold, compared above",
            budget: json!({"TimeUnit": "QUARTERLY", "BudgetType": "SAVINGS_PLANS_COVERAGE"}),
            notification: json!({
                "NotificationType": "ACTUAL",
                "ComparisonOperator": "GREATER_THAN",
                "Threshold": 0.5
            }),
            expected: NOTIFICATION_STATE_OK,
        },
        Row {
            case: "a usage budget measured in GB, under a fractional absolute threshold",
            budget: json!({
                "TimeUnit": "DAILY",
                "BudgetType": "USAGE",
                "BudgetLimit": {"Amount": "3.5", "Unit": "GB"}
            }),
            notification: json!({
                "NotificationType": "ACTUAL",
                "ComparisonOperator": "LESS_THAN",
                "Threshold": 1.25,
                "ThresholdType": "ABSOLUTE_VALUE"
            }),
            expected: NOTIFICATION_STATE_ALARM,
        },
        Row {
            case: "a planned-limit budget: the percentage is of the entry in force",
            budget: json!({
                "TimeUnit": "MONTHLY",
                "BudgetType": "COST",
                "PlannedBudgetLimits": {
                    month_start_offset(-1).to_string(): {"Amount": "20", "Unit": "USD"}
                }
            }),
            notification: json!({
                "NotificationType": "ACTUAL",
                "ComparisonOperator": "LESS_THAN",
                "Threshold": 50,
                "ThresholdType": "PERCENTAGE"
            }),
            // 50% of the 20 USD in force is 10 USD, which nothing spent is below.
            expected: NOTIFICATION_STATE_ALARM,
        },
        Row {
            case: "an auto-adjusting budget, which reports no spend to compare at all",
            budget: auto_adjusting_budget(),
            notification: json!({
                "NotificationType": "ACTUAL",
                "ComparisonOperator": "LESS_THAN",
                "Threshold": 50,
                "ThresholdType": "ABSOLUTE_VALUE"
            }),
            // The same comparison that is in alarm on every budget above: what differs is
            // that this budget reports no limit, so no spend is reported for it either
            // and no threshold of it was ever measured.
            expected: NOTIFICATION_STATE_OK,
        },
        Row {
            case: "a percentage threshold of a budget that reports no limit to take it of",
            budget: auto_adjusting_budget(),
            notification: json!({
                "NotificationType": "ACTUAL",
                "ComparisonOperator": "LESS_THAN",
                "Threshold": 50,
                "ThresholdType": "PERCENTAGE"
            }),
            expected: NOTIFICATION_STATE_OK,
        },
    ]
}

/// The subscriber every notification below is created with: "each notification must have
/// at least one subscriber", and an email mailbox is an address nothing is delivered to.
fn subscribers() -> Value {
    json!([{"SubscriptionType": "EMAIL", "Address": "ops@example.com"}])
}

/// Which write establishes the notification — the answer may not depend on it.
#[derive(Clone, Copy, Debug)]
enum Attach {
    /// Attached to the budget by `CreateBudget`'s `NotificationsWithSubscribers`.
    WithBudget,
    /// Added to an existing budget by `CreateNotification`.
    AfterBudget,
}

/// The notifications `DescribeNotificationsForBudget` reports for one budget.
async fn reported_for_budget(
    router: &MultiServiceRouter,
    case: &str,
    account_id: &str,
    name: &str,
) -> Vec<Value> {
    let (status, body) = call(
        router,
        "DescribeNotificationsForBudget",
        &json!({"AccountId": account_id, "BudgetName": name}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeNotificationsForBudget to answer 200, \
         got {status} {body}"
    );
    body["Notifications"].as_array().cloned().unwrap_or_default()
}

/// The notifications `DescribeBudgetNotificationsForAccount` reports for one budget of the
/// account — the second read path, which must report the same state.
async fn reported_for_account(
    router: &MultiServiceRouter,
    case: &str,
    account_id: &str,
    name: &str,
) -> Vec<Value> {
    let (status, body) = call(
        router,
        "DescribeBudgetNotificationsForAccount",
        &json!({"AccountId": account_id}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeBudgetNotificationsForAccount to answer 200, \
         got {status} {body}"
    );
    body["BudgetNotificationsForAccount"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|entry| entry["BudgetName"].as_str() == Some(name))
        .map(|entry| entry["Notifications"].as_array().cloned().unwrap_or_default())
        .unwrap_or_default()
}

/// The `Budget` `DescribeBudget` reports for `name` — the limit and the spend every alarm
/// state below is read off, asserted beside the state so the table cannot be passing
/// against figures other than the ones the budget reports.
async fn reported_budget(
    router: &MultiServiceRouter,
    case: &str,
    account_id: &str,
    name: &str,
) -> Value {
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

/// Assert the figures a threshold is measured against, as the budget itself reports them:
/// a budget that reports a `BudgetLimit` reports the cost or usage attributed to it
/// ([`ATTRIBUTED_SPEND_AMOUNT`]) in that limit's unit and no forecast at all, and one that
/// reports no limit reports no `CalculatedSpend` for a threshold to be measured against.
fn assert_figures(case: &str, budget: &Value) {
    let limit = &budget["BudgetLimit"];
    let spend = &budget["CalculatedSpend"];
    if limit.is_null() {
        assert!(
            spend.is_null(),
            "case {case:?}: a budget reporting no limit must report no spend, got {spend}"
        );
        return;
    }
    let want = json!({"Amount": ATTRIBUTED_SPEND_AMOUNT, "Unit": limit["Unit"]});
    assert!(
        same(&spend["ActualSpend"], &want),
        "case {case:?}: expected the attributed spend {want}, got {spend}"
    );
    assert!(
        spend["ForecastedSpend"].is_null(),
        "case {case:?}: expected no forecast to be reported, got {spend}"
    );
}

/// The `NotificationState` the STORE holds for every notification of `name`, read from the
/// state struct directly: what the write persisted, independently of any read's
/// projection.
fn stored_states(
    engine: &Arc<engine::InMemoryEngine>,
    account_id: &str,
    name: &str,
) -> Vec<Option<String>> {
    let state = engine.state::<ComAmazonawsBudgetsAwsbudgetservicegatewayState>();
    let store = state.lock();
    store
        .budgets
        .get(&(account_id.to_string(), name.to_string()))
        .expect("the budget under test is stored")
        .notifications
        .iter()
        .map(|held| held.notification.notification_state.clone())
        .collect()
}

/// Create `name` holding `notification` (with [`subscribers`]) by the write `attach`
/// names, and fail the test if either call did not take.
async fn given_notification(
    router: &MultiServiceRouter,
    case: &str,
    account_id: &str,
    name: &str,
    budget: &Value,
    notification: &Value,
    attach: Attach,
) {
    let mut budget = budget.clone();
    budget["BudgetName"] = json!(name);
    let mut create = json!({"AccountId": account_id, "Budget": budget});
    if matches!(attach, Attach::WithBudget) {
        create["NotificationsWithSubscribers"] = json!([{
            "Notification": notification,
            "Subscribers": subscribers()
        }]);
    }
    let (status, body) = call(router, "CreateBudget", &create).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateBudget to answer 200, got {status} {body}"
    );
    if matches!(attach, Attach::AfterBudget) {
        let (status, body) = call(
            router,
            "CreateNotification",
            &json!({
                "AccountId": account_id,
                "BudgetName": name,
                "Notification": notification,
                "Subscribers": subscribers()
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateNotification to answer 200, got {status} {body}"
        );
    }
}

/// RULE: the alarm state a read reports is the one the SERVICE computes from the spend
/// behind the budget against the threshold the notification alerts on — whatever
/// `NotificationState` the request carried, and whichever write created the notification.
/// Nothing a caller sent for it is persisted, and both read paths report the same state.
#[tokio::test]
async fn the_reported_alarm_state_is_computed_from_the_spend_not_supplied() {
    // Absent, plus every `NotificationState` the model lists.
    let supplied: Vec<Option<&str>> = ::std::iter::once(None)
        .chain(NOTIFICATION_STATE_VALUES.iter().map(|state| Some(*state)))
        .collect();

    for row in rows() {
        for sent in &supplied {
            for attach in [Attach::WithBudget, Attach::AfterBudget] {
                let case = format!(
                    "{} / sent {} / {attach:?}",
                    row.case,
                    sent.unwrap_or("absent")
                );
                // One store per case, so no case can see another's budget.
                let (router, engine) = router_with_engine();
                let account_id = caller_account();
                let name = "alarm-state";

                let mut notification = row.notification.clone();
                if let Some(state) = sent {
                    notification["NotificationState"] = json!(state);
                }
                given_notification(
                    &router,
                    &case,
                    &account_id,
                    name,
                    &row.budget,
                    &notification,
                    attach,
                )
                .await;

                // The limit and the spend the state was read off, as the budget itself
                // reports them.
                let budget = reported_budget(&router, &case, &account_id, name).await;
                assert_figures(&case, &budget);

                for (path, reported) in [
                    (
                        "DescribeNotificationsForBudget",
                        reported_for_budget(&router, &case, &account_id, name).await,
                    ),
                    (
                        "DescribeBudgetNotificationsForAccount",
                        reported_for_account(&router, &case, &account_id, name).await,
                    ),
                ] {
                    assert_eq!(
                        reported.len(),
                        1,
                        "case {case:?} on {path}: expected exactly one notification, \
                         got {reported:?}"
                    );
                    let read = &reported[0];
                    assert_eq!(
                        read["NotificationState"].as_str(),
                        Some(row.expected),
                        "case {case:?} on {path}: expected NotificationState {:?}, got {read}",
                        row.expected
                    );
                    // The members the caller DOES own are untouched by the derivation.
                    for member in ["NotificationType", "ComparisonOperator", "Threshold"] {
                        assert!(
                            same(&read[member], &row.notification[member]),
                            "case {case:?} on {path}: expected {member} = {}, got {}",
                            row.notification[member],
                            read[member]
                        );
                    }
                }

                // Nothing the request carried for the alarm state reached the store, and
                // no read wrote one into it either.
                assert_eq!(
                    stored_states(&engine, &account_id, name),
                    vec![None],
                    "case {case:?}: expected the store to hold no NotificationState"
                );
            }
        }
    }
}

/// RULE, the update path: `UpdateNotification` changes the reported alarm state only by
/// changing what the notification alerts on — the state the replacement names is as
/// irrelevant as the one the create named.
///
/// The rows cross the two directions the state can move (into alarm and out of it) with
/// every `NotificationState` the replacement can carry.
#[tokio::test]
async fn an_update_cannot_set_the_alarm_state_only_what_is_alerted_on() {
    // A `COST` budget with a 100 USD limit and no spend: "below 80%" is in alarm, "above
    // 80%" is not, so an update between the two moves the state.
    let in_alarm = json!({
        "NotificationType": "ACTUAL",
        "ComparisonOperator": "LESS_THAN",
        "Threshold": 80,
        "ThresholdType": "PERCENTAGE"
    });
    let out_of_alarm = json!({
        "NotificationType": "ACTUAL",
        "ComparisonOperator": "GREATER_THAN",
        "Threshold": 80,
        "ThresholdType": "PERCENTAGE"
    });
    let directions = [
        ("out of alarm", &in_alarm, &out_of_alarm, NOTIFICATION_STATE_OK),
        (
            "into alarm",
            &out_of_alarm,
            &in_alarm,
            NOTIFICATION_STATE_ALARM,
        ),
    ];
    let supplied: Vec<Option<&str>> = ::std::iter::once(None)
        .chain(NOTIFICATION_STATE_VALUES.iter().map(|state| Some(*state)))
        .collect();

    for (direction, created, replacement, expected) in directions {
        for sent in &supplied {
            let case = format!("{direction} / replacement names {}", sent.unwrap_or("absent"));
            let (router, engine) = router_with_engine();
            let account_id = caller_account();
            let name = "updated-alarm";
            given_notification(
                &router,
                &case,
                &account_id,
                name,
                &cost_budget(),
                created,
                Attach::WithBudget,
            )
            .await;

            let mut new = replacement.clone();
            if let Some(state) = sent {
                new["NotificationState"] = json!(state);
            }
            let (status, body) = call(
                &router,
                "UpdateNotification",
                &json!({
                    "AccountId": &account_id,
                    "BudgetName": name,
                    "OldNotification": created,
                    "NewNotification": new
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected UpdateNotification to answer 200, got {status} {body}"
            );

            let reported = reported_for_budget(&router, &case, &account_id, name).await;
            assert_eq!(
                reported.len(),
                1,
                "case {case:?}: expected exactly one notification, got {reported:?}"
            );
            assert_eq!(
                reported[0]["NotificationState"].as_str(),
                Some(expected),
                "case {case:?}: expected NotificationState {expected:?}, got {}",
                reported[0]
            );
            assert_eq!(
                stored_states(&engine, &account_id, name),
                vec![None],
                "case {case:?}: expected the store to hold no NotificationState"
            );
        }
    }
}

/// RULE: reporting the alarm state changes nothing a later caller sees — reading it again
/// reports the same state, for a notification in alarm and one out of it.
#[tokio::test]
async fn reading_the_alarm_state_twice_reports_the_same_state() {
    for row in rows() {
        let (router, _engine) = router_with_engine();
        let account_id = caller_account();
        let name = "repeat-read";
        given_notification(
            &router,
            row.case,
            &account_id,
            name,
            &row.budget,
            &row.notification,
            Attach::AfterBudget,
        )
        .await;

        let first = reported_for_budget(&router, row.case, &account_id, name).await;
        let second = reported_for_budget(&router, row.case, &account_id, name).await;
        assert!(
            same(&json!(first), &json!(second)),
            "case {:?}: expected the second read to report {first:?}, got {second:?}",
            row.case
        );
        assert_eq!(
            second[0]["NotificationState"].as_str(),
            Some(row.expected),
            "case {:?}: expected NotificationState {:?}, got {}",
            row.case,
            row.expected,
            second[0]
        );
    }
}
