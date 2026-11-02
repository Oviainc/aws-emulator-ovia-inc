// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the CAPACITY limits `CreateBudget` enforces over the
//! notifications and subscribers it attaches to the budget it creates.
//!
//! The RULE every test here asserts: a create refuses, with the modelled
//! `CreationLimitExceededException`, a set of notifications or subscribers larger than
//! the budget it would create can hold — the documented ten notifications a budget can
//! hold, and per notification one SNS and ten email subscribers — and a refused create
//! stores nothing at all, while a set within every limit is accepted and reads back
//! whole. The notification capacity belongs to the BUDGET, not to the call, so a batch
//! create and the same notifications appended one at a time are accepted alike.
//!
//! The modelled `@length(1, 11)` on `Subscribers` is wider than the per-type subscriber
//! limits, so the subscriber table spans both: a size the model rejects at the boundary
//! is a `ValidationException`, a size it accepts but the service cannot hold is the
//! modelled limit error, and the sizes on and under each limit are accepted.
//!
//! Each table's rows differ along the count, the subscription type and the position of
//! the offending notification inside the batch; the harness (`router`, `call`,
//! `is_error`, `caller_account`, `same`) is the lifecycle suite's, so every suite in this
//! target drives the service identically.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The limits are the crate's own model-derived constants, so a table built from them
/// cannot drift from the handler it checks.
use service_com_amazonaws_budgets_awsbudgetservicegateway::state::{
    MAX_EMAIL_SUBSCRIBERS_PER_NOTIFICATION, MAX_NOTIFICATIONS_PER_BUDGET,
    MAX_SNS_SUBSCRIBERS_PER_NOTIFICATION,
};

use super::budget_lifecycle::{caller_account, call, is_error, router, same};

/// This service answers every 4xx refusal with HTTP 400 whatever `@httpError` the
/// modelled shape declares (`crate::scaffolded::modeled_error_status`), so the status
/// is asserted beside the error code rather than per row.
const REFUSAL_STATUS: u16 = 400;

/// A budget that satisfies every modelled constraint and documented cross-member rule,
/// so a row varies only the notifications attached to it.
fn budget(name: &str) -> Value {
    json!({
        "BudgetName": name,
        "TimeUnit": "MONTHLY",
        "BudgetType": "COST",
        "BudgetLimit": {"Amount": "100", "Unit": "USD"}
    })
}

/// A notification identified by its threshold, so `index` notifications built this way
/// are `index` DIFFERENT notifications rather than repeats of one.
fn notification(index: usize) -> Value {
    json!({
        "NotificationType": "ACTUAL",
        "ComparisonOperator": "GREATER_THAN",
        "Threshold": (index as f64) + 0.5,
        "ThresholdType": "PERCENTAGE"
    })
}

/// `count` distinct `EMAIL` subscribers.
fn email_subscribers(count: usize) -> Vec<Value> {
    (0..count)
        .map(|i| json!({"SubscriptionType": "EMAIL", "Address": format!("limit{i}@example.com")}))
        .collect()
}

/// `count` distinct `SNS` subscribers.
fn sns_subscribers(count: usize) -> Vec<Value> {
    (0..count)
        .map(|i| {
            json!({
                "SubscriptionType": "SNS",
                "Address": format!("arn:aws:sns:us-east-1:{}:alerts-{i}", caller_account())
            })
        })
        .collect()
}

/// `count` notifications, each with one email subscriber.
fn notifications_with_subscribers(count: usize) -> Vec<Value> {
    (0..count)
        .map(|index| {
            json!({
                "Notification": notification(index),
                "Subscribers": email_subscribers(1)
            })
        })
        .collect()
}

/// Call `CreateBudget` for `name` under `account_id` with `attached`.
async fn create(
    router: &MultiServiceRouter,
    account_id: &str,
    name: &str,
    attached: &[Value],
) -> (u16, Value) {
    call(
        router,
        "CreateBudget",
        &json!({
            "AccountId": account_id,
            "Budget": budget(name),
            "NotificationsWithSubscribers": attached
        }),
    )
    .await
}

