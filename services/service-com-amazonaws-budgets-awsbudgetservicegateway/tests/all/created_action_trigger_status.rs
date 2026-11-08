// SPDX-License-Identifier: CC0-1.0

//! The `ActionStatus` a CREATED budget action carries, which the service computes from the
//! action's own trigger threshold and the figures its budget reads back with.
//!
//! The RULE asserted here: `ActionThreshold` is "the trigger threshold of the action", so a
//! created action is ARMED — `ActionStatus$STANDBY` — while the spend the budget tracks has
//! not reached that threshold, and `ActionStatus$PENDING` once it has AND the action's
//! execution needs the manual approval its `ApprovalModel` asks for ("this specifies if the
//! action needs manual or automatic approval"), that approval being what
//! `ExecuteBudgetAction`'s `APPROVE_BUDGET_ACTION` gives and nothing else here does. Which
//! spend the action
//! watches is its `NotificationType` ("it must be `ACTUAL` or `FORECASTED`") and the amount
//! it is measured against is `ActionThresholdValue` read through `ActionThresholdType`
//! (`PERCENTAGE` being that percentage of the budgeted amount), the same two readings a
//! notification's alarm state is made of (`tests/all/notification_alarm_state.rs`) — and
//! where the budget's figures state no comparison at all, nothing has reached the threshold
//! and the action is armed.
//!
//! Three facts about this emulator make the rule observable: nothing spends behind a budget,
//! so every budget reports an `ActualSpend` of zero; no forecast is reported for one
//! (`CalculatedSpend$ForecastedSpend` is "predicted based on your historical usage
//! profile", and there is no usage history here); and nothing performs an action's
//! execution, so an `AUTOMATIC` action whose trigger is met is left armed rather than given
//! a status for work nothing did. So a `MANUAL` `ACTUAL` action whose threshold is zero has
//! its approval outstanding, every higher threshold has not been reached, a `FORECASTED`
//! action watches a figure the budget does not state, and an `AUTOMATIC` action stays armed.
//!
//! The rows span what the rule generalises over: the notification type, the approval model,
//! the threshold type,
//! and the threshold value at each published `@range` bound of `NotificationThreshold` (0
//! and 15000000000000 — `smithy_query(query="shape",shape="ActionThreshold")`), just inside
//! the minimum and between the bounds; the last test walks one past each bound, which
//! request validation refuses before any status is stamped. Every row is read back through
//! `DescribeBudgetAction`, the per-budget listing and the account-wide listing, so no read
//! reports a status another does not.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_budgets_awsbudgetservicegateway::state::{
    ACTION_STATUS_PENDING, ACTION_STATUS_STANDBY,
};

use super::budget_action_execution::{email_subscriber, iam_definition};
use super::budget_lifecycle::{caller_account, call, is_error, router};
use super::client_error_envelopes::message;

/// The published bounds of `NotificationThreshold`, which `ActionThresholdValue` targets.
const THRESHOLD_MIN: f64 = 0.0;
const THRESHOLD_MAX: f64 = 15_000_000_000_000.0;

/// The budgeted amount every budget below is created with, in US dollars — the base a
/// `PERCENTAGE` threshold is a percentage of.
const BUDGET_AMOUNT: &str = "50";

