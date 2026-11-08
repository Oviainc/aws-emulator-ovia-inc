// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for `DeleteSubscriber`, read back through
//! `DescribeSubscribersForNotification`, `DescribeNotificationsForBudget` and the store
//! itself, driven over the wire through the same `MultiServiceRouter` the production
//! router serves from (the harness is the lifecycle suite's, so every suite in this
//! target drives the service identically).
//!
//! The RULE every test here asserts: a delete removes ONE subscriber — the one
//! `Subscriber` names by value, its subscription type and its address being its whole
//! identity — from the notification `Notification` names by value on the budget
//! `(AccountId, BudgetName)` names; the notification's remaining subscribers keep their
//! value and their position, no other notification of the budget is touched, and
//! "deleting the last subscriber to a notification also deletes the notification", so a
//! call that empties a set removes the notification with it. A budget, a notification or
//! a subscriber that is not held is refused with `NotFoundException` and nothing is
//! written.
//!
//! Each table spans that class rather than one request: the rows differ along every
//! member a notification's identity is made of (each published bound of the threshold's
//! range included), along the alarm state, along the position of the subscriber inside
//! its notification and of the notification inside its budget, and along the size and
//! the type mix of the subscriber set.
//!
//! The account gate every operation of this service shares — an `AccountId` that is not
//! the caller's own is refused with the modelled 403 — is asserted for this operation
//! among all of them in `account_scoping.rs`, so every request here names the caller's
//! own account.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The enum catalogs are not re-typed here: these are the model-derived constants the
/// scaffold generated into `src/ops/types/shared_types.rs`, so a table built from them
/// cannot drift from the model.
use service_com_amazonaws_budgets_awsbudgetservicegateway::ops::types::{
    COMPARISON_OPERATOR_VALUES, NOTIFICATION_STATE_VALUES, NOTIFICATION_TYPE_VALUES,
    SUBSCRIPTION_TYPE_VALUES, THRESHOLD_TYPE_VALUES,
};
use service_com_amazonaws_budgets_awsbudgetservicegateway::state::{
    ComAmazonawsBudgetsAwsbudgetservicegatewayState, MAX_NOTIFICATIONS_PER_BUDGET,
};

use super::budget_lifecycle::{caller_account, call, is_error, router_with_engine, same};
use super::client_error_envelopes::message;

/// The published `@range` of `Notification$Threshold`, from
/// `smithy_query(query="shape", shape="Notification")`: `range {min: 0, max:
/// 15000000000000}`.
const THRESHOLD_MIN: f64 = 0.0;
const THRESHOLD_MAX: f64 = 15_000_000_000_000.0;

/// An email subscriber of the notification at `notification`, at index `index` of its
/// set. The address names both, so a read can be checked for exactly which subscriber is
/// still attached and where.
fn email(notification: usize, index: usize) -> Value {
    json!({
        "SubscriptionType": "EMAIL",
        "Address": format!("n{notification}s{index}@example.com")
    })
}

/// The SNS subscriber of the notification at `notification` — "one SNS subscriber"
/// (`Notification`), so a notification's set holds at most this one.
fn sns(notification: usize) -> Value {
    json!({
        "SubscriptionType": "SNS",
        "Address": format!("arn:aws:sns:us-east-1:{}:n{notification}", caller_account())
    })
}

