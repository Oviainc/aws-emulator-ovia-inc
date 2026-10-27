// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the IDENTITY of a notification inside the SET a write attaches to
//! a budget, driven over the wire through the same `MultiServiceRouter` the production
//! router serves from (the harness is the lifecycle suite's).
//!
//! The RULE every test here asserts: a `Notification` carries no id — it is only what it
//! alerts on (`NotificationType`, `ComparisonOperator`, `Threshold`, `ThresholdType`,
//! absent standing for the percentage thresholds always are), and every operation that
//! acts on one names it by passing that whole shape back and resolves the FIRST match — so
//! a budget holds at most ONE notification per identity, and a write that would leave it
//! holding two is refused with the modelled `DuplicateRecordException` with nothing
//! persisted, however the notifications arrive.
//!
//! The sibling suite `notification_lifecycle` asserts that rule for the write that
//! attaches ONE notification (`CreateNotification`); this one asserts it for the SET
//! `CreateBudget` attaches with the budget, which is the same rule over a list, plus the
//! other half of the identity rule: entries differing in ANY identifying member are
//! notifications of their own, each addressable on its own afterwards.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// The enum catalogs are not re-typed here: these are the model-derived constants the
/// scaffold generated into `src/ops/types/shared_types.rs`, so a table built from them
/// cannot drift from the model.
use service_com_amazonaws_budgets_awsbudgetservicegateway::ops::types::{
    COMPARISON_OPERATOR_VALUES, NOTIFICATION_TYPE_VALUES, THRESHOLD_TYPE_VALUES,
};
use service_com_amazonaws_budgets_awsbudgetservicegateway::state::{
    duplicate_notification_refusal, ComAmazonawsBudgetsAwsbudgetservicegatewayState,
    MAX_EMAIL_SUBSCRIBERS_PER_NOTIFICATION, MAX_NOTIFICATIONS_PER_BUDGET,
    THRESHOLD_TYPE_PERCENTAGE,
};

use super::budget_lifecycle::{caller_account, call, is_error, router_with_engine, same};
use super::client_error_envelopes::message;

/// Every refusal this service renders over awsJson answers this status, whichever modelled
/// error it carries — the sibling suites assert the same one.
const REFUSAL_STATUS: u16 = 400;

/// An email subscriber named by `tag`: an address of the form its own `SubscriptionType`
/// names, within `Subscribers`' modelled `@length(1, 11)` when one of them is supplied.
fn email(tag: &str) -> Value {
    json!({"SubscriptionType": "EMAIL", "Address": format!("{tag}@example.com")})
}

/// `count` email subscribers, each with its own address.
fn distinct_emails(count: usize) -> Vec<Value> {
    (0..count).map(|n| email(&format!("distinct{n}"))).collect()
}

/// A notification as a caller supplies it. `threshold_type` is `None` for a request that
/// names none at all.
fn notification(
    notification_type: &str,
    operator: &str,
    threshold: Value,
    threshold_type: Option<&str>,
) -> Value {
    let mut supplied = json!({
        "NotificationType": notification_type,
        "ComparisonOperator": operator,
        "Threshold": threshold
    });
    if let Some(threshold_type) = threshold_type {
        supplied["ThresholdType"] = json!(threshold_type);
    }
    supplied
}

/// The notification every table varies from: one inside `Notification`'s documented
/// example (`ACTUAL` / `GREATER_THAN` / `PERCENTAGE` / 80).
fn base() -> Value {
    notification("ACTUAL", "GREATER_THAN", json!(80), Some("PERCENTAGE"))
}

/// A `NotificationWithSubscribers` entry: the notification, and the subscribers attached
/// with it.
fn attached(notification: &Value, subscribers: Vec<Value>) -> Value {
    json!({"Notification": notification, "Subscribers": Value::Array(subscribers)})
}

/// A `NotificationWithSubscribers` entry carrying the single subscriber `tag` names.
fn attached_to(notification: &Value, tag: &str) -> Value {
    attached(notification, vec![email(tag)])
}

/// The budget a row attaches its notifications to: every modelled constraint and
/// documented cross-member rule satisfied, so a row varies only the notifications.
fn budget(name: &str) -> Value {
    json!({
        "BudgetName": name,
        "TimeUnit": "MONTHLY",
        "BudgetType": "COST",
        "BudgetLimit": {"Amount": "100", "Unit": "USD"}
    })
}

