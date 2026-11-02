// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for `CreateSubscriber`, read back through
//! `DescribeSubscribersForNotification` and through the store itself, driven over the
//! wire through the same `MultiServiceRouter` the production router serves from (the
//! harness is the lifecycle suite's, so every suite in this target drives the service
//! identically).
//!
//! Each test states a RULE and asserts it over a table that spans the class the rule
//! generalises over, not one request:
//!
//! 1. A created subscriber is PERSISTED on the notification it was created for, read
//!    back as supplied and appended after the subscribers that notification already
//!    held; no other notification of the budget is touched.
//! 2. A subscriber is identified by BOTH of its members — the subscription type and the
//!    address — so a repeat is a `DuplicateRecordException` and a difference in either
//!    one is a different subscriber.
//! 3. A subscriber is reached only through a notification of a budget the account holds
//!    ("you must create the associated budget and notification before you create the
//!    subscriber"), and the notification is named by what it alerts on — never by the
//!    service-owned `NotificationState`.
//! 4. The documented per-type limits — "one SNS subscriber and up to 10 email
//!    subscribers, for a total of 11 subscribers" — hold when a notification is grown
//!    one subscriber at a time.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// The enum catalogs are not re-typed here: these are the model-derived constants the
/// scaffold generated into `src/ops/types/shared_types.rs`, so a table built from them
/// cannot drift from the model.
use service_com_amazonaws_budgets_awsbudgetservicegateway::ops::types::{
    COMPARISON_OPERATOR_VALUES, NOTIFICATION_STATE_VALUES, NOTIFICATION_TYPE_VALUES,
    SUBSCRIPTION_TYPE_VALUES, THRESHOLD_TYPE_VALUES,
};
use service_com_amazonaws_budgets_awsbudgetservicegateway::state::{
    ComAmazonawsBudgetsAwsbudgetservicegatewayState, MAX_EMAIL_SUBSCRIBERS_PER_NOTIFICATION,
    MAX_SNS_SUBSCRIBERS_PER_NOTIFICATION,
};

use super::budget_lifecycle::{caller_account, call, is_error, router_with_engine, same};

/// The subscriber every seeded notification starts with: `Subscribers` is
/// `@length(min: 1)`, so a notification cannot be created without one, and every table
/// below adds to that set rather than establishing it.
fn seed_subscriber() -> Value {
    json!({"SubscriptionType": "EMAIL", "Address": "seed@example.com"})
}

/// The notifications the tables create subscribers on.
///
/// The rows differ along what a notification's identity is made of: the notification
/// type and the comparison operator (the full cross product of the model's two
/// catalogs), the threshold (each published `@range` bound of `Notification$Threshold`
/// — 0 and 15000000000000 — and a fraction between them) and the threshold type (each
/// `ThresholdType` the model lists, plus absent).
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
            catalog.push(notification);
        }
    }
    catalog
}

/// Create `budget_name` under `account_id` holding `catalog`, each notification with
/// [`seed_subscriber`], in catalog order. The budget itself is created without
/// notifications and each one is appended through `CreateNotification`, so the catalog
/// can be longer than a single `CreateBudget` attaches.
async fn seed(
    router: &MultiServiceRouter,
    account_id: &str,
    budget_name: &str,
    catalog: &[Value],
) {
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
            }
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup {budget_name:?}: expected CreateBudget to answer 200, got {status} {body}"
    );
    for (position, notification) in catalog.iter().enumerate() {
        let (status, body) = call(
            router,
            "CreateNotification",
            &json!({
                "AccountId": account_id,
                "BudgetName": budget_name,
                "Notification": notification,
                "Subscribers": [seed_subscriber()]
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "setup {budget_name:?} notification {position}: expected CreateNotification to \
             answer 200, got {status} {body}"
        );
    }
}

/// One `CreateSubscriber` call.
///
/// `pub(crate)` so every suite that drives the growth path calls it the same way.
pub(crate) async fn create_subscriber(
    router: &MultiServiceRouter,
    account_id: &str,
    budget_name: &str,
    notification: &Value,
    subscriber: &Value,
) -> (u16, Value) {
    call(
        router,
        "CreateSubscriber",
        &json!({
            "AccountId": account_id,
            "BudgetName": budget_name,
            "Notification": notification,
            "Subscriber": subscriber
        }),
    )
    .await
}

/// The subscribers `DescribeSubscribersForNotification` reports for one notification,
/// in order. Fails the test if the read itself did not answer.
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
        "reading {budget_name:?} subscribers back: expected 200, got {status} {body}"
    );
    body["Subscribers"].as_array().cloned().unwrap_or_default()
}