/// Append `index`'s notification to an existing budget through `CreateNotification`,
/// with the one email subscriber the batch form attaches.
async fn append(
    router: &MultiServiceRouter,
    account_id: &str,
    name: &str,
    index: usize,
) -> (u16, Value) {
    call(
        router,
        "CreateNotification",
        &json!({
            "AccountId": account_id,
            "BudgetName": name,
            "Notification": notification(index),
            "Subscribers": email_subscribers(1)
        }),
    )
    .await
}

/// Assert that `case`'s refused create left NO budget behind: a budget the service
/// cannot hold is not created with the part of the request it could hold.
async fn assert_nothing_stored(
    router: &MultiServiceRouter,
    case: &str,
    account_id: &str,
    name: &str,
) {
    let (status, body) = call(
        router,
        "DescribeBudget",
        &json!({"AccountId": account_id, "BudgetName": name}),
    )
    .await;
    assert_eq!(
        status, REFUSAL_STATUS,
        "case {case:?}: expected the refused budget never to exist, got {status} {body}"
    );
    assert!(
        is_error(&body, "NotFoundException"),
        "case {case:?}: expected NotFoundException for the refused budget, got {body}"
    );
}

/// The notifications `DescribeNotificationsForBudget` reports for a budget, in order.
async fn reported_notifications(
    router: &MultiServiceRouter,
    account_id: &str,
    name: &str,
) -> Vec<Value> {
    let (status, body) = call(
        router,
        "DescribeNotificationsForBudget",
        &json!({"AccountId": account_id, "BudgetName": name, "MaxResults": 100}),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected DescribeNotificationsForBudget to answer 200, got {status} {body}"
    );
    body["Notifications"].as_array().cloned().unwrap_or_default()
}

/// RULE: a create attaches any number of notifications the budget it creates can hold —
/// up to the ten a budget's capacity allows, which is also the modelled list bound — so
/// every such set is accepted and reads back in order, and only a list past the modelled
/// bound is refused, at request validation.
///
/// The rows are the counts across the whole admissible range: none, one, half the
/// capacity, one under it, the capacity itself, and one past the modelled bound.
#[tokio::test]
async fn a_create_attaches_as_many_notifications_as_the_budget_can_hold() {
    // `NotificationWithSubscribersList` is modelled @length(0, 10) — the same ten
    // `Notification`'s doc gives as a budget's capacity, so the wire bound and the
    // capacity coincide and the table is built from one figure.
    const MODELLED_MAX: usize = 10;
    assert_eq!(
        MAX_NOTIFICATIONS_PER_BUDGET, MODELLED_MAX,
        "case \"the modelled list bound and a budget's capacity\": expected both to be \
         {MODELLED_MAX}, got {MAX_NOTIFICATIONS_PER_BUDGET}"
    );
    let rows: Vec<(usize, Option<&str>)> = vec![
        (0, None),
        (1, None),
        (MAX_NOTIFICATIONS_PER_BUDGET / 2, None),
        (MAX_NOTIFICATIONS_PER_BUDGET - 1, None),
        (MAX_NOTIFICATIONS_PER_BUDGET, None),
        (MODELLED_MAX + 1, Some("ValidationException")),
    ];

    for (count, refusal) in rows {
        let case = format!("{count} notifications");
        // One store per row, so no row can see another's budget: every request names
        // the one account this service serves.
        let router = router();
        let account_id = caller_account();
        let name = "limits";
        let attached = notifications_with_subscribers(count);
        let (status, body) = create(&router, &account_id, name, &attached).await;

        match refusal {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected a set within the limit to answer 200, \
                     got {status} {body}"
                );
                let reported = reported_notifications(&router, &account_id, name).await;
                assert_eq!(
                    reported.len(),
                    count,
                    "case {case:?}: expected {count} notifications to read back, \
                     got {reported:?}"
                );
                for (position, sent) in attached.iter().enumerate() {
                    let expected = sent["Notification"]["Threshold"].clone();
                    let actual = reported[position]
                        .get("Threshold")
                        .cloned()
                        .unwrap_or(Value::Null);
                    assert!(
                        same(&expected, &actual),
                        "case {case:?}: notification {position} expected Threshold \
                         {expected}, got {actual}"
                    );
                }
            }
            Some(expected) => {
                assert_eq!(
                    status, REFUSAL_STATUS,
                    "case {case:?}: expected the refusal at {REFUSAL_STATUS}, \
                     got {status} {body}"
                );
                assert!(
                    is_error(&body, expected),
                    "case {case:?}: expected {expected}, got {body}"
                );
                assert_nothing_stored(&router, &case, &account_id, name).await;
            }
        }
    }
}