/// One row of a table: the case it names, and the `NotificationsWithSubscribers` list the
/// create supplies.
struct Row {
    case: String,
    attaching: Vec<Value>,
}

/// The repeat table: every row names ONE notification more than once, so the budget it
/// would create would hold two notifications of one identity.
///
/// Each row stays inside the modelled `@length(0, 10)` of `NotificationWithSubscribersList`
/// and the `@length(1, 11)` of `Subscribers`, so every one of them reaches the handler
/// rather than request validation, and the rows differ along what the rule generalises
/// over: which member the repeat restates or leaves out, how many copies there are, where
/// they sit in the list, how long the list is, and what the copies carry outside the
/// identity (their subscribers, their alarm state, the spelling of the threshold).
fn repeat_rows() -> Vec<Row> {
    let base = base();
    let absolute = notification("ACTUAL", "GREATER_THAN", json!(250), Some("ABSOLUTE_VALUE"));
    let other = notification("FORECASTED", "LESS_THAN", json!(40), Some("PERCENTAGE"));

    // The repeat with no `ThresholdType` at all: "thresholds are always a percentage"
    // (`Notification$Threshold`), so it names the same notification as the explicit
    // `PERCENTAGE` of `base` (`state::THRESHOLD_TYPE_PERCENTAGE`).
    let no_threshold_type = notification("ACTUAL", "GREATER_THAN", json!(80), None);
    let percentage_named = notification(
        "ACTUAL",
        "GREATER_THAN",
        json!(80),
        Some(THRESHOLD_TYPE_PERCENTAGE),
    );
    // The alarm state is the service's own reading of the spend behind the budget, so it
    // is not part of what names a notification.
    let mut in_alarm = base.clone();
    in_alarm["NotificationState"] = json!("ALARM");

    // The whole modelled list, whose last entry restates its first.
    let mut whole_list: Vec<Value> = (1..MAX_NOTIFICATIONS_PER_BUDGET)
        .map(|n| {
            attached_to(
                &notification(
                    "ACTUAL",
                    "GREATER_THAN",
                    json!(n as f64),
                    Some("PERCENTAGE"),
                ),
                &format!("ops{n}"),
            )
        })
        .collect();
    let first_again = whole_list[0].clone();
    whole_list.push(first_again);

    vec![
        Row {
            case: "one notification twice, with the same subscriber".to_string(),
            attaching: vec![attached_to(&base, "ops"), attached_to(&base, "ops")],
        },
        Row {
            case: "one notification twice, each copy with its own subscriber".to_string(),
            attaching: vec![attached_to(&base, "a"), attached_to(&base, "b")],
        },
        Row {
            case: "a repeat that names no threshold type, the percentage the first names"
                .to_string(),
            attaching: vec![
                attached_to(&percentage_named, "a"),
                attached_to(&no_threshold_type, "b"),
            ],
        },
        Row {
            case: "a first entry with no threshold type and a repeat naming PERCENTAGE"
                .to_string(),
            attaching: vec![
                attached_to(&no_threshold_type, "a"),
                attached_to(&percentage_named, "b"),
            ],
        },
        Row {
            case: "a repeat differing only in the alarm state, which the service owns"
                .to_string(),
            attaching: vec![attached_to(&base, "a"), attached_to(&in_alarm, "b")],
        },
        Row {
            case: "a repeat whose threshold is written 80.0 rather than 80".to_string(),
            attaching: vec![
                attached_to(&base, "a"),
                attached_to(
                    &notification("ACTUAL", "GREATER_THAN", json!(80.0), Some("PERCENTAGE")),
                    "b",
                ),
            ],
        },
        Row {
            case: "a repeat separated by a notification of its own".to_string(),
            attaching: vec![
                attached_to(&base, "a"),
                attached_to(&other, "b"),
                attached_to(&base, "c"),
            ],
        },
        Row {
            case: "three copies of one notification".to_string(),
            attaching: vec![
                attached_to(&base, "a"),
                attached_to(&base, "b"),
                attached_to(&base, "c"),
            ],
        },
        Row {
            case: "a repeat of an ABSOLUTE_VALUE notification".to_string(),
            attaching: vec![attached_to(&absolute, "a"), attached_to(&absolute, "b")],
        },
        Row {
            case: format!(
                "two copies of one notification, each carrying the documented maximum of {} \
                 email subscribers",
                MAX_EMAIL_SUBSCRIBERS_PER_NOTIFICATION
            ),
            attaching: vec![
                attached(
                    &base,
                    distinct_emails(MAX_EMAIL_SUBSCRIBERS_PER_NOTIFICATION),
                ),
                attached(
                    &base,
                    distinct_emails(MAX_EMAIL_SUBSCRIBERS_PER_NOTIFICATION),
                ),
            ],
        },
        Row {
            case: format!(
                "the whole modelled list of {MAX_NOTIFICATIONS_PER_BUDGET}, its last entry \
                 restating its first"
            ),
            attaching: whole_list,
        },
    ]
}