/// Every subscriber the STORE holds for a budget, as `(SubscriptionType, Address)` per
/// notification, in notification order. Read from the state struct directly, so what the
/// create wrote is asserted independently of any read operation's own projection.
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
        .expect("the budget under test is stored")
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
}

/// A subscriber as `(SubscriptionType, Address)`, for comparing a reported or stored set
/// against the one the table supplied.
pub(crate) fn pair(subscriber: &Value) -> (String, String) {
    (
        subscriber["SubscriptionType"].as_str().unwrap_or_default().to_string(),
        subscriber["Address"].as_str().unwrap_or_default().to_string(),
    )
}

/// One row of the persistence table: the subscribers to add, one call each, in order.
struct Row {
    case: &'static str,
    added: Vec<Value>,
}

/// The persistence table. The rows differ along what rule 1 generalises over: the
/// subscription type (both values the model lists), the address form (an email, an email
/// with a tag and a subdomain chain, the shortest address the email form admits, and an
/// SNS topic ARN) and the number of subscribers added to one notification.
///
/// Every address is in the form its own `SubscriptionType` names — "either an Amazon SNS
/// topic or an email address" (`Subscriber`) — which is what a write accepts at all;
/// `subscriber_address_validation.rs` spans the addresses that are not.
fn persistence_rows() -> Vec<Row> {
    let of_each_type: Vec<Value> = SUBSCRIPTION_TYPE_VALUES
        .iter()
        .map(|subscription_type| match *subscription_type {
            "SNS" => json!({
                "SubscriptionType": "SNS",
                "Address": format!("arn:aws:sns:us-east-1:{}:each-type", caller_account())
            }),
            other => json!({"SubscriptionType": other, "Address": "each-type@example.com"}),
        })
        .collect();
    vec![
        Row {
            case: "one email subscriber",
            added: vec![json!({"SubscriptionType": "EMAIL", "Address": "ops@example.com"})],
        },
        Row {
            case: "one SNS topic subscriber",
            added: vec![json!({
                "SubscriptionType": "SNS",
                "Address": format!("arn:aws:sns:us-east-1:{}:budget-alerts", caller_account())
            })],
        },
        Row {
            case: "three email subscribers, one call each",
            added: vec![
                json!({"SubscriptionType": "EMAIL", "Address": "a@example.com"}),
                json!({"SubscriptionType": "EMAIL", "Address": "b@example.com"}),
                json!({"SubscriptionType": "EMAIL", "Address": "c@example.com"}),
            ],
        },
        Row {
            case: "one subscriber of every subscription type the model lists",
            added: of_each_type,
        },
        Row {
            case: "the shortest address the email form admits",
            added: vec![json!({"SubscriptionType": "EMAIL", "Address": "a@b.co"})],
        },
        Row {
            case: "an address with a tag and a subdomain chain",
            added: vec![json!({
                "SubscriptionType": "EMAIL",
                "Address": "ops+monthly@team.example.co.uk"
            })],
        },
    ]
}

/// RULE 1: the created subscriber is persisted on the notification it names, appended
/// after the subscribers that notification already held, and read back as supplied —
/// while every other notification of the budget keeps exactly the subscribers it had.
///
/// Every row of [`persistence_rows`] is asserted against every position of
/// [`notification_catalog`], so the rule is checked on the first, a middle and the last
/// notification of a budget, over notifications that differ in every identity member.
#[tokio::test]
async fn a_created_subscriber_is_persisted_on_its_notification() {
    let (router, engine) = router_with_engine();
    let account_id = caller_account();
    let catalog = notification_catalog();

    for (row_index, row) in persistence_rows().into_iter().enumerate() {
        for target in 0..catalog.len() {
            let budget_name = format!("persist-{row_index}-{target}");
            seed(&router, &account_id, &budget_name, &catalog).await;
            let case = format!(
                "{} on notification {target} of {}",
                row.case,
                catalog.len()
            );

            for subscriber in &row.added {
                let (status, body) = create_subscriber(
                    &router,
                    &account_id,
                    &budget_name,
                    &catalog[target],
                    subscriber,
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected CreateSubscriber({subscriber}) to answer 200, \
                     got {status} {body}"
                );
            }

            let expected: Vec<(String, String)> = std::iter::once(pair(&seed_subscriber()))
                .chain(row.added.iter().map(pair))
                .collect();
            let reported: Vec<(String, String)> =
                reported_subscribers(&router, &account_id, &budget_name, &catalog[target])
                    .await
                    .iter()
                    .map(pair)
                    .collect();
            assert_eq!(
                reported, expected,
                "case {case:?}: expected the read to report {expected:?}, got {reported:?}"
            );

            let held = stored_subscribers(&engine, &account_id, &budget_name);
            assert_eq!(
                held[target], expected,
                "case {case:?}: expected the store to hold {expected:?}, got {:?}",
                held[target]
            );
            for (position, other) in held.iter().enumerate() {
                if position == target {
                    continue;
                }
                assert_eq!(
                    other,
                    &vec![pair(&seed_subscriber())],
                    "case {case:?}: expected notification {position} to keep only the \
                     subscriber it was created with, got {other:?}"
                );
            }
        }
    }
}