/// RULE: the notification capacity belongs to the BUDGET, not to the call that attaches
/// them, so the same end state is reachable however the notifications arrive: a create
/// carrying some of them and `CreateNotification` appending the rest both answer 200 up
/// to the capacity, the budget reads back every one of them in arrival order, and only
/// the notification past the capacity is refused with
/// `CreationLimitExceededException`.
///
/// The rows are every split of the capacity between the batch form and the incremental
/// form, from all-appended to all-batched.
#[tokio::test]
async fn the_notification_capacity_is_the_same_batched_or_appended() {
    for batched in 0..=MAX_NOTIFICATIONS_PER_BUDGET {
        let appended = MAX_NOTIFICATIONS_PER_BUDGET - batched;
        let case = format!("{batched} batched, {appended} appended");
        // One store per row, so no row can see another's budget.
        let router = router();
        let account_id = caller_account();
        let name = "capacity-splits";
        let (status, body) = create(
            &router,
            &account_id,
            name,
            &notifications_with_subscribers(batched),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the batch create to answer 200, got {status} {body}"
        );
        for index in batched..MAX_NOTIFICATIONS_PER_BUDGET {
            let (status, body) = append(&router, &account_id, name, index).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected notification {index} to append with 200, \
                 got {status} {body}"
            );
        }

        // One past the capacity, whichever way the budget reached it.
        let (status, body) = append(&router, &account_id, name, MAX_NOTIFICATIONS_PER_BUDGET).await;
        assert_eq!(
            status, REFUSAL_STATUS,
            "case {case:?}: expected the notification past the capacity of \
             {MAX_NOTIFICATIONS_PER_BUDGET} to be refused at {REFUSAL_STATUS}, \
             got {status} {body}"
        );
        assert!(
            is_error(&body, "CreationLimitExceededException"),
            "case {case:?}: expected CreationLimitExceededException past the capacity, \
             got {body}"
        );

        let reported = reported_notifications(&router, &account_id, name).await;
        assert_eq!(
            reported.len(),
            MAX_NOTIFICATIONS_PER_BUDGET,
            "case {case:?}: expected {MAX_NOTIFICATIONS_PER_BUDGET} notifications to read \
             back, got {reported:?}"
        );
        for (index, held) in reported.iter().enumerate() {
            let expected = notification(index)["Threshold"].clone();
            let actual = held.get("Threshold").cloned().unwrap_or(Value::Null);
            assert!(
                same(&expected, &actual),
                "case {case:?}: notification {index} expected Threshold {expected}, \
                 got {actual}"
            );
        }
    }
}