/// Create a budget holding a fixed limit, so every row's threshold has an amount to be
/// read against.
async fn seed_budget(router: &MultiServiceRouter, account_id: &str, budget_name: &str) {
    let (status, body) = call(
        router,
        "CreateBudget",
        &json!({
            "AccountId": account_id,
            "Budget": {
                "BudgetName": budget_name,
                "TimeUnit": "MONTHLY",
                "BudgetType": "COST",
                "BudgetLimit": {"Amount": BUDGET_AMOUNT, "Unit": "USD"}
            }
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: expected CreateBudget({budget_name}) to answer 200, got {status} {body}"
    );
}

/// A `CreateBudgetAction` body for one row.
fn create_body(
    account_id: &str,
    budget_name: &str,
    notification_type: &str,
    threshold_value: f64,
    threshold_type: &str,
    approval_model: &str,
) -> Value {
    json!({
        "AccountId": account_id,
        "BudgetName": budget_name,
        "NotificationType": notification_type,
        "ActionType": "APPLY_IAM_POLICY",
        "ActionThreshold": {
            "ActionThresholdValue": threshold_value,
            "ActionThresholdType": threshold_type
        },
        "Definition": iam_definition(),
        "ExecutionRoleArn": format!("arn:aws:iam::{account_id}:role/budget-action-role"),
        "ApprovalModel": approval_model,
        "Subscribers": [email_subscriber(0)]
    })
}

/// The `Status` each read reports for the one action of `budget_name`: the single read, the
/// per-budget listing and the account-wide listing, having checked the three agree.
async fn status_of(
    router: &MultiServiceRouter,
    account_id: &str,
    budget_name: &str,
    action_id: &str,
    case: &str,
) -> String {
    let (status, read) = call(
        router,
        "DescribeBudgetAction",
        &json!({
            "AccountId": account_id,
            "BudgetName": budget_name,
            "ActionId": action_id
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the action to be readable, got {status} {read}"
    );
    let single = read["Action"]["Status"].as_str().unwrap_or_default().to_string();

    for (reader, request) in [
        (
            "DescribeBudgetActionsForBudget",
            json!({"AccountId": account_id, "BudgetName": budget_name}),
        ),
        (
            "DescribeBudgetActionsForAccount",
            json!({"AccountId": account_id}),
        ),
    ] {
        let (status, body) = call(router, reader, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {reader} to answer 200, got {status} {body}"
        );
        let listed = body["Actions"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .find(|action| action["ActionId"] == json!(action_id))
            .unwrap_or(Value::Null);
        let from_list = listed["Status"].as_str().unwrap_or_default().to_string();
        assert_eq!(
            from_list, single,
            "case {case:?}: expected {reader} to report the status the single read does, \
             {single}, got {from_list}"
        );
    }
    single
}

/// RULE: the status a created action carries is the service's reading of its trigger
/// threshold against the spend the budget tracks, under the approval its `ApprovalModel`
/// asks for — armed until that spend reaches the threshold, and waiting for its approval
/// once it has.
#[tokio::test]
async fn a_created_action_carries_the_status_its_trigger_threshold_gives_it() {
    let router = router();
    let account_id = caller_account();
    // Rows differ along the notification type, the approval model, the threshold type and
    // the threshold value (both published bounds, just inside the minimum, and a value
    // between them).
    let rows: [(&str, &str, f64, &str, &str, &str); 13] = [
        (
            "a MANUAL ACTUAL action at the published minimum threshold, as an absolute amount",
            "ACTUAL",
            THRESHOLD_MIN,
            "ABSOLUTE_VALUE",
            "MANUAL",
            ACTION_STATUS_PENDING,
        ),
        (
            "a MANUAL ACTUAL action at the published minimum threshold, as a percentage",
            "ACTUAL",
            THRESHOLD_MIN,
            "PERCENTAGE",
            "MANUAL",
            ACTION_STATUS_PENDING,
        ),
        (
            // Nothing here performs an action's execution, so an action that needs no
            // approval is left armed even where its trigger is already met.
            "an AUTOMATIC ACTUAL action at the published minimum threshold",
            "ACTUAL",
            THRESHOLD_MIN,
            "ABSOLUTE_VALUE",
            "AUTOMATIC",
            ACTION_STATUS_STANDBY,
        ),
        (
            "an AUTOMATIC ACTUAL action at the published minimum threshold, as a percentage",
            "ACTUAL",
            THRESHOLD_MIN,
            "PERCENTAGE",
            "AUTOMATIC",
            ACTION_STATUS_STANDBY,
        ),
        (
            "a MANUAL ACTUAL action just inside the minimum, as an absolute amount",
            "ACTUAL",
            0.000_1,
            "ABSOLUTE_VALUE",
            "MANUAL",
            ACTION_STATUS_STANDBY,
        ),
        (
            "a MANUAL ACTUAL action just inside the minimum, as a percentage",
            "ACTUAL",
            0.000_1,
            "PERCENTAGE",
            "MANUAL",
            ACTION_STATUS_STANDBY,
        ),
        (
            "a MANUAL ACTUAL action at a percentage between the bounds",
            "ACTUAL",
            80.0,
            "PERCENTAGE",
            "MANUAL",
            ACTION_STATUS_STANDBY,
        ),
        (
            "an AUTOMATIC ACTUAL action at a percentage between the bounds",
            "ACTUAL",
            80.0,
            "PERCENTAGE",
            "AUTOMATIC",
            ACTION_STATUS_STANDBY,
        ),
        (
            "a MANUAL ACTUAL action at the published maximum threshold, as an absolute amount",
            "ACTUAL",
            THRESHOLD_MAX,
            "ABSOLUTE_VALUE",
            "MANUAL",
            ACTION_STATUS_STANDBY,
        ),
        (
            "a MANUAL ACTUAL action at the published maximum threshold, as a percentage",
            "ACTUAL",
            THRESHOLD_MAX,
            "PERCENTAGE",
            "MANUAL",
            ACTION_STATUS_STANDBY,
        ),
        (
            // No forecast is reported for a budget here, so a FORECASTED action watches a
            // figure the budget does not state: nothing has reached its threshold.
            "a MANUAL FORECASTED action at the published minimum threshold, as an absolute amount",
            "FORECASTED",
            THRESHOLD_MIN,
            "ABSOLUTE_VALUE",
            "MANUAL",
            ACTION_STATUS_STANDBY,
        ),
        (
            "a MANUAL FORECASTED action at the published minimum threshold, as a percentage",
            "FORECASTED",
            THRESHOLD_MIN,
            "PERCENTAGE",
            "MANUAL",
            ACTION_STATUS_STANDBY,
        ),
        (
            "a MANUAL FORECASTED action at a percentage between the bounds",
            "FORECASTED",
            80.0,
            "PERCENTAGE",
            "MANUAL",
            ACTION_STATUS_STANDBY,
        ),
    ];

    for (
        position,
        (case, notification_type, threshold_value, threshold_type, approval_model, expected),
    ) in rows.iter().enumerate()
    {
        let budget_name = format!("trigger-status-{position}");
        seed_budget(&router, &account_id, &budget_name).await;
        let (status, created) = call(
            &router,
            "CreateBudgetAction",
            &create_body(
                &account_id,
                &budget_name,
                notification_type,
                *threshold_value,
                threshold_type,
                approval_model,
            ),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateBudgetAction to answer 200, got {status} {created}"
        );
        let action_id = created["ActionId"].as_str().unwrap_or_default().to_string();

        let reported = status_of(&router, &account_id, &budget_name, &action_id, case).await;
        assert_eq!(
            &reported, expected,
            "case {case:?}: expected the created action to be {expected}, got {reported:?}"
        );
        // A read changes nothing: the status holds across repeated reads, since no clock
        // owes this action a move.
        let again = status_of(&router, &account_id, &budget_name, &action_id, case).await;
        assert_eq!(
            again, reported,
            "case {case:?}: expected the status to hold across reads, got {again:?} after \
             {reported:?}"
        );
    }
}

/// RULE, the bounds themselves: a threshold one past either published bound of
/// `ActionThresholdValue` is refused at request validation, so no status is stamped and no
/// action is stored.
#[tokio::test]
async fn a_threshold_past_either_published_bound_creates_nothing() {
    let router = router();
    let account_id = caller_account();
    let budget_name = "trigger-status-bounds";
    seed_budget(&router, &account_id, budget_name).await;

    for (case, threshold_value, constraint) in [
        (
            "one below the published minimum",
            THRESHOLD_MIN - 1.0,
            "Member must have value greater than or equal to 0",
        ),
        (
            "one above the published maximum",
            THRESHOLD_MAX + 1.0,
            "Member must have value less than or equal to 15000000000000",
        ),
    ] {
        let (status, body) = call(
            &router,
            "CreateBudgetAction",
            &create_body(
                &account_id,
                budget_name,
                "ACTUAL",
                threshold_value,
                "PERCENTAGE",
                "MANUAL",
            ),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the refusal at 400, got {status} {body}"
        );
        assert!(
            is_error(&body, "ValidationException"),
            "case {case:?}: expected ValidationException, got {body}"
        );
        let text = message(&body);
        assert!(
            text.contains(constraint),
            "case {case:?}: expected a message naming {constraint:?}, got {text:?}"
        );
    }

    let (status, body) = call(
        &router,
        "DescribeBudgetActionsForBudget",
        &json!({"AccountId": &account_id, "BudgetName": budget_name}),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the action listing to answer 200, got {status} {body}"
    );
    assert_eq!(
        body["Actions"].as_array().map(Vec::len),
        Some(0),
        "expected the refused creates to store no action, got {body}"
    );
}
