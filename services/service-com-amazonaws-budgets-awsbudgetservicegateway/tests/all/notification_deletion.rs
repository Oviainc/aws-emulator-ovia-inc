// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for `DeleteNotification`, driven over the wire through the same
//! `MultiServiceRouter` the production router serves from (the harness is the
//! lifecycle suite's, so both suites drive the service identically).
//!
//! The RULE every test here asserts: a notification is named by VALUE — a caller hands
//! back the `Notification` shape, and the members that decide which notification that
//! is are the notification type, the comparison operator, the threshold and the
//! threshold type (absent naming the same threshold type as `PERCENTAGE`, since
//! "Thresholds are always a percentage") — never the service-owned
//! `NotificationState`; the named notification is removed from the budget that owns it
//! together with the subscribers held with it ("Deleting a notification also deletes
//! the subscribers that are associated with the notification"), every other
//! notification keeps its place, and a notification or budget that does not exist is
//! refused with the modelled `NotFoundException`.
//!
//! Each table spans that class rather than one recorded request: its rows differ along
//! every identity member, the alarm state, the position of the notification inside its
//! budget, and the subscriber count.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// The enum catalogs are not re-typed here: these are the model-derived constants the
/// scaffold generated into `src/ops/types/shared_types.rs`, so a table built from them
/// cannot drift from the model.
use service_com_amazonaws_budgets_awsbudgetservicegateway::ops::types::{
    COMPARISON_OPERATOR_VALUES, NOTIFICATION_STATE_VALUES, NOTIFICATION_TYPE_VALUES,
    THRESHOLD_TYPE_VALUES,
};
use service_com_amazonaws_budgets_awsbudgetservicegateway::state::ComAmazonawsBudgetsAwsbudgetservicegatewayState;

use super::budget_lifecycle::{account, call, is_error, router, router_with_engine, same};

/// The notification set the tables below delete from.
///
/// Its rows differ along what a notification's identity is made of: the notification
/// type and the comparison operator (the full cross product of the model's two
/// catalogs), the threshold (each published `@range` bound of
/// `Notification$Threshold` — 0 and 15000000000000 — and a fraction between them) and
/// the threshold type (each `ThresholdType` the model lists, plus absent). The
/// service-owned alarm state varies across the rows too, so a table that matches on
/// identity cannot be matching on the alarm state by accident.
///
/// Six rows, which `NotificationWithSubscribersList`'s `@length(max: 10)` lets a single
/// `CreateBudget` establish.
fn notification_catalog() -> Vec<Value> {
    let thresholds = [0.0_f64, 42.5, 15_000_000_000_000.0];
    let threshold_types: Vec<Option<&str>> = THRESHOLD_TYPE_VALUES
        .iter()
        .map(|value| Some(*value))
        .chain(std::iter::once(None))
        .collect();
    let mut catalog = Vec::new();
    for (type_index, notification_type) in NOTIFICATION_TYPE_VALUES.iter().enumerate() {
        for (operator_index, comparison_operator) in COMPARISON_OPERATOR_VALUES.iter().enumerate() {
            let position = type_index * COMPARISON_OPERATOR_VALUES.len() + operator_index;
            let mut notification = json!({
                "NotificationType": notification_type,
                "ComparisonOperator": comparison_operator,
                "Threshold": thresholds[position % thresholds.len()],
            });
            if let Some(threshold_type) = threshold_types[position % threshold_types.len()] {
                notification["ThresholdType"] = json!(threshold_type);
            }
            if position.is_multiple_of(2) {
                notification["NotificationState"] = json!("ALARM");
            }
            catalog.push(notification);
        }
    }
    catalog
}

/// The members a delete names a notification BY, with an absent `ThresholdType`
/// resolved to the percentage it means — what two notifications must agree on to be
/// the same one.
fn identity(notification: &Value) -> Value {
    json!({
        "NotificationType": notification["NotificationType"].clone(),
        "ComparisonOperator": notification["ComparisonOperator"].clone(),
        "Threshold": notification["Threshold"].clone(),
        "ThresholdType": notification
            .get("ThresholdType")
            .cloned()
            .unwrap_or_else(|| json!("PERCENTAGE")),
    })
}

/// The subscribers attached to the notification at `position`: one email subscriber,
/// and an SNS subscriber as well on every other row, so the rows differ in subscriber
/// count and type. Every address names its position, so the store can be checked for
/// exactly whose subscribers went away.
fn subscribers(position: usize) -> Vec<Value> {
    let mut list = vec![json!({
        "SubscriptionType": "EMAIL",
        "Address": format!("n{position}@example.com")
    })];
    if !position.is_multiple_of(2) {
        list.push(json!({
            "SubscriptionType": "SNS",
            "Address": format!("arn:aws:sns::000000000000:n{position}")
        }));
    }
    list
}