/// RULE: every notification a create attaches stays within the documented per-type
/// subscriber limits — one SNS subscriber and ten email subscribers — so a notification
/// past either half is refused with `CreationLimitExceededException` even when the
/// modelled `@length(1, 11)` on `Subscribers` accepted the list, and an accepted set
/// reads back through `DescribeSubscribersForNotification`.
///
/// The rows sit on each published bound and one past it, per subscription type, plus
/// the modelled total and one past that.
#[tokio::test]
async fn a_created_notification_holds_at_most_the_documented_subscribers_of_each_type() {
    let sns_and_email = [
        sns_subscribers(MAX_SNS_SUBSCRIBERS_PER_NOTIFICATION),
        email_subscribers(MAX_EMAIL_SUBSCRIBERS_PER_NOTIFICATION),
    ]
    .concat();
    let two_sns_and_email = [
        sns_subscribers(MAX_SNS_SUBSCRIBERS_PER_NOTIFICATION + 1),
        email_subscribers(MAX_EMAIL_SUBSCRIBERS_PER_NOTIFICATION - 1),
    ]
    .concat();

    // (case, the subscribers of the one notification, the error expected or None)
    let rows: Vec<(&str, Vec<Value>, Option<&str>)> = vec![
        ("one email, the modelled minimum", email_subscribers(1), None),
        (
            "the email maximum",
            email_subscribers(MAX_EMAIL_SUBSCRIBERS_PER_NOTIFICATION),
            None,
        ),
        (
            "one SNS, its maximum",
            sns_subscribers(MAX_SNS_SUBSCRIBERS_PER_NOTIFICATION),
            None,
        ),
        (
            "one SNS and the email maximum: the modelled total of 11",
            sns_and_email,
            None,
        ),
        (
            "one past the email maximum",
            email_subscribers(MAX_EMAIL_SUBSCRIBERS_PER_NOTIFICATION + 1),
            Some("CreationLimitExceededException"),
        ),
        (
            "one past the SNS maximum",
            sns_subscribers(MAX_SNS_SUBSCRIBERS_PER_NOTIFICATION + 1),
            Some("CreationLimitExceededException"),
        ),
        (
            "one past the SNS maximum, inside the modelled total",
            two_sns_and_email,
            Some("CreationLimitExceededException"),
        ),
        (
            "one past the modelled total",
            email_subscribers(MAX_EMAIL_SUBSCRIBERS_PER_NOTIFICATION + 2),
            Some("ValidationException"),
        ),
        (
            "fewer than the modelled minimum",
            Vec::new(),
            Some("ValidationException"),
        ),
    ];

    for (index, (case, subscribers, refusal)) in rows.into_iter().enumerate() {
        // One store per row, so no row can see another's budget.
        let router = router();
        let account_id = caller_account();
        let name = "subscriber-limits";
        let attached = vec![json!({
            "Notification": notification(index),
            "Subscribers": subscribers.clone()
        })];
        let (status, body) = create(&router, &account_id, name, &attached).await;

        match refusal {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected a subscriber set within the limits to answer \
                     200, got {status} {body}"
                );
                let (status, body) = call(
                    &router,
                    "DescribeSubscribersForNotification",
                    &json!({
                        "AccountId": &account_id,
                        "BudgetName": name,
                        "Notification": notification(index),
                        "MaxResults": 100
                    }),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected DescribeSubscribersForNotification to answer \
                     200, got {status} {body}"
                );
                let expected = Value::Array(subscribers);
                assert!(
                    same(&body["Subscribers"], &expected),
                    "case {case:?}: expected the subscribers {expected} to read back in \
                     order, got {body}"
                );
            }
            Some(expected) => {
                assert_eq!(
                    status, REFUSAL_STATUS,
                    "case {case:?}: expected the refusal at {REFUSAL_STATUS}, \
                     got {status} {body}"
                );
                assert!(
                    is_error(&body, expected),
                    "case {case:?}: expected {expected}, got {body}"
                );
                assert_nothing_stored(&router, case, &account_id, name).await;
            }
        }
    }
}

/// RULE: the limits hold over the WHOLE request, so one notification above its
/// subscriber limit refuses the create wherever it sits in the batch — and the
/// notifications beside it are not stored either.
///
/// The rows are each position of a three-notification batch in turn.
#[tokio::test]
async fn a_notification_above_its_subscriber_limit_refuses_the_whole_create() {
    let batch = 3;
    for offending in 0..batch {
        let case = format!("offending notification at position {offending} of {batch}");
        // One store per row, so no row can see another's budget.
        let router = router();
        let account_id = caller_account();
        let name = "batch-limits";
        let attached: Vec<Value> = (0..batch)
            .map(|index| {
                let subscribers = if index == offending {
                    email_subscribers(MAX_EMAIL_SUBSCRIBERS_PER_NOTIFICATION + 1)
                } else {
                    email_subscribers(1)
                };
                json!({"Notification": notification(index), "Subscribers": subscribers})
            })
            .collect();
        let (status, body) = create(&router, &account_id, name, &attached).await;
        assert_eq!(
            status, REFUSAL_STATUS,
            "case {case:?}: expected the refusal at {REFUSAL_STATUS}, got {status} {body}"
        );
        assert!(
            is_error(&body, "CreationLimitExceededException"),
            "case {case:?}: expected CreationLimitExceededException, got {body}"
        );
        assert_nothing_stored(&router, &case, &account_id, name).await;
    }
}
