// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the part a notification's `Threshold` plays in its IDENTITY,
//! driven over the wire through the same `MultiServiceRouter` the production router
//! serves from (the harness is the lifecycle suite's).
//!
//! The RULE every test here asserts: a `Notification` carries no id, so it is named by
//! what it alerts on and the `Threshold` is one of those members — and a notification
//! names ITSELF for every value that member admits. `Notification$Threshold` is a
//! modelled `double` (`smithy_query(query="shape",shape="NotificationThreshold")`), and
//! awsJson spells the non-finite doubles as the strings `"NaN"`, `"Infinity"` and
//! `"-Infinity"`, so a threshold that is not a number is a value a caller can send and
//! one that IEEE equality matches against nothing, itself included. Two notifications
//! are therefore the same notification exactly when their thresholds are the same
//! VALUE: a repeat of one a budget holds is refused as the duplicate record whatever its
//! threshold, two different thresholds are two notifications each addressable on its
//! own, and a threshold outside the modelled range never reaches the store at all.
//!
//! The sibling suites `notification_lifecycle`, `notification_update` and
//! `attached_notification_identity` assert that identity over the other three members
//! with a finite threshold; this one spans the threshold's own value domain.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_budgets_awsbudgetservicegateway::state::{
    duplicate_notification_refusal, MAX_NOTIFICATIONS_PER_BUDGET,
};

use super::budget_lifecycle::{caller_account, call, is_error, router, same};
use super::client_error_envelopes::message;

/// Every refusal of a request this service renders answers this status, whichever
/// modelled error it carries (`tests/all/client_error_envelopes.rs`).
const REFUSAL_STATUS: u16 = 400;

/// The modelled bounds of a notification's threshold:
/// `smithy_query(query="shape",shape="NotificationThreshold")` gives a `double` with
/// `@range(min: 0, max: 15000000000000)` and no other constraint.
const THRESHOLD_MIN: f64 = 0.0;
const THRESHOLD_MAX: f64 = 15_000_000_000_000.0;

/// The awsJson spelling of a double that is not a number — a string, which is how the
/// protocol carries every non-finite double.
fn not_a_number() -> Value {
    json!("NaN")
}

/// An email subscriber named by `tag`: an address of the form its own `SubscriptionType`
/// names.
fn email(tag: &str) -> Value {
    json!({"SubscriptionType": "EMAIL", "Address": format!("{tag}@example.com")})
}

/// A notification as a caller supplies it. `threshold_type` is `None` for a request that
/// names none at all, and `threshold` is the raw wire value, so a row can send a number
/// or the string form of a non-finite one.
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

/// A notification inside `Notification`'s documented example (`ACTUAL` / `GREATER_THAN`
/// / `PERCENTAGE`), carrying `threshold`.
fn example_with(threshold: Value) -> Value {
    notification("ACTUAL", "GREATER_THAN", threshold, Some("PERCENTAGE"))
}

/// The budget a row attaches its notifications to: every modelled constraint and
/// documented cross-member rule satisfied, so a row varies only the thresholds.
fn budget(name: &str) -> Value {
    json!({
        "BudgetName": name,
        "TimeUnit": "MONTHLY",
        "BudgetType": "COST",
        "BudgetLimit": {"Amount": "100", "Unit": "USD"}
    })
}