/// The distinct table: every row's entries differ in at least one identifying member, so
/// the budget it creates holds each of them as a notification of its own.
///
/// The rows are built from the model-derived catalogs of the identifying members, so each
/// documented value of `NotificationType`, `ComparisonOperator` and `ThresholdType` is a
/// row's point of difference, beside a row that differs only in the threshold, the empty
/// list and a row holding the documented maximum of notifications a budget can have.
fn distinct_rows() -> Vec<Row> {
    let mut rows = vec![Row {
        case: "no notifications attached at all".to_string(),
        attaching: Vec::new(),
    }];

    rows.push(Row {
        case: "one notification per notification type the model lists".to_string(),
        attaching: NOTIFICATION_TYPE_VALUES
            .iter()
            .enumerate()
            .map(|(n, notification_type)| {
                attached_to(
                    &notification(
                        notification_type,
                        "GREATER_THAN",
                        json!(80),
                        Some("PERCENTAGE"),
                    ),
                    &format!("type{n}"),
                )
            })
            .collect(),
    });
    rows.push(Row {
        case: "one notification per comparison operator the model lists".to_string(),
        attaching: COMPARISON_OPERATOR_VALUES
            .iter()
            .enumerate()
            .map(|(n, operator)| {
                attached_to(
                    &notification("ACTUAL", operator, json!(80), Some("PERCENTAGE")),
                    &format!("operator{n}"),
                )
            })
            .collect(),
    });
    rows.push(Row {
        case: "one notification per threshold type the model lists".to_string(),
        attaching: THRESHOLD_TYPE_VALUES
            .iter()
            .enumerate()
            .map(|(n, threshold_type)| {
                attached_to(
                    &notification("ACTUAL", "GREATER_THAN", json!(80), Some(threshold_type)),
                    &format!("threshold-type{n}"),
                )
            })
            .collect(),
    });
    rows.push(Row {
        case: "two notifications differing only in the threshold".to_string(),
        attaching: vec![
            attached_to(&base(), "low"),
            attached_to(
                &notification("ACTUAL", "GREATER_THAN", json!(81), Some("PERCENTAGE")),
                "high",
            ),
        ],
    });
    rows.push(Row {
        case: format!(
            "the documented maximum of {MAX_NOTIFICATIONS_PER_BUDGET} notifications, all \
             distinct"
        ),
        attaching: (0..MAX_NOTIFICATIONS_PER_BUDGET)
            .map(|n| {
                attached_to(
                    &notification(
                        "ACTUAL",
                        "GREATER_THAN",
                        json!(n as f64 + 1.0),
                        Some("PERCENTAGE"),
                    ),
                    &format!("ops{n}"),
                )
            })
            .collect(),
    });
    rows
}

/// Whether the store holds a budget under `(account_id, budget_name)` — read from the state
/// struct directly, so "nothing was persisted" is asserted independently of any read
/// operation's projection.
fn store_holds_budget(
    engine: &Arc<engine::InMemoryEngine>,
    account_id: &str,
    budget_name: &str,
) -> bool {
    let state = engine.state::<ComAmazonawsBudgetsAwsbudgetservicegatewayState>();
    let store = state.lock();
    store
        .budgets
        .contains_key(&(account_id.to_string(), budget_name.to_string()))
}

