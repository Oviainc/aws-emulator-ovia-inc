// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for `UpdateSubscriber`, read back through
//! `DescribeSubscribersForNotification` and through the store itself, driven over the
//! wire through the same `MultiServiceRouter` the production router serves from (the
//! harness is the lifecycle suite's, so every suite in this target drives the service
//! identically).
//!
//! The RULE every test here asserts: an update replaces ONE subscriber of the
//! notification `Notification` names on the budget `(AccountId, BudgetName)` names —
//! the one `OldSubscriber` names by value, its subscription type and its address being
//! its whole identity — with `NewSubscriber`, IN PLACE: the replacement takes the old
//! one's position, every other subscriber of that notification keeps its value and its
//! position, the set keeps its size, and no other notification of the budget is touched.
//! A budget, a notification or a subscriber that is not held is refused with
//! `NotFoundException`; a `NewSubscriber` naming a DIFFERENT subscriber the same
//! notification already holds is refused with `DuplicateRecordException`; and a
//! replacement that would leave the notification outside the documented per-type
//! subscriber split is refused with `InvalidParameterException`, the client error this
//! operation models in place of a creation-limit error it does not.
//!
//! Each table spans that class rather than one request: the rows differ along every
//! member a notification's identity is made of (each published bound of the threshold's
//! range included), along the alarm state, along the position of the subscriber inside
//! its notification and of the notification inside its budget, along the size and the
//! type mix of the subscriber set, and along whether the replacement keeps or changes
//! the subscription type.
//!
//! The account gate every operation of this service shares — an `AccountId` that is not
//! the caller's own is refused with the modelled 403 — is asserted for this operation
//! among all of them in `account_scoping.rs`, and the ADDRESS rule every write that
//! establishes a subscriber is held to in `subscriber_address_validation.rs`, so every
//! request here names the caller's own account and a well-formed address.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The enum catalogs are not re-typed here: these are the model-derived constants the
/// scaffold generated into `src/ops/types/shared_types.rs`, so a table built from them
/// cannot drift from the model.
use service_com_amazonaws_budgets_awsbudgetservicegateway::ops::types::{
    COMPARISON_OPERATOR_VALUES, NOTIFICATION_TYPE_VALUES, SUBSCRIPTION_TYPE_VALUES,
    THRESHOLD_TYPE_VALUES,
};
use service_com_amazonaws_budgets_awsbudgetservicegateway::state::{
    ComAmazonawsBudgetsAwsbudgetservicegatewayState, MAX_EMAIL_SUBSCRIBERS_PER_NOTIFICATION,
    MAX_NOTIFICATIONS_PER_BUDGET, MAX_SNS_SUBSCRIBERS_PER_NOTIFICATION,
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

/// The notifications the tables update subscribers on.
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

/// The subscribers the notification at `position` is created with.
///
/// The rows differ in SIZE and in type mix — one email, two emails, and a set whose SNS
/// subscriber sits between two emails — so the rule is asserted for a set of one, for a
/// replacement at the front, the middle and the back of a set, and for a set that
/// already holds its one permitted SNS subscriber.
fn subscriber_set(position: usize) -> Vec<Value> {
    match position % 3 {
        0 => vec![email(position, 0)],
        1 => vec![email(position, 0), email(position, 1)],
        _ => vec![email(position, 0), sns(position), email(position, 1)],
    }
}

/// The subscriber the one at `index` of `held` is updated TO: an address no row of any
/// set uses, so the replacement is a subscriber the notification does not already hold
/// however the rows line up.
///
/// Its TYPE varies with the set it lands in: a set that already holds its one permitted
/// SNS subscriber can only take another email where that SNS subscriber is not the one
/// being replaced, so the rows cover a replacement that KEEPS the subscription type and
/// one that CHANGES it without ever asking for a set the per-type split forbids.
fn replacement(notification: usize, index: usize, held: &[Value]) -> Value {
    let replaced_is_sns = held[index]["SubscriptionType"] == json!("SNS");
    let set_holds_sns = held
        .iter()
        .any(|subscriber| subscriber["SubscriptionType"] == json!("SNS"));
    let subscription_type = if replaced_is_sns || (!set_holds_sns && (notification + index).is_multiple_of(2))
    {
        "SNS"
    } else {
        "EMAIL"
    };
    let address = if subscription_type == "SNS" {
        format!(
            "arn:aws:sns:us-east-1:{}:new-n{notification}p{index}",
            caller_account()
        )
    } else {
        format!("new-n{notification}p{index}@example.com")
    };
    json!({"SubscriptionType": subscription_type, "Address": address})
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

/// One `UpdateSubscriber` call.
async fn update_subscriber(
    router: &MultiServiceRouter,
    account_id: &str,
    budget_name: &str,
    notification: &Value,
    old: &Value,
    new: &Value,
) -> (u16, Value) {
    call(
        router,
        "UpdateSubscriber",
        &json!({
            "AccountId": account_id,
            "BudgetName": budget_name,
            "Notification": notification,
            "OldSubscriber": old,
            "NewSubscriber": new
        }),
    )
    .await
}

/// The subscribers `DescribeSubscribersForNotification` reports for one notification, in
/// order.
async fn reported_subscribers(
    router: &MultiServiceRouter,
    account_id: &str,
    budget_name: &str,
    notification: &Value,
) -> Vec<Value> {
    let (status, body) = call(
        router,
        "DescribeSubscribersForNotification",
        &json!({
            "AccountId": account_id,
            "BudgetName": budget_name,
            "Notification": notification
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "reading the subscribers of {budget_name:?} back: expected 200, got {status} {body}"
    );
    body["Subscribers"].as_array().cloned().unwrap_or_default()
}

/// Every subscriber the STORE holds for a budget, as `(SubscriptionType, Address)` per
/// notification in notification order — read from the state struct directly, so what a
/// refused update did to the store is asserted independently of any read's projection.
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

/// RULE: the update replaces exactly the named subscriber of exactly the named
/// notification, in place — the replacement takes the old one's position, the rest of
/// the set keeps its value and its order, the set keeps its size, every other
/// notification of the budget keeps its own subscribers, and the subscriber that was
/// replaced can no longer be named.
///
/// The rows are every position of every notification of the catalog, so the rule is
/// asserted at the front, the middle and the back of sets of one, two and three
/// subscribers, over notifications that differ in every identity member and in alarm
/// state, for replacements that keep and that change the subscription type.
#[tokio::test]
async fn an_update_replaces_the_named_subscriber_in_place() {
    assert_catalog_spanned();
    let account_id = caller_account();
    let catalog = notification_catalog();

    for target in 0..catalog.len() {
        let held = subscriber_set(target);
        for index in 0..held.len() {
            let (router, engine) = router_with_engine();
            let budget_name = format!("update-n{target}-p{index}");
            seed(&router, &account_id, &budget_name, &catalog).await;
            let new = replacement(target, index, &held);
            let case = format!(
                "notification {target} of {}, subscriber {index} of {}: {} -> {new}",
                catalog.len(),
                held.len(),
                held[index]
            );

            let (status, body) = update_subscriber(
                &router,
                &account_id,
                &budget_name,
                &catalog[target],
                &held[index],
                &new,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected UpdateSubscriber to answer 200, got {status} {body}"
            );

            // The named notification holds the set it held, with that ONE position
            // replaced, in the same order.
            let mut expected = held.clone();
            expected[index] = new.clone();
            let got = reported_subscribers(&router, &account_id, &budget_name, &catalog[target])
                .await;
            assert_subscribers(&case, &expected, &got);

            // Every OTHER notification of the budget still holds its own set, untouched.
            for (other, notification) in catalog.iter().enumerate() {
                if other == target {
                    continue;
                }
                let got =
                    reported_subscribers(&router, &account_id, &budget_name, notification).await;
                assert_subscribers(
                    &format!("{case} — notification {other}"),
                    &subscriber_set(other),
                    &got,
                );
            }

            // The store agrees with the read, position for position.
            let stored = stored_subscribers(&engine, &account_id, &budget_name);
            let want: Vec<(String, String)> = expected
                .iter()
                .map(|subscriber| {
                    (
                        subscriber["SubscriptionType"].as_str().unwrap_or_default().to_string(),
                        subscriber["Address"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect();
            assert_eq!(
                stored[target], want,
                "case {case:?}: expected the store to hold {want:?}, got {:?}",
                stored[target]
            );

            // And the subscriber that was replaced is no longer one of that
            // notification's subscribers, so naming it again is not found.
            let (status, body) = update_subscriber(
                &router,
                &account_id,
                &budget_name,
                &catalog[target],
                &held[index],
                &new,
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected a second update of the same old subscriber to be \
                 refused, got {status} {body}"
            );
            assert!(
                is_error(&body, "NotFoundException"),
                "case {case:?}: expected NotFoundException on the second update, got {body}"
            );
        }
    }
}

/// RULE, the duplicate side: a subscriber's two members ARE its identity, so a
/// notification cannot end up holding two of the same subscriber — a `NewSubscriber`
/// naming another subscriber of the same notification is refused with
/// `DuplicateRecordException` and nothing is changed. Naming the subscriber being
/// replaced is not a collision with itself: that call is accepted and leaves the set
/// where it was.
///
/// The rows are every ordered pair of the notification's subscribers, including each one
/// updated to itself, over a set that holds both subscription types.
#[tokio::test]
async fn an_update_onto_another_subscriber_of_the_notification_is_a_duplicate() {
    assert_catalog_spanned();
    let (router, engine) = router_with_engine();
    let account_id = caller_account();
    let catalog = notification_catalog();
    let budget_name = "duplicate";
    seed(&router, &account_id, budget_name, &catalog).await;
    // The three-subscriber set: two emails with the one permitted SNS subscriber between
    // them, so the pairs span both types in both directions.
    let target = catalog
        .iter()
        .enumerate()
        .find(|(position, _)| subscriber_set(*position).len() == 3)
        .map(|(position, _)| position)
        .expect("one notification of the catalog holds a three-subscriber set");
    let held = subscriber_set(target);
    let before = stored_subscribers(&engine, &account_id, budget_name);

    for old in 0..held.len() {
        for new in 0..held.len() {
            let case = format!("{} -> {}", held[old], held[new]);
            let (status, body) = update_subscriber(
                &router,
                &account_id,
                budget_name,
                &catalog[target],
                &held[old],
                &held[new],
            )
            .await;
            if old == new {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected an update onto the same subscriber to be served, \
                     got {status} {body}"
                );
            } else {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected the duplicate to be refused at 400, got \
                     {status} {body}"
                );
                assert!(
                    is_error(&body, "DuplicateRecordException"),
                    "case {case:?}: expected DuplicateRecordException, got {body}"
                );
                assert!(
                    !message(&body).is_empty(),
                    "case {case:?}: expected a message a caller can read, got {body}"
                );
            }

            // Either way the set is exactly what it was, in order.
            let got = reported_subscribers(&router, &account_id, budget_name, &catalog[target])
                .await;
            assert_subscribers(&case, &held, &got);
            let after = stored_subscribers(&engine, &account_id, budget_name);
            assert_eq!(
                after, before,
                "case {case:?}: expected the store to stay at {before:?}, got {after:?}"
            );
        }
    }
}

/// One row of the not-found table: what the request names that is not held.
struct Absent {
    case: &'static str,
    budget_name: String,
    notification: Value,
    old_subscriber: Value,
}

/// RULE, the not-found side: an update reaches a subscriber only through a notification
/// of a budget the account holds, and names all three by value — so a budget that is not
/// held, a notification that budget does not alert on (differing in ANY member of a
/// notification's identity) and a subscriber that notification does not hold (differing
/// in either member of a subscriber's identity) are each refused with
/// `NotFoundException`, and nothing is written.
#[tokio::test]
async fn an_update_naming_something_not_held_is_not_found() {
    assert_catalog_spanned();
    let (router, engine) = router_with_engine();
    let account_id = caller_account();
    let catalog = notification_catalog();
    let budget_name = "absent";
    seed(&router, &account_id, budget_name, &catalog).await;
    let before = stored_subscribers(&engine, &account_id, budget_name);

    // The notification the rows address, and its first subscriber.
    let held = catalog[0].clone();
    let subscriber = subscriber_set(0)[0].clone();
    let new = json!({"SubscriptionType": "EMAIL", "Address": "replacement@example.com"});

    // A notification differing from the held one in ONE identity member at a time. The
    // notification type and the comparison operator are rotated off the model's own
    // catalogs; the threshold is moved; the threshold type is swapped for the other one
    // the model lists.
    let other_type = NOTIFICATION_TYPE_VALUES
        .iter()
        .find(|value| json!(value) != held["NotificationType"])
        .expect("the model lists more than one notification type");
    let other_operator = COMPARISON_OPERATOR_VALUES
        .iter()
        .find(|value| json!(value) != held["ComparisonOperator"])
        .expect("the model lists more than one comparison operator");
    let other_threshold_type = THRESHOLD_TYPE_VALUES
        .iter()
        .find(|value| json!(value) != held["ThresholdType"])
        .expect("the model lists more than one threshold type");

    let mut rows = vec![
        Absent {
            case: "a budget the account does not hold",
            budget_name: "no-such-budget".to_string(),
            notification: held.clone(),
            old_subscriber: subscriber.clone(),
        },
        Absent {
            case: "a notification differing in its notification type",
            budget_name: budget_name.to_string(),
            notification: {
                let mut notification = held.clone();
                notification["NotificationType"] = json!(other_type);
                notification
            },
            old_subscriber: subscriber.clone(),
        },
        Absent {
            case: "a notification differing in its comparison operator",
            budget_name: budget_name.to_string(),
            notification: {
                let mut notification = held.clone();
                notification["ComparisonOperator"] = json!(other_operator);
                notification
            },
            old_subscriber: subscriber.clone(),
        },
        Absent {
            case: "a notification differing in its threshold",
            budget_name: budget_name.to_string(),
            notification: {
                let mut notification = held.clone();
                notification["Threshold"] = json!(THRESHOLD_MAX);
                notification
            },
            old_subscriber: subscriber.clone(),
        },
        Absent {
            case: "a notification differing in its threshold type",
            budget_name: budget_name.to_string(),
            notification: {
                let mut notification = held.clone();
                notification["ThresholdType"] = json!(other_threshold_type);
                notification
            },
            old_subscriber: subscriber.clone(),
        },
        Absent {
            case: "a subscriber differing in its subscription type only",
            budget_name: budget_name.to_string(),
            notification: held.clone(),
            old_subscriber: json!({
                "SubscriptionType": "SNS",
                "Address": subscriber["Address"].clone()
            }),
        },
        Absent {
            case: "a subscriber differing in its address only",
            budget_name: budget_name.to_string(),
            notification: held.clone(),
            old_subscriber: json!({
                "SubscriptionType": subscriber["SubscriptionType"].clone(),
                "Address": "nobody@example.com"
            }),
        },
    ];
    // ... and a subscriber held by ANOTHER notification of the same budget, which this
    // notification does not hold: a subscriber belongs to the notification it was
    // attached to.
    rows.push(Absent {
        case: "a subscriber another notification of the same budget holds",
        budget_name: budget_name.to_string(),
        notification: held.clone(),
        old_subscriber: subscriber_set(1)[0].clone(),
    });

    for row in rows {
        let case = row.case;
        let (status, body) = update_subscriber(
            &router,
            &account_id,
            &row.budget_name,
            &row.notification,
            &row.old_subscriber,
            &new,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the refusal at 400, got {status} {body}"
        );
        assert!(
            is_error(&body, "NotFoundException"),
            "case {case:?}: expected NotFoundException, got {body}"
        );
        assert!(
            !message(&body).is_empty(),
            "case {case:?}: expected a message a caller can read, got {body}"
        );
        let after = stored_subscribers(&engine, &account_id, budget_name);
        assert_eq!(
            after, before,
            "case {case:?}: expected the refusal to leave the store at {before:?}, got {after:?}"
        );
    }
}

/// RULE, the capacity side: "a notification can have one SNS subscriber and up to 10
/// email subscribers, for a total of 11 subscribers" (`Notification`) — a bound on the
/// SET, so a replacement that changes a subscriber's TYPE is held to it even though a
/// swap never changes the set's size. The refusal is the client error this operation
/// models, and a replacement that stays inside the split is served.
///
/// The rows are each per-type half at its published bound and one past it: an email
/// replaced by a second SNS subscriber (past the SNS half), the SNS subscriber of a set
/// already holding the maximum emails replaced by one more email (past the email half),
/// and the same two sets with a replacement of the same type as the subscriber it
/// replaces (at each bound, served).
#[tokio::test]
async fn an_update_cannot_take_a_notification_outside_the_per_type_subscriber_split() {
    assert_catalog_spanned();
    let (router, engine) = router_with_engine();
    let account_id = caller_account();
    let budget_name = "per-type-split";

    // A set at the SNS half's bound: one SNS subscriber and one email.
    let at_sns_bound: Vec<Value> = std::iter::repeat_with(|| sns(0))
        .take(MAX_SNS_SUBSCRIBERS_PER_NOTIFICATION)
        .chain(std::iter::once(email(0, 0)))
        .collect();
    // A set at the email half's bound: the maximum emails, plus the one SNS subscriber.
    let at_email_bound: Vec<Value> = (0..MAX_EMAIL_SUBSCRIBERS_PER_NOTIFICATION)
        .map(|index| email(1, index))
        .chain(std::iter::once(sns(1)))
        .collect();

    let notifications = [
        json!({"NotificationType": "ACTUAL", "ComparisonOperator": "GREATER_THAN", "Threshold": 10.0}),
        json!({"NotificationType": "ACTUAL", "ComparisonOperator": "LESS_THAN", "Threshold": 20.0}),
    ];
    let (status, body) = call(
        &router,
        "CreateBudget",
        &json!({
            "AccountId": &account_id,
            "Budget": {
                "BudgetName": budget_name,
                "TimeUnit": "MONTHLY",
                "BudgetType": "COST",
                "BudgetLimit": {"Amount": "100", "Unit": "USD"}
            },
            "NotificationsWithSubscribers": [
                {"Notification": &notifications[0], "Subscribers": &at_sns_bound},
                {"Notification": &notifications[1], "Subscribers": &at_email_bound}
            ]
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: expected CreateBudget to answer 200, got {status} {body}"
    );
    let before = stored_subscribers(&engine, &account_id, budget_name);

    // Past each half: a replacement of the OTHER type than the subscriber it replaces.
    let past: Vec<(&str, usize, Value, Value)> = vec![
        (
            "an email replaced by a second SNS subscriber",
            0,
            at_sns_bound[MAX_SNS_SUBSCRIBERS_PER_NOTIFICATION].clone(),
            json!({
                "SubscriptionType": "SNS",
                "Address": format!("arn:aws:sns:us-east-1:{}:second", caller_account())
            }),
        ),
        (
            "the SNS subscriber replaced by one email past the email half",
            1,
            sns(1),
            email(1, MAX_EMAIL_SUBSCRIBERS_PER_NOTIFICATION),
        ),
    ];
    for (case, target, old, new) in past {
        let (status, body) = update_subscriber(
            &router,
            &account_id,
            budget_name,
            &notifications[target],
            &old,
            &new,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the refusal at 400, got {status} {body}"
        );
        assert!(
            is_error(&body, "InvalidParameterException"),
            "case {case:?}: expected InvalidParameterException, got {body}"
        );
        let after = stored_subscribers(&engine, &account_id, budget_name);
        assert_eq!(
            after, before,
            "case {case:?}: expected the refusal to leave the store at {before:?}, got {after:?}"
        );
    }

    // At each half: a replacement of the SAME type, which leaves both halves where they
    // were and is served.
    let served: Vec<(&str, usize, Value, Value)> = vec![
        (
            "the SNS subscriber replaced by another SNS topic",
            0,
            sns(0),
            json!({
                "SubscriptionType": "SNS",
                "Address": format!("arn:aws:sns:us-east-1:{}:moved", caller_account())
            }),
        ),
        (
            "one of the maximum emails replaced by another email",
            1,
            email(1, 0),
            json!({"SubscriptionType": "EMAIL", "Address": "moved@example.com"}),
        ),
    ];
    for (case, target, old, new) in served {
        let (status, body) = update_subscriber(
            &router,
            &account_id,
            budget_name,
            &notifications[target],
            &old,
            &new,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateSubscriber to answer 200, got {status} {body}"
        );
        let got =
            reported_subscribers(&router, &account_id, budget_name, &notifications[target]).await;
        let held = if target == 0 {
            &at_sns_bound
        } else {
            &at_email_bound
        };
        let position = held
            .iter()
            .position(|subscriber| same(subscriber, &old))
            .expect("the subscriber being replaced is one the notification holds");
        let mut expected = held.clone();
        expected[position] = new.clone();
        assert_subscribers(case, &expected, &got);
    }
}