/// Create the budget a row works in, asserting the setup succeeded.
async fn seed_budget(
    router: &MultiServiceRouter,
    account_id: &str,
    budget_name: &str,
    case: &str,
) {
    let (status, body) = call(
        router,
        "CreateBudget",
        &json!({"AccountId": account_id, "Budget": budget(budget_name)}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the budget {budget_name:?} to be created, got {status} {body}"
    );
}

/// `CreateNotification` for one notification and one subscriber.
async fn create_notification(
    router: &MultiServiceRouter,
    account_id: &str,
    budget_name: &str,
    wanted: &Value,
    subscribers: &Value,
) -> (u16, Value) {
    call(
        router,
        "CreateNotification",
        &json!({
            "AccountId": account_id,
            "BudgetName": budget_name,
            "Notification": wanted,
            "Subscribers": subscribers
        }),
    )
    .await
}

/// The notifications `DescribeNotificationsForBudget` reports for a budget. `MaxResults`
/// is the documented per-budget maximum, so one page carries every one of them.
async fn reported_notifications(
    router: &MultiServiceRouter,
    account_id: &str,
    budget_name: &str,
) -> Vec<Value> {
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
    assert_eq!(
        status, 200,
        "expected the notifications of {budget_name:?} to read back, got {status} {body}"
    );
    body["Notifications"].as_array().cloned().unwrap_or_default()
}

/// `DescribeSubscribersForNotification` for the notification `wanted` names.
async fn subscribers_of(
    router: &MultiServiceRouter,
    account_id: &str,
    budget_name: &str,
    wanted: &Value,
) -> (u16, Value) {
    call(
        router,
        "DescribeSubscribersForNotification",
        &json!({
            "AccountId": account_id,
            "BudgetName": budget_name,
            "Notification": wanted,
            "MaxResults": 100
        }),
    )
    .await
}

/// One row of the identity table: the notification it stores, and the one an update
/// moves it to — a notification of its own, differing in the threshold alone.
struct Row {
    case: String,
    notification: Value,
    moved_to: Value,
}

/// A row whose notification is the documented example carrying `threshold`, moved to the
/// same example carrying `moved_to`.
fn example_row(case: &str, threshold: Value, moved_to: Value) -> Row {
    Row {
        case: case.to_string(),
        notification: example_with(threshold),
        moved_to: example_with(moved_to),
    }
}

/// The identity table. The rows differ along what the rule generalises over: the value
/// the threshold carries (a number that is not a number at all, both spellings of zero,
/// each modelled bound, a fractional value, an integral one and the same value written
/// with a decimal point) and, for the non-finite value, the other identifying members it
/// is carried by (the threshold type it is read through, including none at all, the
/// notification type and the comparison).
fn identity_rows() -> Vec<Row> {
    vec![
        example_row(
            "the documented example threshold, written as an integer",
            json!(80),
            json!(81),
        ),
        example_row(
            "the documented example threshold, written with a decimal point",
            json!(80.0),
            json!(81.5),
        ),
        example_row(
            "a threshold that is not a number, in the awsJson string form",
            not_a_number(),
            json!(80),
        ),
        example_row(
            "the modelled minimum threshold",
            json!(THRESHOLD_MIN),
            json!(1),
        ),
        example_row(
            "zero written with a sign, the other spelling of the minimum",
            json!(-0.0),
            json!(1),
        ),
        example_row(
            "the modelled maximum threshold",
            json!(THRESHOLD_MAX),
            json!(THRESHOLD_MAX - 1.0),
        ),
        example_row("a fractional threshold", json!(12.5), json!(12.75)),
        Row {
            case: "a not-a-number threshold read as an absolute value".to_string(),
            notification: notification(
                "ACTUAL",
                "GREATER_THAN",
                not_a_number(),
                Some("ABSOLUTE_VALUE"),
            ),
            moved_to: notification("ACTUAL", "GREATER_THAN", json!(250), Some("ABSOLUTE_VALUE")),
        },
        Row {
            case: "a not-a-number threshold on a notification naming no threshold type"
                .to_string(),
            notification: notification("ACTUAL", "GREATER_THAN", not_a_number(), None),
            moved_to: notification("ACTUAL", "GREATER_THAN", json!(80), None),
        },
        Row {
            case: "a not-a-number threshold of a FORECASTED notification compared the other way"
                .to_string(),
            notification: notification("FORECASTED", "LESS_THAN", not_a_number(), Some("PERCENTAGE")),
            moved_to: notification("FORECASTED", "LESS_THAN", json!(40), Some("PERCENTAGE")),
        },
    ]
}

/// RULE: a notification names itself for every value its threshold admits, so a budget
/// holds exactly one of it — the repeat is the modelled `DuplicateRecordException` — and
/// every operation that names it by value resolves it: its subscribers read back, a
/// subscriber can be added to, updated on and removed from it, an update moves it to
/// another threshold (after which the old value names nothing), and a delete leaves it
/// gone.
#[tokio::test]
async fn a_notification_names_itself_whatever_its_threshold() {
    let router = router();
    let account_id = caller_account();

    for (index, row) in identity_rows().into_iter().enumerate() {
        let case = &row.case;
        // One budget per row, so a row can only ever see its own state.
        let budget_name = format!("threshold-identity-{index}");
        seed_budget(&router, &account_id, &budget_name, case).await;

        let held = email("ops");
        let subscribers = json!([held.clone()]);
        let (status, body) = create_notification(
            &router,
            &account_id,
            &budget_name,
            &row.notification,
            &subscribers,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the notification to be created, got {status} {body}"
        );

        // The same request again names the notification the budget now holds, so it is
        // the duplicate record rather than a second copy.
        let (status, body) = create_notification(
            &router,
            &account_id,
            &budget_name,
            &row.notification,
            &subscribers,
        )
        .await;
        assert_eq!(
            status, REFUSAL_STATUS,
            "case {case:?}: expected the repeat to be refused at {REFUSAL_STATUS}, \
             got {status} {body}"
        );
        assert!(
            is_error(&body, "DuplicateRecordException"),
            "case {case:?}: expected DuplicateRecordException for the repeat, got {body}"
        );
        let expected = duplicate_notification_refusal(&budget_name);
        let answered = message(&body);
        assert_eq!(
            answered, expected,
            "case {case:?}: expected the message {expected:?}, got {answered:?}"
        );

        // One notification is held, reported with the threshold it was given.
        let reported = reported_notifications(&router, &account_id, &budget_name).await;
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: expected exactly one notification to be held, got {reported:?}"
        );
        assert!(
            same(&reported[0]["Threshold"], &row.notification["Threshold"]),
            "case {case:?}: expected the threshold {} to read back, got {}",
            row.notification["Threshold"],
            reported[0]["Threshold"]
        );

        // Reachable by value: the subscribers stored with it read back.
        let (status, body) =
            subscribers_of(&router, &account_id, &budget_name, &row.notification).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the subscribers of the notification to read back, \
             got {status} {body}"
        );
        assert!(
            same(&body["Subscribers"], &subscribers),
            "case {case:?}: expected {subscribers} to read back, got {}",
            body["Subscribers"]
        );

        // The subscriber writes name the notification the same way: one added to it,
        // updated on it, and removed from it again.
        let added = email("oncall");
        let (status, body) = call(
            &router,
            "CreateSubscriber",
            &json!({
                "AccountId": &account_id,
                "BudgetName": &budget_name,
                "Notification": &row.notification,
                "Subscriber": &added
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a subscriber to be added to the notification, \
             got {status} {body}"
        );
        let renamed = email("duty");
        let (status, body) = call(
            &router,
            "UpdateSubscriber",
            &json!({
                "AccountId": &account_id,
                "BudgetName": &budget_name,
                "Notification": &row.notification,
                "OldSubscriber": &added,
                "NewSubscriber": &renamed
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the added subscriber to be updated, got {status} {body}"
        );
        let (status, body) = call(
            &router,
            "DeleteSubscriber",
            &json!({
                "AccountId": &account_id,
                "BudgetName": &budget_name,
                "Notification": &row.notification,
                "Subscriber": &renamed
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the updated subscriber to be deleted, got {status} {body}"
        );
        let (_, body) = subscribers_of(&router, &account_id, &budget_name, &row.notification).await;
        assert!(
            same(&body["Subscribers"], &subscribers),
            "case {case:?}: expected the notification to be left with {subscribers}, got {}",
            body["Subscribers"]
        );

        // An update resolves it too, and moves it to the threshold the row names: the
        // notification is then that one, and the threshold it was created with names
        // nothing the budget holds.
        let (status, body) = call(
            &router,
            "UpdateNotification",
            &json!({
                "AccountId": &account_id,
                "BudgetName": &budget_name,
                "OldNotification": &row.notification,
                "NewNotification": &row.moved_to
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the notification to be updated, got {status} {body}"
        );
        let (status, body) =
            subscribers_of(&router, &account_id, &budget_name, &row.notification).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the threshold it was moved off to be readable, \
             got {status} {body}"
        );
        assert!(
            body.get("Subscribers").is_none_or(Value::is_null),
            "case {case:?}: expected the threshold it left to hold no subscribers, got {body}"
        );
        let (status, body) =
            subscribers_of(&router, &account_id, &budget_name, &row.moved_to).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the moved notification to keep its subscribers, \
             got {status} {body}"
        );
        assert!(
            same(&body["Subscribers"], &subscribers),
            "case {case:?}: expected {subscribers} under the new threshold, got {}",
            body["Subscribers"]
        );

        // Moved back, so the delete below names the value this row is about.
        let (status, body) = call(
            &router,
            "UpdateNotification",
            &json!({
                "AccountId": &account_id,
                "BudgetName": &budget_name,
                "OldNotification": &row.moved_to,
                "NewNotification": &row.notification
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the notification to be moved back, got {status} {body}"
        );

        let (status, body) = call(
            &router,
            "DeleteNotification",
            &json!({
                "AccountId": &account_id,
                "BudgetName": &budget_name,
                "Notification": &row.notification
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the notification to be deleted, got {status} {body}"
        );
        let reported = reported_notifications(&router, &account_id, &budget_name).await;
        assert!(
            reported.is_empty(),
            "case {case:?}: expected the delete to leave no notification, got {reported:?}"
        );
    }
}

/// RULE, the other half: thresholds that are DIFFERENT values are different
/// notifications, so a budget holds each of them beside the other, each addressable on
/// its own and each deletable without the other.
#[tokio::test]
async fn thresholds_that_differ_are_notifications_of_their_own() {
    let router = router();
    let account_id = caller_account();

    // (case, one threshold, another) — the pairs differ along the same value domain the
    // identity table spans, each pair naming two values that are not the same value.
    let pairs: Vec<(&str, Value, Value)> = vec![
        (
            "a threshold that is not a number beside the documented example",
            not_a_number(),
            json!(80),
        ),
        (
            "a threshold that is not a number beside the modelled minimum",
            not_a_number(),
            json!(THRESHOLD_MIN),
        ),
        ("two integral thresholds one apart", json!(80), json!(81)),
        (
            "the two modelled bounds",
            json!(THRESHOLD_MIN),
            json!(THRESHOLD_MAX),
        ),
        (
            "a fractional threshold and its neighbour",
            json!(12.5),
            json!(12.75),
        ),
    ];

    for (index, (case, left, right)) in pairs.into_iter().enumerate() {
        let budget_name = format!("distinct-thresholds-{index}");
        seed_budget(&router, &account_id, &budget_name, case).await;

        let first = example_with(left);
        let second = example_with(right);
        let first_subscribers = json!([email("first")]);
        let second_subscribers = json!([email("second")]);

        for (wanted, subscribers) in [
            (&first, &first_subscribers),
            (&second, &second_subscribers),
        ] {
            let (status, body) =
                create_notification(&router, &account_id, &budget_name, wanted, subscribers).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected {wanted} to be a notification of its own, \
                 got {status} {body}"
            );
        }

        let reported = reported_notifications(&router, &account_id, &budget_name).await;
        assert_eq!(
            reported.len(),
            2,
            "case {case:?}: expected both notifications to be held, got {reported:?}"
        );

        // Each resolves to the subscribers it was created with, so neither read is
        // answered out of the other's entry.
        for (wanted, subscribers) in [
            (&first, &first_subscribers),
            (&second, &second_subscribers),
        ] {
            let (status, body) = subscribers_of(&router, &account_id, &budget_name, wanted).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the subscribers of {wanted} to read back, \
                 got {status} {body}"
            );
            assert!(
                same(&body["Subscribers"], subscribers),
                "case {case:?}: expected {subscribers} for {wanted}, got {}",
                body["Subscribers"]
            );
        }

        // Deleting one leaves the other held and still reachable.
        let (status, body) = call(
            &router,
            "DeleteNotification",
            &json!({
                "AccountId": &account_id,
                "BudgetName": &budget_name,
                "Notification": &first
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {first} to be deleted, got {status} {body}"
        );
        let reported = reported_notifications(&router, &account_id, &budget_name).await;
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: expected only the other notification to be left, got {reported:?}"
        );
        let (status, body) = subscribers_of(&router, &account_id, &budget_name, &second).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {second} to be reachable after the delete, \
             got {status} {body}"
        );
        assert!(
            same(&body["Subscribers"], &second_subscribers),
            "case {case:?}: expected {second_subscribers} for {second}, got {}",
            body["Subscribers"]
        );
    }
}

/// RULE: a threshold outside the modelled `@range(0, 15000000000000)` is refused at
/// request validation, so no notification is stored under a value no bound admits — the
/// non-finite thresholds the range CAN order (the infinities) among them.
///
/// The rows sit one step past each published bound, on the two infinities, and on each
/// bound itself, which is admitted.
#[tokio::test]
async fn a_threshold_outside_the_modelled_range_is_refused() {
    let router = router();
    let account_id = caller_account();
    let budget_name = "threshold-range";
    seed_budget(&router, &account_id, budget_name, "the range table").await;

    // (case, threshold, whether the range admits it)
    let rows: Vec<(&str, Value, bool)> = vec![
        ("the minimum itself", json!(THRESHOLD_MIN), true),
        ("the maximum itself", json!(THRESHOLD_MAX), true),
        ("one past the minimum", json!(-1), false),
        ("just past the minimum", json!(-0.5), false),
        ("one past the maximum", json!(THRESHOLD_MAX + 1.0), false),
        ("the positive infinity", json!("Infinity"), false),
        ("the negative infinity", json!("-Infinity"), false),
    ];

    for (case, threshold, admitted) in rows {
        let wanted = example_with(threshold);
        let (status, body) = create_notification(
            &router,
            &account_id,
            budget_name,
            &wanted,
            &json!([email("ops")]),
        )
        .await;
        if admitted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected a threshold on the bound to be accepted, \
                 got {status} {body}"
            );
            // Taken back out, so the rows that follow start from the same budget.
            let (status, body) = call(
                &router,
                "DeleteNotification",
                &json!({
                    "AccountId": &account_id,
                    "BudgetName": budget_name,
                    "Notification": &wanted
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the accepted notification to be deleted, \
                 got {status} {body}"
            );
            continue;
        }
        assert_eq!(
            status, REFUSAL_STATUS,
            "case {case:?}: expected the refusal at {REFUSAL_STATUS}, got {status} {body}"
        );
        assert!(
            is_error(&body, "ValidationException"),
            "case {case:?}: expected ValidationException, got {body}"
        );
        let reported = reported_notifications(&router, &account_id, budget_name).await;
        assert!(
            reported.is_empty(),
            "case {case:?}: expected a refused threshold to store no notification, \
             got {reported:?}"
        );
    }
}