/// The notifications `DescribeNotificationsForBudget` reports for a budget. `MaxResults` is
/// the documented per-budget maximum, so one page carries every one of them.
async fn reported_notifications(
    router: &MultiServiceRouter,
    account_id: &str,
    budget_name: &str,
) -> (u16, Vec<Value>) {
    let (status, body) = call(
        router,
        "DescribeNotificationsForBudget",
        &json!({
            "AccountId": account_id,
            "BudgetName": budget_name,
            "MaxResults": MAX_NOTIFICATIONS_PER_BUDGET
        }),
    )
    .await;
    let reported = body["Notifications"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    (status, reported)
}

/// Whether `reported` names the same notification as `wanted`: the identifying members
/// only, with an absent `ThresholdType` read as the percentage thresholds always are.
fn names_same_notification(reported: &Value, wanted: &Value) -> bool {
    fn threshold_type(notification: &Value) -> String {
        notification["ThresholdType"]
            .as_str()
            .unwrap_or(THRESHOLD_TYPE_PERCENTAGE)
            .to_string()
    }
    ["NotificationType", "ComparisonOperator", "Threshold"]
        .iter()
        .all(|member| same(&reported[*member], &wanted[*member]))
        && threshold_type(reported) == threshold_type(wanted)
}

/// RULE: the set `CreateBudget` attaches is held to the notification-identity rule, so a
/// list naming one notification twice is refused with the modelled
/// `DuplicateRecordException` — the refusal the single-notification write path answers for
/// the same repeat — and nothing is persisted: no budget, and no notification of it.
#[tokio::test]
async fn create_budget_refuses_a_repeated_notification() {
    let (router, engine) = router_with_engine();
    let account_id = caller_account();

    for (index, row) in repeat_rows().into_iter().enumerate() {
        let case = &row.case;
        // One budget name per row, so a row can only ever see its own state.
        let budget_name = format!("repeated-notification-{index}");
        let (status, body) = call(
            &router,
            "CreateBudget",
            &json!({
                "AccountId": &account_id,
                "Budget": budget(&budget_name),
                "NotificationsWithSubscribers": Value::Array(row.attaching.clone())
            }),
        )
        .await;
        assert_eq!(
            status, REFUSAL_STATUS,
            "case {case:?}: expected a repeated notification to be refused at \
             {REFUSAL_STATUS}, got {status} {body}"
        );
        assert!(
            is_error(&body, "DuplicateRecordException"),
            "case {case:?}: expected DuplicateRecordException, got {body}"
        );
        let expected = duplicate_notification_refusal(&budget_name);
        let answered = message(&body);
        assert_eq!(
            answered, expected,
            "case {case:?}: expected the message {expected:?}, got {answered:?}"
        );

        // Nothing is persisted by a refused create: the budget itself is not stored, so
        // neither is any notification of it.
        assert!(
            !store_holds_budget(&engine, &account_id, &budget_name),
            "case {case:?}: expected a refused create to store no budget, \
             got one under {budget_name:?}"
        );
        let (status, body) = call(
            &router,
            "DescribeBudget",
            &json!({"AccountId": &account_id, "BudgetName": &budget_name}),
        )
        .await;
        assert_eq!(
            status, REFUSAL_STATUS,
            "case {case:?}: expected the budget not to exist, got {status} {body}"
        );
        assert!(
            is_error(&body, "NotFoundException"),
            "case {case:?}: expected NotFoundException for a budget nothing created, \
             got {body}"
        );
    }
}

/// RULE, the same one from the other side: the budget's own write path and the set a
/// create attaches refuse the repeat alike, so a notification that `CreateBudget` would
/// not attach twice is also one `CreateNotification` will not add beside itself — same
/// error, same message.
#[tokio::test]
async fn both_write_paths_refuse_the_same_repeat_alike() {
    let (router, _engine) = router_with_engine();
    let account_id = caller_account();

    for (index, row) in repeat_rows().into_iter().enumerate() {
        let case = &row.case;
        let budget_name = format!("same-refusal-{index}");
        // The repeat is whatever entry of the row names a notification an earlier entry
        // already named; the first entry is the one it repeats for every row here, except
        // where an earlier entry differs — so the budget is seeded with the WHOLE list up
        // to the repeat, one `CreateNotification` at a time, and the repeat is the entry
        // that refusal must name.
        let (status, body) = call(
            &router,
            "CreateBudget",
            &json!({"AccountId": &account_id, "Budget": budget(&budget_name)}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the budget to be created without notifications, \
             got {status} {body}"
        );

        let mut refusal: Option<Value> = None;
        for entry in &row.attaching {
            let (status, body) = call(
                &router,
                "CreateNotification",
                &json!({
                    "AccountId": &account_id,
                    "BudgetName": &budget_name,
                    "Notification": &entry["Notification"],
                    "Subscribers": &entry["Subscribers"]
                }),
            )
            .await;
            if status != 200 {
                refusal = Some(body);
                break;
            }
        }
        let refusal = refusal.unwrap_or_else(|| {
            panic!("case {case:?}: expected one entry of the list to be refused on its own")
        });
        assert!(
            is_error(&refusal, "DuplicateRecordException"),
            "case {case:?}: expected the single-notification path to answer \
             DuplicateRecordException, got {refusal}"
        );
        let expected = duplicate_notification_refusal(&budget_name);
        let answered = message(&refusal);
        assert_eq!(
            answered, expected,
            "case {case:?}: expected the message {expected:?}, got {answered:?}"
        );
    }
}

/// RULE, the other half: entries differing in ANY identifying member are notifications of
/// their own, so the budget holds every one of them, each reachable by what it alerts on —
/// its own subscribers readable, and a delete of one leaving it gone and the rest held.
#[tokio::test]
async fn create_budget_holds_every_distinct_notification_addressably() {
    let (router, engine) = router_with_engine();
    let account_id = caller_account();

    for (index, row) in distinct_rows().into_iter().enumerate() {
        let case = &row.case;
        let budget_name = format!("distinct-notifications-{index}");
        let (status, body) = call(
            &router,
            "CreateBudget",
            &json!({
                "AccountId": &account_id,
                "Budget": budget(&budget_name),
                "NotificationsWithSubscribers": Value::Array(row.attaching.clone())
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected distinct notifications to be accepted, got {status} {body}"
        );
        assert!(
            store_holds_budget(&engine, &account_id, &budget_name),
            "case {case:?}: expected the budget to be stored, got none under {budget_name:?}"
        );

        let (status, reported) = reported_notifications(&router, &account_id, &budget_name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the notifications to read back, got {status} {reported:?}"
        );
        assert_eq!(
            reported.len(),
            row.attaching.len(),
            "case {case:?}: expected {} notifications to be reported, got {reported:?}",
            row.attaching.len()
        );

        for entry in &row.attaching {
            let wanted = &entry["Notification"];
            let matches = reported
                .iter()
                .filter(|found| names_same_notification(found, wanted))
                .count();
            assert_eq!(
                matches, 1,
                "case {case:?}: expected {wanted} to be reported exactly once, \
                 got {matches} of it in {reported:?}"
            );

            // Each entry's own subscribers are reachable through the notification it was
            // attached with: a set stored where a read cannot reach it would report the
            // wrong entry's subscribers here.
            let (status, body) = call(
                &router,
                "DescribeSubscribersForNotification",
                &json!({
                    "AccountId": &account_id,
                    "BudgetName": &budget_name,
                    "Notification": wanted,
                    "MaxResults": 100
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the subscribers of {wanted} to read back, \
                 got {status} {body}"
            );
            assert!(
                same(&body["Subscribers"], &entry["Subscribers"]),
                "case {case:?}: expected {} for {wanted}, got {}",
                entry["Subscribers"],
                body["Subscribers"]
            );
        }

        // A delete names a notification the same way, so deleting one of them leaves it
        // gone — not a second copy of it behind.
        if let Some(entry) = row.attaching.first() {
            let wanted = &entry["Notification"];
            let (status, body) = call(
                &router,
                "DeleteNotification",
                &json!({
                    "AccountId": &account_id,
                    "BudgetName": &budget_name,
                    "Notification": wanted
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected {wanted} to be deleted, got {status} {body}"
            );
            let (_, reported) = reported_notifications(&router, &account_id, &budget_name).await;
            let left = reported
                .iter()
                .filter(|found| names_same_notification(found, wanted))
                .count();
            assert_eq!(
                left, 0,
                "case {case:?}: expected {wanted} to be gone after the delete, \
                 got {left} of it in {reported:?}"
            );
            assert_eq!(
                reported.len(),
                row.attaching.len() - 1,
                "case {case:?}: expected the other notifications to be held, got {reported:?}"
            );
        }
    }
}