/// RULE 2: a subscriber "consists of a subscription type and either an Amazon SNS topic
/// or an email address", and both members are `@required` — so both are its identity. A
/// subscriber the notification already holds is refused with `DuplicateRecordException`
/// and changes nothing; one differing in EITHER member is a different subscriber and is
/// stored beside it.
///
/// The rows differ along which member differs and how: the subscription type alone, the
/// address alone, the letter case of the address, and the domain it names.
///
/// A row differing in the subscription type alone carries the address form THAT type
/// names ("either an Amazon SNS topic or an email address"), since an address in the
/// other type's form is refused before identity is ever compared
/// (`subscriber_address_validation.rs`).
#[tokio::test]
async fn a_subscriber_is_identified_by_its_type_and_its_address() {
    let (router, engine) = router_with_engine();
    let account_id = caller_account();
    let notification = json!({
        "NotificationType": "ACTUAL",
        "ComparisonOperator": "GREATER_THAN",
        "Threshold": 80.0
    });
    let budget_name = "identity";
    seed(
        &router,
        &account_id,
        budget_name,
        std::slice::from_ref(&notification),
    )
    .await;

    // `(the subscriber to create, whether it is the same subscriber as one already held)`
    let rows: Vec<(&str, Value, bool)> = vec![
        (
            "the subscriber the notification was created with",
            seed_subscriber(),
            true,
        ),
        (
            "a subscriber of the other subscription type",
            json!({
                "SubscriptionType": "SNS",
                "Address": format!("arn:aws:sns:us-east-1:{}:identity", caller_account())
            }),
            false,
        ),
        (
            "that SNS subscriber a second time",
            json!({
                "SubscriptionType": "SNS",
                "Address": format!("arn:aws:sns:us-east-1:{}:identity", caller_account())
            }),
            true,
        ),
        (
            "the same type with an address differing only in letter case",
            json!({"SubscriptionType": "EMAIL", "Address": "SEED@example.com"}),
            false,
        ),
        (
            "the same type with an address differing only in its domain",
            json!({"SubscriptionType": "EMAIL", "Address": "seed@example.net"}),
            false,
        ),
        (
            "that other-domain address a second time",
            json!({"SubscriptionType": "EMAIL", "Address": "seed@example.net"}),
            true,
        ),
    ];

    let mut expected = vec![pair(&seed_subscriber())];
    for (case, subscriber, is_duplicate) in rows {
        let (status, body) = create_subscriber(
            &router,
            &account_id,
            budget_name,
            &notification,
            &subscriber,
        )
        .await;
        if is_duplicate {
            assert_eq!(
                status, 400,
                "case {case:?}: expected a subscriber already held to be refused, \
                 got {status} {body}"
            );
            assert!(
                is_error(&body, "DuplicateRecordException"),
                "case {case:?}: expected DuplicateRecordException, got {body}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case:?}: expected a different subscriber to be accepted, \
                 got {status} {body}"
            );
            expected.push(pair(&subscriber));
        }
        let held = stored_subscribers(&engine, &account_id, budget_name);
        assert_eq!(
            held[0], expected,
            "case {case:?}: expected the notification to hold {expected:?}, got {:?}",
            held[0]
        );
    }
}