/// Every subscriber address of `position`'s notification, in order.
fn addresses(position: usize) -> Vec<String> {
    subscribers(position)
        .iter()
        .map(|subscriber| subscriber["Address"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Create `budget_name` holding `catalog`, each notification with [`subscribers`].
async fn create_with_notifications(
    router: &MultiServiceRouter,
    account_id: &str,
    budget_name: &str,
    catalog: &[Value],
) {
    let with_subscribers: Vec<Value> = catalog
        .iter()
        .enumerate()
        .map(|(position, notification)| {
            json!({"Notification": notification, "Subscribers": subscribers(position)})
        })
        .collect();
    let (status, body) = call(
        router,
        "CreateBudget",
        &json!({
            "AccountId": account_id,
            "Budget": {
                "BudgetName": budget_name,
                "TimeUnit": "MONTHLY",
                "BudgetType": "COST",
                "BudgetLimit": {"Amount": "100", "Unit": "USD"}
            },
            "NotificationsWithSubscribers": with_subscribers
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup {budget_name:?}: expected CreateBudget to answer 200, got {status} {body}"
    );
}

/// The notifications `DescribeNotificationsForBudget` reports for a budget, in order.
async fn reported_notifications(
    router: &MultiServiceRouter,
    account_id: &str,
    budget_name: &str,
) -> Vec<Value> {
    let (status, body) = call(
        router,
        "DescribeNotificationsForBudget",
        &json!({"AccountId": account_id, "BudgetName": budget_name}),
    )
    .await;
    assert_eq!(
        status, 200,
        "reading {budget_name:?} back: expected 200, got {status} {body}"
    );
    body["Notifications"].as_array().cloned().unwrap_or_default()
}

/// Every subscriber address the STORE still holds for a budget, in notification order.
/// Read from the state struct directly, so what the deletion did to the store is
/// asserted independently of any read operation's own projection.
fn stored_addresses(
    engine: &Arc<engine::InMemoryEngine>,
    account_id: &str,
    budget_name: &str,
) -> Vec<String> {
    let state = engine.state::<ComAmazonawsBudgetsAwsbudgetservicegatewayState>();
    let store = state.lock();
    store
        .budgets
        .get(&(account_id.to_string(), budget_name.to_string()))
        .expect("the budget under test is stored")
        .notifications
        .iter()
        .flat_map(|held| {
            held.subscribers
                .iter()
                .map(|subscriber| subscriber.address.clone())
        })
        .collect()
}

/// One `DeleteNotification` call.
async fn delete_notification(
    router: &MultiServiceRouter,
    account_id: &str,
    budget_name: &str,
    notification: &Value,
) -> (u16, Value) {
    call(
        router,
        "DeleteNotification",
        &json!({
            "AccountId": account_id,
            "BudgetName": budget_name,
            "Notification": notification
        }),
    )
    .await
}

/// RULE: the delete removes exactly the named notification and the subscribers held
/// with it; every other notification of that budget keeps its value and its place, and
/// a second delete of the same notification is refused with `NotFoundException`.
///
/// The rows are each position of the catalog in turn, so the rule is asserted for the
/// first, the middle and the last notification of a budget, over notifications that
/// differ in every identity member and in subscriber count.
#[tokio::test]
async fn deleting_a_notification_removes_only_it_and_its_subscribers() {
    let (router, engine) = router_with_engine();
    let account_id = account(818_181_818);
    let catalog = notification_catalog();

    for target in 0..catalog.len() {
        let budget_name = format!("delete-{target}");
        create_with_notifications(&router, &account_id, &budget_name, &catalog).await;
        let case = format!("target {target} of {}: {}", catalog.len(), catalog[target]);

        let (status, body) =
            delete_notification(&router, &account_id, &budget_name, &catalog[target]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DeleteNotification to answer 200, got {status} {body}"
        );

        let remaining = reported_notifications(&router, &account_id, &budget_name).await;
        let expected: Vec<Value> = catalog
            .iter()
            .enumerate()
            .filter(|(position, _)| *position != target)
            .map(|(_, notification)| identity(notification))
            .collect();
        assert_eq!(
            remaining.len(),
            expected.len(),
            "case {case:?}: expected {} notifications left, got {remaining:?}",
            expected.len()
        );
        for (position, want) in expected.iter().enumerate() {
            let got = identity(&remaining[position]);
            assert!(
                same(want, &got),
                "case {case:?}: expected notification {position} to be {want}, got {got}"
            );
        }

        let expected_addresses: Vec<String> = (0..catalog.len())
            .filter(|position| *position != target)
            .flat_map(addresses)
            .collect();
        let held = stored_addresses(&engine, &account_id, &budget_name);
        assert_eq!(
            held, expected_addresses,
            "case {case:?}: expected the deleted notification's subscribers to go with \
             it and no others, got {held:?}"
        );

        // Nothing is left to delete a second time.
        let (status, body) =
            delete_notification(&router, &account_id, &budget_name, &catalog[target]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a second delete to be refused, got {status} {body}"
        );
        assert!(
            is_error(&body, "NotFoundException"),
            "case {case:?}: expected NotFoundException on a second delete, got {body}"
        );
    }
}

/// RULE, the half about the alarm state: `NotificationState` "specifies whether this
/// notification is in alarm", a state the service moves on its own, so it is NOT part
/// of a notification's identity — whichever state the caller sends (or none), the
/// notification whose identity members match is the one deleted.
///
/// The rows are every catalog notification crossed with every `NotificationState` the
/// model lists plus absent, including the state the stored notification is NOT in.
#[tokio::test]
async fn the_alarm_state_is_not_part_of_a_notifications_identity() {
    let router = router();
    let account_id = account(828_282_828);
    let catalog = notification_catalog();
    let states: Vec<Option<&str>> = NOTIFICATION_STATE_VALUES
        .iter()
        .map(|value| Some(*value))
        .chain(std::iter::once(None))
        .collect();

    for (position, stored) in catalog.iter().enumerate() {
        for sent_state in &states {
            let budget_name = format!("alarm-{position}-{}", sent_state.unwrap_or("absent"));
            create_with_notifications(
                &router,
                &account_id,
                &budget_name,
                std::slice::from_ref(stored),
            )
            .await;
            let case = format!(
                "stored {stored} deleted while naming NotificationState {:?}",
                sent_state.unwrap_or("absent")
            );

            let mut named = stored.clone();
            match sent_state {
                Some(state) => named["NotificationState"] = json!(state),
                None => {
                    named.as_object_mut().expect("a notification object").remove("NotificationState");
                }
            }

            let (status, body) =
                delete_notification(&router, &account_id, &budget_name, &named).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the delete to answer 200, got {status} {body}"
            );
            let remaining = reported_notifications(&router, &account_id, &budget_name).await;
            assert!(
                remaining.is_empty(),
                "case {case:?}: expected the notification to be gone, got {remaining:?}"
            );
        }
    }
}

/// RULE, the half about the threshold type: a threshold is a percentage unless the
/// caller says otherwise ("Thresholds are always a percentage"), so an absent
/// `ThresholdType` and an explicit `PERCENTAGE` name the same notification — while
/// `ABSOLUTE_VALUE` names a different one.
///
/// The rows cross the threshold type the notification was CREATED with (absent,
/// `PERCENTAGE`, `ABSOLUTE_VALUE`) with the one the delete NAMES, and state for each
/// pair whether they are the same notification.
#[tokio::test]
async fn an_absent_threshold_type_means_percentage() {
    let router = router();
    let account_id = account(838_383_838);
    let forms: Vec<Option<&str>> = vec![None, Some("PERCENTAGE"), Some("ABSOLUTE_VALUE")];

    for created in &forms {
        for named in &forms {
            let label = |form: &Option<&str>| form.unwrap_or("absent").to_string();
            let budget_name = format!("threshold-{}-{}", label(created), label(named));
            let mut stored = json!({
                "NotificationType": "ACTUAL",
                "ComparisonOperator": "GREATER_THAN",
                "Threshold": 80.0
            });
            if let Some(form) = created {
                stored["ThresholdType"] = json!(form);
            }
            create_with_notifications(
                &router,
                &account_id,
                &budget_name,
                std::slice::from_ref(&stored),
            )
            .await;

            let mut sent = stored.clone();
            sent.as_object_mut().expect("a notification object").remove("ThresholdType");
            if let Some(form) = named {
                sent["ThresholdType"] = json!(form);
            }

            // Absent and PERCENTAGE are the same threshold type; ABSOLUTE_VALUE is not.
            let percentage = |form: &Option<&str>| form.unwrap_or("PERCENTAGE") == "PERCENTAGE";
            let is_same = percentage(created) == percentage(named);
            let case = format!(
                "created with ThresholdType {:?}, deleted naming {:?}",
                label(created),
                label(named)
            );

            let (status, body) = delete_notification(&router, &account_id, &budget_name, &sent).await;
            if is_same {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the same notification to be deleted, \
                     got {status} {body}"
                );
            } else {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected a different notification to be refused, \
                     got {status} {body}"
                );
                assert!(
                    is_error(&body, "NotFoundException"),
                    "case {case:?}: expected NotFoundException, got {body}"
                );
            }
            let remaining = reported_notifications(&router, &account_id, &budget_name).await;
            assert_eq!(
                remaining.len(),
                usize::from(!is_same),
                "case {case:?}: expected {} notification(s) to remain, got {remaining:?}",
                usize::from(!is_same)
            );
        }
    }
}

/// RULE, the refusal side: a `Notification` that differs from the stored one in ANY
/// identity member names no notification of that budget, so it is refused with
/// `NotFoundException` and the budget keeps everything it held.
///
/// One row per identity member, each varying that member alone.
#[tokio::test]
async fn a_notification_that_differs_in_an_identity_member_is_not_found() {
    let router = router();
    let account_id = account(848_484_848);
    let stored = json!({
        "NotificationType": "ACTUAL",
        "ComparisonOperator": "GREATER_THAN",
        "Threshold": 80.0,
        "ThresholdType": "PERCENTAGE"
    });
    let budget_name = "near-miss";
    create_with_notifications(
        &router,
        &account_id,
        budget_name,
        std::slice::from_ref(&stored),
    )
    .await;

    let mut rows: Vec<(&str, Value)> = Vec::new();
    for (member, value) in [
        ("NotificationType", json!("FORECASTED")),
        ("ComparisonOperator", json!("LESS_THAN")),
        ("ComparisonOperator", json!("EQUAL_TO")),
        ("Threshold", json!(80.5)),
        ("Threshold", json!(0.0)),
        ("ThresholdType", json!("ABSOLUTE_VALUE")),
    ] {
        let mut near_miss = stored.clone();
        near_miss[member] = value;
        rows.push((member, near_miss));
    }

    for (member, near_miss) in rows {
        let case = format!("{member} differs: {near_miss}");
        let (status, body) =
            delete_notification(&router, &account_id, budget_name, &near_miss).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the delete to be refused, got {status} {body}"
        );
        assert!(
            is_error(&body, "NotFoundException"),
            "case {case:?}: expected NotFoundException, got {body}"
        );
        let remaining = reported_notifications(&router, &account_id, budget_name).await;
        assert_eq!(
            remaining.len(),
            1,
            "case {case:?}: expected the refused delete to remove nothing, got {remaining:?}"
        );
    }

    // And the notification that DOES match is still deletable afterwards.
    let (status, body) = delete_notification(&router, &account_id, budget_name, &stored).await;
    assert_eq!(
        status, 200,
        "expected the matching notification to delete after the near misses, \
         got {status} {body}"
    );
}

/// RULE, the budget side: a notification is reached only through the budget that owns
/// it, which is identified by `(AccountId, BudgetName)` — so a budget this account does
/// not hold is `NotFoundException`, and another account's budget of the same name keeps
/// its notifications. A `BudgetName` outside its modelled `@pattern` is the
/// request-validation refusal it is, not a not-found.
#[tokio::test]
async fn a_notification_is_reached_only_through_the_budget_that_owns_it() {
    let router = router();
    let owner = account(858_585_858);
    let other = account(868_686_868);
    let notification = json!({
        "NotificationType": "ACTUAL",
        "ComparisonOperator": "GREATER_THAN",
        "Threshold": 80.0
    });
    create_with_notifications(
        &router,
        &owner,
        "shared-name",
        std::slice::from_ref(&notification),
    )
    .await;

    for (case, account_id, budget_name) in [
        ("a budget the account does not hold", &owner, "absent"),
        ("another account's budget of the same name", &other, "shared-name"),
    ] {
        let (status, body) =
            delete_notification(&router, account_id, budget_name, &notification).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the delete to be refused, got {status} {body}"
        );
        assert!(
            is_error(&body, "NotFoundException"),
            "case {case:?}: expected NotFoundException, got {body}"
        );
    }

    // The owner's notification is untouched by either refusal.
    let remaining = reported_notifications(&router, &owner, "shared-name").await;
    assert_eq!(
        remaining.len(),
        1,
        "expected the owner's notification to survive, got {remaining:?}"
    );

    // A name outside `BudgetName`'s pattern is a validation refusal, which the
    // framework's regex engine cannot compile, so this handler answers it.
    for (case, budget_name) in [
        ("a colon", "a:b"),
        ("a backslash", "a\\b"),
        ("the /action/ substring", "a/action/b"),
    ] {
        let (status, body) =
            delete_notification(&router, &owner, budget_name, &notification).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {body}"
        );
        assert!(
            is_error(&body, "ValidationException"),
            "case {case:?}: expected ValidationException, got {body}"
        );
        let text = body
            .get("message")
            .or_else(|| body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            text.contains("'budgetName'")
                && text.contains("Member must satisfy regular expression pattern"),
            "case {case:?}: expected the message to name the member and the constraint, \
             got {text:?}"
        );
    }
}