/// The notifications the tables delete subscribers from.
///
/// The rows differ along what a notification's identity is made of: the notification
/// type and the comparison operator (the full cross product of the model's two
/// catalogs), the threshold (each published `@range` bound and a fraction between them)
/// and the threshold type (each `ThresholdType` the model lists, plus absent). The
/// service-owned alarm state varies across the rows too, so a table that matches on
/// identity cannot be matching on the alarm state by accident.
///
/// Every row has a distinct `(NotificationType, ComparisonOperator)` pair, so no two
/// rows are the same notification.
fn notification_catalog() -> Vec<Value> {
    let thresholds = [THRESHOLD_MIN, 42.5, THRESHOLD_MAX];
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

/// The members a request names a notification BY, with an absent `ThresholdType`
/// resolved to the percentage it means — what two notifications must agree on to be the
/// same one.
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

/// The subscribers the notification at `position` is created with.
///
/// The rows differ in SIZE and in type mix — one email, two emails, and a set whose SNS
/// subscriber sits between two emails — so the rule is asserted for a set of one (whose
/// delete takes the notification with it), and for a delete at the front, the middle and
/// the back of a larger set, including one holding its single permitted SNS subscriber.
fn subscriber_set(position: usize) -> Vec<Value> {
    match position % 3 {
        0 => vec![email(position, 0)],
        1 => vec![email(position, 0), email(position, 1)],
        _ => vec![email(position, 0), sns(position), email(position, 1)],
    }
}

/// Create `budget_name` holding `catalog`, each notification with its
/// [`subscriber_set`], in catalog order, in ONE `CreateBudget`.
async fn seed(router: &MultiServiceRouter, account_id: &str, budget_name: &str, catalog: &[Value]) {
    assert!(
        catalog.len() <= MAX_NOTIFICATIONS_PER_BUDGET,
        "setup {budget_name:?}: a budget holds at most {MAX_NOTIFICATIONS_PER_BUDGET} \
         notifications, so a catalog of {} cannot be attached to one budget",
        catalog.len()
    );
    let with_subscribers: Vec<Value> = catalog
        .iter()
        .enumerate()
        .map(|(position, notification)| {
            json!({"Notification": notification, "Subscribers": subscriber_set(position)})
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

/// One `DeleteSubscriber` call.
async fn delete_subscriber(
    router: &MultiServiceRouter,
    account_id: &str,
    budget_name: &str,
    notification: &Value,
    subscriber: &Value,
) -> (u16, Value) {
    call(
        router,
        "DeleteSubscriber",
        &json!({
            "AccountId": account_id,
            "BudgetName": budget_name,
            "Notification": notification,
            "Subscriber": subscriber
        }),
    )
    .await
}

/// The subscribers `DescribeSubscribersForNotification` reports for one notification, in
/// order, with the status it answered.
async fn read_subscribers(
    router: &MultiServiceRouter,
    account_id: &str,
    budget_name: &str,
    notification: &Value,
) -> (u16, Value) {
    call(
        router,
        "DescribeSubscribersForNotification",
        &json!({
            "AccountId": account_id,
            "BudgetName": budget_name,
            "Notification": notification
        }),
    )
    .await
}

/// The subscribers `DescribeSubscribersForNotification` reports for a notification that
/// is expected to still be there.
async fn reported_subscribers(
    router: &MultiServiceRouter,
    account_id: &str,
    budget_name: &str,
    notification: &Value,
) -> Vec<Value> {
    let (status, body) = read_subscribers(router, account_id, budget_name, notification).await;
    assert_eq!(
        status, 200,
        "reading the subscribers of {budget_name:?} back: expected 200, got {status} {body}"
    );
    body["Subscribers"].as_array().cloned().unwrap_or_default()
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

/// Every subscriber the STORE holds for a budget, as `(SubscriptionType, Address)` per
/// notification in notification order — read from the state struct directly, so what a
/// refused delete did to the store is asserted independently of any read's projection.
fn stored_subscribers(
    engine: &Arc<engine::InMemoryEngine>,
    account_id: &str,
    budget_name: &str,
) -> Vec<Vec<(String, String)>> {
    let state = engine.state::<ComAmazonawsBudgetsAwsbudgetservicegatewayState>();
    let store = state.lock();
    store
        .budgets
        .get(&(account_id.to_string(), budget_name.to_string()))
        .map(|stored| {
            stored
                .notifications
                .iter()
                .map(|held| {
                    held.subscribers
                        .iter()
                        .map(|subscriber| {
                            (
                                subscriber.subscription_type.clone(),
                                subscriber.address.clone(),
                            )
                        })
                        .collect()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Assert the notification reports exactly `expected`, in order, comparing numbers by
/// value.
fn assert_subscribers(case: &str, expected: &[Value], got: &[Value]) {
    assert_eq!(
        got.len(),
        expected.len(),
        "case {case:?}: expected {} subscriber(s), got {got:?}",
        expected.len()
    );
    for (position, want) in expected.iter().enumerate() {
        assert!(
            same(want, &got[position]),
            "case {case:?}: expected subscriber {position} to be {want}, got {}",
            got[position]
        );
    }
}

/// The subscription types these tables span are the model's own catalog, so a catalog
/// that grew a third type fails the suite instead of going silently untested.
fn assert_catalog_spanned() {
    assert_eq!(
        SUBSCRIPTION_TYPE_VALUES,
        &["EMAIL", "SNS"],
        "the subscription-type catalog this suite spans changed: {SUBSCRIPTION_TYPE_VALUES:?}"
    );
}

/// RULE: the delete removes exactly the named subscriber of exactly the named
/// notification — the rest of that set keeps its value and its order, every other
/// notification of the budget keeps its own subscribers, and the subscriber that was
/// removed can no longer be named.
///
/// The rows are every position of every notification of the catalog, so the rule is
/// asserted at the front, the middle and the back of sets of one, two and three
/// subscribers, over notifications that differ in every identity member and in alarm
/// state, for an email and for an SNS subscriber.
#[tokio::test]
async fn a_delete_removes_only_the_named_subscriber() {
    assert_catalog_spanned();
    let account_id = caller_account();
    let catalog = notification_catalog();

    for target in 0..catalog.len() {
        let held = subscriber_set(target);
        for index in 0..held.len() {
            let (router, engine) = router_with_engine();
            let budget_name = format!("delete-n{target}-p{index}");
            seed(&router, &account_id, &budget_name, &catalog).await;
            let case = format!(
                "notification {target} of {}, subscriber {index} of {}: {}",
                catalog.len(),
                held.len(),
                held[index]
            );

            let (status, body) = delete_subscriber(
                &router,
                &account_id,
                &budget_name,
                &catalog[target],
                &held[index],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected DeleteSubscriber to answer 200, got {status} {body}"
            );

            let expected: Vec<Value> = held
                .iter()
                .enumerate()
                .filter(|(position, _)| *position != index)
                .map(|(_, subscriber)| subscriber.clone())
                .collect();

            // Whatever the set had left, that is what the store holds for every
            // notification of the budget — the emptied notification being gone entirely.
            let expected_store: Vec<Vec<(String, String)>> = (0..catalog.len())
                .map(|position| {
                    let set = if position == target {
                        expected.clone()
                    } else {
                        subscriber_set(position)
                    };
                    set.iter()
                        .map(|subscriber| {
                            (
                                subscriber["SubscriptionType"].as_str().unwrap_or_default().to_string(),
                                subscriber["Address"].as_str().unwrap_or_default().to_string(),
                            )
                        })
                        .collect()
                })
                .filter(|set: &Vec<(String, String)>| !set.is_empty())
                .collect();
            let got_store = stored_subscribers(&engine, &account_id, &budget_name);
            assert_eq!(
                got_store, expected_store,
                "case {case:?}: expected the store to hold {expected_store:?}, got {got_store:?}"
            );

            if expected.is_empty() {
                // The last subscriber took its notification with it, so the read reports
                // the empty subscriber set that notification now has: `Subscribers` is
                // `@length(min: 1)`, so the empty set is the member left out.
                let (status, body) =
                    read_subscribers(&router, &account_id, &budget_name, &catalog[target]).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the emptied notification to read as an empty \
                     subscriber set, got {status} {body}"
                );
                assert!(
                    body.get("Subscribers").is_none_or(Value::is_null),
                    "case {case:?}: expected no Subscribers member for an emptied \
                     notification, got {body}"
                );
            } else {
                let remaining =
                    reported_subscribers(&router, &account_id, &budget_name, &catalog[target]).await;
                assert_subscribers(&case, &expected, &remaining);
            }

            // No other notification of the budget lost a subscriber.
            for (position, notification) in catalog.iter().enumerate() {
                if position == target {
                    continue;
                }
                let others =
                    reported_subscribers(&router, &account_id, &budget_name, notification).await;
                assert_subscribers(
                    &format!("{case}, bystander notification {position}"),
                    &subscriber_set(position),
                    &others,
                );
            }

            // Nothing is left to delete a second time.
            let (status, body) = delete_subscriber(
                &router,
                &account_id,
                &budget_name,
                &catalog[target],
                &held[index],
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected a second delete of the same subscriber to be \
                 refused, got {status} {body}"
            );
            assert!(
                is_error(&body, "NotFoundException"),
                "case {case:?}: expected NotFoundException on the second delete, got {body}"
            );
        }
    }
}

/// RULE, the cascade half: "deleting the last subscriber to a notification also deletes
/// the notification" — so draining a notification one subscriber at a time removes it
/// from the budget exactly when its set empties, and never before.
///
/// The rows are sets of one, two and three subscribers, drained front-to-back and
/// back-to-front, on notifications that differ in every identity member, so the rule is
/// asserted over every set size and both drain orders.
#[tokio::test]
async fn emptying_a_notifications_subscribers_deletes_the_notification() {
    assert_catalog_spanned();
    let account_id = caller_account();
    let catalog = notification_catalog();

    for reverse in [false, true] {
        for target in 0..catalog.len() {
            let (router, engine) = router_with_engine();
            let budget_name = format!("drain-n{target}-{}", if reverse { "back" } else { "front" });
            seed(&router, &account_id, &budget_name, &catalog).await;
            let mut held = subscriber_set(target);
            if reverse {
                held.reverse();
            }
            let size = held.len();

            for (removed, subscriber) in held.iter().enumerate() {
                let case = format!(
                    "notification {target}, draining {} of {size} ({}): {subscriber}",
                    removed + 1,
                    if reverse { "back first" } else { "front first" }
                );
                let (status, body) = delete_subscriber(
                    &router,
                    &account_id,
                    &budget_name,
                    &catalog[target],
                    subscriber,
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the delete to answer 200, got {status} {body}"
                );

                let last = removed + 1 == size;
                let reported = reported_notifications(&router, &account_id, &budget_name).await;
                let still_there = reported
                    .iter()
                    .any(|notification| same(&identity(notification), &identity(&catalog[target])));
                assert_eq!(
                    still_there, !last,
                    "case {case:?}: expected the notification to {} be reported, got {reported:?}",
                    if last { "no longer" } else { "still" }
                );
                assert_eq!(
                    reported.len(),
                    catalog.len() - usize::from(last),
                    "case {case:?}: expected {} notification(s) on the budget, got {reported:?}",
                    catalog.len() - usize::from(last)
                );
            }

            // The store itself holds one notification fewer, and every bystander keeps
            // the subscribers it was created with.
            let got_store = stored_subscribers(&engine, &account_id, &budget_name);
            let expected_store: Vec<Vec<(String, String)>> = (0..catalog.len())
                .filter(|position| *position != target)
                .map(|position| {
                    subscriber_set(position)
                        .iter()
                        .map(|subscriber| {
                            (
                                subscriber["SubscriptionType"].as_str().unwrap_or_default().to_string(),
                                subscriber["Address"].as_str().unwrap_or_default().to_string(),
                            )
                        })
                        .collect()
                })
                .collect();
            assert_eq!(
                got_store, expected_store,
                "notification {target} drained {}: expected the store to hold \
                 {expected_store:?}, got {got_store:?}",
                if reverse { "back first" } else { "front first" }
            );
        }
    }
}

/// RULE, the alarm-state half: `NotificationState` "specifies whether this notification
/// is in alarm", a state the service moves on its own, so it is NOT part of a
/// notification's identity — whichever state the caller sends (or none), the
/// notification whose identity members match is the one the subscriber is removed from.
///
/// The rows cross every catalog notification with every `NotificationState` the model
/// lists plus absent, including the state the stored notification is NOT in.
#[tokio::test]
async fn the_alarm_state_is_not_part_of_the_notification_a_delete_names() {
    let account_id = caller_account();
    let catalog = notification_catalog();
    let states: Vec<Option<&str>> = NOTIFICATION_STATE_VALUES
        .iter()
        .map(|value| Some(*value))
        .chain(std::iter::once(None))
        .collect();

    for (position, stored) in catalog.iter().enumerate() {
        for sent_state in &states {
            let (router, _engine) = router_with_engine();
            let budget_name = format!("alarm-{position}-{}", sent_state.unwrap_or("absent"));
            seed(
                &router,
                &account_id,
                &budget_name,
                std::slice::from_ref(stored),
            )
            .await;
            let case = format!(
                "stored {stored} named with NotificationState {:?}",
                sent_state.unwrap_or("absent")
            );

            let mut named = stored.clone();
            match sent_state {
                Some(state) => named["NotificationState"] = json!(state),
                None => {
                    named
                        .as_object_mut()
                        .expect("a notification object")
                        .remove("NotificationState");
                }
            }

            // The set seeded at position 0 is a single email subscriber, so this delete
            // also exercises the cascade.
            let (status, body) = delete_subscriber(
                &router,
                &account_id,
                &budget_name,
                &named,
                &subscriber_set(0)[0],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the delete to answer 200, got {status} {body}"
            );
            let remaining = reported_notifications(&router, &account_id, &budget_name).await;
            assert!(
                remaining.is_empty(),
                "case {case:?}: expected the emptied notification to be gone, got {remaining:?}"
            );
        }
    }
}

/// One row of the not-found table.
struct Missing {
    case: String,
    budget_name: String,
    notification: Value,
    subscriber: Value,
}

/// RULE, the not-found side: a delete reaches a subscriber only through a notification of
/// a budget the account holds, and names all three by value — so a budget that is not
/// held, a notification that budget does not alert on (differing in ANY member of a
/// notification's identity) and a subscriber that notification does not hold (differing
/// in either member of a subscriber's identity) are each refused with
/// `NotFoundException`, and nothing is written.
#[tokio::test]
async fn a_delete_naming_something_not_held_is_not_found() {
    assert_catalog_spanned();
    let (router, engine) = router_with_engine();
    let account_id = caller_account();
    let catalog = notification_catalog();
    let budget_name = "held";
    seed(&router, &account_id, budget_name, &catalog).await;
    let before = stored_subscribers(&engine, &account_id, budget_name);

    // The notification whose subscribers the near misses below name: the catalog row
    // with the largest set, so a near miss can differ in either subscriber member.
    let anchor = 2;
    let anchor_notification = catalog[anchor].clone();
    let anchor_subscriber = subscriber_set(anchor)[0].clone();

    let mut rows: Vec<Missing> = Vec::new();

    // A budget this account holds no budget under, and a name that is close to one.
    for (case, name) in [
        ("a name the account holds no budget under", "absent"),
        ("the stored name with a suffix", "held-2"),
        ("the stored name in another case", "HELD"),
    ] {
        rows.push(Missing {
            case: format!("budget: {case}"),
            budget_name: name.to_string(),
            notification: anchor_notification.clone(),
            subscriber: anchor_subscriber.clone(),
        });
    }

    // A notification differing in exactly one identity member, one row per member, each
    // value drawn from the model's own catalog or the published bounds of the threshold's
    // range.
    for (member, value) in [
        ("NotificationType", json!("FORECASTED")),
        ("ComparisonOperator", json!("LESS_THAN")),
        ("ComparisonOperator", json!("EQUAL_TO")),
        ("Threshold", json!(THRESHOLD_MIN)),
        ("Threshold", json!(THRESHOLD_MAX)),
        ("Threshold", json!(1.5)),
        ("ThresholdType", json!("ABSOLUTE_VALUE")),
        ("ThresholdType", json!("PERCENTAGE")),
    ] {
        let mut near_miss = identity(&anchor_notification);
        if near_miss[member] == value {
            // Already the stored value, so this row would name the SAME notification.
            continue;
        }
        near_miss[member] = value.clone();
        rows.push(Missing {
            case: format!("notification: {member} is {value}"),
            budget_name: budget_name.to_string(),
            notification: near_miss,
            subscriber: anchor_subscriber.clone(),
        });
    }

    // A subscriber differing in exactly one identity member: the same address over the
    // other subscription type is a DIFFERENT subscriber, and so is a different address
    // over the same type — including one held by another notification of the budget.
    for (case, subscriber) in [
        (
            "the stored address over the other subscription type",
            json!({
                "SubscriptionType": "SNS",
                "Address": anchor_subscriber["Address"].clone()
            }),
        ),
        (
            "an address no notification holds",
            json!({"SubscriptionType": "EMAIL", "Address": "nobody@example.com"}),
        ),
        (
            "an address another notification of the budget holds",
            subscriber_set(1)[0].clone(),
        ),
    ] {
        rows.push(Missing {
            case: format!("subscriber: {case}"),
            budget_name: budget_name.to_string(),
            notification: anchor_notification.clone(),
            subscriber,
        });
    }

    for row in &rows {
        let Missing {
            case,
            budget_name,
            notification,
            subscriber,
        } = row;
        let (status, body) = delete_subscriber(
            &router,
            &account_id,
            budget_name,
            notification,
            subscriber,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the delete to be refused, got {status} {body}"
        );
        assert!(
            is_error(&body, "NotFoundException"),
            "case {case:?}: expected NotFoundException, got {body}"
        );
        assert!(
            !message(&body).is_empty(),
            "case {case:?}: expected a message a caller can read, got {body}"
        );
        let after = stored_subscribers(&engine, &account_id, "held");
        assert_eq!(
            after, before,
            "case {case:?}: expected the refused delete to write nothing, got {after:?}"
        );
    }

    // And the subscriber that IS held is still deletable after every near miss.
    let (status, body) = delete_subscriber(
        &router,
        &account_id,
        budget_name,
        &anchor_notification,
        &anchor_subscriber,
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the held subscriber to delete after the near misses, got {status} {body}"
    );
}

/// RULE, the request-validation side: a `BudgetName` outside its modelled `@pattern` is
/// the request-validation refusal it is — HTTP 400 `ValidationException` naming the
/// member and the constraint — not a not-found, because the framework's regex engine
/// cannot compile that pattern and so this handler answers it.
#[tokio::test]
async fn a_budget_name_outside_its_pattern_is_a_validation_refusal() {
    let (router, _engine) = router_with_engine();
    let account_id = caller_account();
    let notification = json!({
        "NotificationType": "ACTUAL",
        "ComparisonOperator": "GREATER_THAN",
        "Threshold": 80.0
    });
    let subscriber = json!({"SubscriptionType": "EMAIL", "Address": "ops@example.com"});

    for (case, budget_name) in [
        ("a colon", "a:b"),
        ("a backslash", "a\\b"),
        ("the /action/ substring", "a/action/b"),
    ] {
        let (status, body) = delete_subscriber(
            &router,
            &account_id,
            budget_name,
            &notification,
            &subscriber,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {body}"
        );
        assert!(
            is_error(&body, "ValidationException"),
            "case {case:?}: expected ValidationException, got {body}"
        );
        let text = message(&body);
        assert!(
            text.contains("'budgetName'")
                && text.contains("Member must satisfy regular expression pattern"),
            "case {case:?}: expected the message to name the member and the constraint, \
             got {text:?}"
        );
    }
}