/// RULE 3: the subscriber is reached only through a notification of a budget the account
/// holds — a name the account holds no budget under, or a notification the budget
/// does not alert on, is refused with `NotFoundException` and stores nothing — while the
/// service-owned `NotificationState` is not part of what names the notification, so any
/// value for it (or none) still reaches the stored one. A `BudgetName` outside its
/// modelled `@pattern` is the request-validation refusal it is, not a not-found.
#[tokio::test]
async fn a_subscriber_is_reached_only_through_a_notification_of_an_existing_budget() {
    let (router, engine) = router_with_engine();
    let owner = caller_account();
    let notification = json!({
        "NotificationType": "ACTUAL",
        "ComparisonOperator": "GREATER_THAN",
        "Threshold": 80.0,
        "ThresholdType": "PERCENTAGE"
    });
    let budget_name = "stored-name";
    seed(
        &router,
        &owner,
        budget_name,
        std::slice::from_ref(&notification),
    )
    .await;
    let subscriber = json!({"SubscriptionType": "EMAIL", "Address": "reached@example.com"});

    // The budget side: a name the account holds nothing under, and a stored name whose
    // case does not match.
    for (case, named_budget) in [
        ("a name the account holds no budget under", "absent"),
        ("the stored name in another case", "STORED-NAME"),
    ] {
        let (status, body) = create_subscriber(
            &router,
            &owner,
            named_budget,
            &notification,
            &subscriber,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected CreateSubscriber to be refused, got {status} {body}"
        );
        assert!(
            is_error(&body, "NotFoundException"),
            "case {case:?}: expected NotFoundException, got {body}"
        );
    }

    // The notification side: one row per identity member, each varying that member alone.
    for (member, value) in [
        ("NotificationType", json!("FORECASTED")),
        ("ComparisonOperator", json!("LESS_THAN")),
        ("ComparisonOperator", json!("EQUAL_TO")),
        ("Threshold", json!(80.5)),
        ("Threshold", json!(0.0)),
        ("ThresholdType", json!("ABSOLUTE_VALUE")),
    ] {
        let mut near_miss = notification.clone();
        near_miss[member] = value;
        let case = format!("{member} differs: {near_miss}");
        let (status, body) =
            create_subscriber(&router, &owner, budget_name, &near_miss, &subscriber).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected CreateSubscriber to be refused, got {status} {body}"
        );
        assert!(
            is_error(&body, "NotFoundException"),
            "case {case:?}: expected NotFoundException, got {body}"
        );
    }

    // None of those refusals stored anything.
    let held = stored_subscribers(&engine, &owner, budget_name);
    assert_eq!(
        held[0],
        vec![pair(&seed_subscriber())],
        "expected the refused creates to store nothing, got {:?}",
        held[0]
    );

    // The alarm state is not part of the notification's identity: every value the model
    // lists, and absent, names the stored notification. Each call carries a distinct
    // address so the accepted ones are not duplicates of each other.
    let states: Vec<Option<&str>> = NOTIFICATION_STATE_VALUES
        .iter()
        .map(|value| Some(*value))
        .chain(std::iter::once(None))
        .collect();
    let mut expected = vec![pair(&seed_subscriber())];
    for sent_state in &states {
        let label = sent_state.unwrap_or("absent");
        let mut named = notification.clone();
        match sent_state {
            Some(state) => named["NotificationState"] = json!(state),
            None => {
                named
                    .as_object_mut()
                    .expect("a notification object")
                    .remove("NotificationState");
            }
        }
        let subscriber =
            json!({"SubscriptionType": "EMAIL", "Address": format!("state-{label}@example.com")});
        let case = format!("NotificationState named as {label:?}");
        let (status, body) =
            create_subscriber(&router, &owner, budget_name, &named, &subscriber).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateSubscriber to answer 200, got {status} {body}"
        );
        expected.push(pair(&subscriber));
        let held = stored_subscribers(&engine, &owner, budget_name);
        assert_eq!(
            held[0], expected,
            "case {case:?}: expected the notification to hold {expected:?}, got {:?}",
            held[0]
        );
    }

    // A name outside `BudgetName`'s pattern is a validation refusal, which the
    // framework's regex engine cannot compile, so this handler answers it.
    for (case, named_budget) in [
        ("a colon", "a:b"),
        ("a backslash", "a\\b"),
        ("the /action/ substring", "a/action/b"),
    ] {
        let (status, body) =
            create_subscriber(&router, &owner, named_budget, &notification, &subscriber).await;
        assert_eq!(status, 400, "case {case:?}: expected 400, got {status} {body}");
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

/// RULE 4: "a notification can have one SNS subscriber and up to 10 email subscribers,
/// for a total of 11 subscribers" — a limit on the SET a notification ends up with, so it
/// holds when the set is grown one `CreateSubscriber` at a time. Each published per-type
/// bound is reached and accepted, the first call past either one is refused with
/// `CreationLimitExceededException`, and a refused call leaves the set as it was.
#[tokio::test]
async fn growing_a_notification_one_subscriber_at_a_time_honours_the_per_type_limits() {
    let (router, engine) = router_with_engine();
    let account_id = caller_account();
    let notification = json!({
        "NotificationType": "FORECASTED",
        "ComparisonOperator": "LESS_THAN",
        "Threshold": 50.0
    });
    let budget_name = "limits";
    seed(
        &router,
        &account_id,
        budget_name,
        std::slice::from_ref(&notification),
    )
    .await;

    // The seed subscriber is an email, so this many more reach the email bound exactly.
    let email = |n: usize| json!({"SubscriptionType": "EMAIL", "Address": format!("e{n}@example.com")});
    let sns = |n: usize| {
        json!({
            "SubscriptionType": "SNS",
            "Address": format!("arn:aws:sns:us-east-1:{}:t{n}", caller_account())
        })
    };
    let mut expected = vec![pair(&seed_subscriber())];

    for n in 1..MAX_EMAIL_SUBSCRIBERS_PER_NOTIFICATION {
        let subscriber = email(n);
        let case = format!("email subscriber {} of {MAX_EMAIL_SUBSCRIBERS_PER_NOTIFICATION}", n + 1);
        let (status, body) = create_subscriber(
            &router,
            &account_id,
            budget_name,
            &notification,
            &subscriber,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a subscriber within the bound to be accepted, \
             got {status} {body}"
        );
        expected.push(pair(&subscriber));
    }

    for n in 0..MAX_SNS_SUBSCRIBERS_PER_NOTIFICATION {
        let subscriber = sns(n);
        let case = format!("SNS subscriber {} of {MAX_SNS_SUBSCRIBERS_PER_NOTIFICATION}", n + 1);
        let (status, body) = create_subscriber(
            &router,
            &account_id,
            budget_name,
            &notification,
            &subscriber,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a subscriber within the bound to be accepted, \
             got {status} {body}"
        );
        expected.push(pair(&subscriber));
    }

    // The set is now the documented total of 11.
    let reported = reported_subscribers(&router, &account_id, budget_name, &notification).await;
    assert_eq!(
        reported.len(),
        MAX_EMAIL_SUBSCRIBERS_PER_NOTIFICATION + MAX_SNS_SUBSCRIBERS_PER_NOTIFICATION,
        "expected the full set to be {} subscribers, got {reported:?}",
        MAX_EMAIL_SUBSCRIBERS_PER_NOTIFICATION + MAX_SNS_SUBSCRIBERS_PER_NOTIFICATION
    );

    // One past either per-type bound is refused, and stores nothing.
    for (case, subscriber) in [
        ("one email past the email bound", email(usize::MAX)),
        ("one SNS topic past the SNS bound", sns(usize::MAX)),
    ] {
        let (status, body) = create_subscriber(
            &router,
            &account_id,
            budget_name,
            &notification,
            &subscriber,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the create to be refused, got {status} {body}"
        );
        assert!(
            is_error(&body, "CreationLimitExceededException"),
            "case {case:?}: expected CreationLimitExceededException, got {body}"
        );
        let held = stored_subscribers(&engine, &account_id, budget_name);
        assert_eq!(
            held[0], expected,
            "case {case:?}: expected the refused create to change nothing, got {:?}",
            held[0]
        );
    }

    // The read is unchanged by the refusals, and reports the set in creation order.
    let after = reported_subscribers(&router, &account_id, budget_name, &notification).await;
    let after_pairs: Vec<(String, String)> = after.iter().map(pair).collect();
    assert_eq!(
        after_pairs, expected,
        "expected the read to still report {expected:?}, got {after_pairs:?}"
    );
    assert!(
        same(
            &Value::Array(after),
            &Value::Array(
                reported_subscribers(&router, &account_id, budget_name, &notification).await
            )
        ),
        "expected two reads of the same notification to report the same set"
    );
}
