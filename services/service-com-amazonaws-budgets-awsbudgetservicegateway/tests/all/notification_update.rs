// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for `UpdateNotification`, driven over the wire through the same
//! `MultiServiceRouter` the production router serves from (the harness is the lifecycle
//! suite's, so every suite drives the service identically).
//!
//! The RULE every test here asserts: an update replaces ONE notification of the budget
//! that `(AccountId, BudgetName)` names — the one `OldNotification` names by value (the
//! notification type, the comparison operator, the threshold and the threshold type,
//! an absent threshold type naming the same type as `PERCENTAGE`; never the
//! service-owned `NotificationState`) — with `NewNotification`, in place: the
//! notification keeps its position among the budget's notifications and keeps the
//! subscribers held with it, because this call names neither. An `OldNotification` no
//! notification of that budget matches, or a budget this account does not hold, is
//! refused with `NotFoundException`; a `NewNotification` that names a DIFFERENT
//! notification the same budget already holds is refused with
//! `DuplicateRecordException`, since what a notification alerts on is its identity.
//!
//! Each table spans that class rather than one request: its rows differ along every
//! identity member (including each published bound of the threshold's range and one
//! past it), the alarm state, the position of the notification inside its budget, and
//! the subscriber count.
//!
//! The account gate every operation of this service shares — an `AccountId` that is not
//! the caller's own is refused with `AccessDeniedException` — is asserted for this operation
//! among all of them in `account_scoping.rs`, so every request here names the caller's
//! own account.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::time::{SystemTime, UNIX_EPOCH};

/// The enum catalogs are not re-typed here: these are the model-derived constants the
/// scaffold generated into `src/ops/types/shared_types.rs`, so a table built from them
/// cannot drift from the model.
use service_com_amazonaws_budgets_awsbudgetservicegateway::ops::types::{
    COMPARISON_OPERATOR_VALUES, NOTIFICATION_STATE_VALUES, NOTIFICATION_TYPE_VALUES,
    THRESHOLD_TYPE_VALUES,
};
use service_com_amazonaws_budgets_awsbudgetservicegateway::state::MAX_NOTIFICATIONS_PER_BUDGET;

use super::budget_lifecycle::{caller_account, call, is_error, router, same};

/// The published `@range` of `Notification$Threshold`, from
/// `smithy_query(query="shape", shape="Notification")`: `range {min: 0, max:
/// 15000000000000}`.
const THRESHOLD_MIN: f64 = 0.0;
const THRESHOLD_MAX: f64 = 15_000_000_000_000.0;

/// Seconds in a day, for the end dates the expiry table puts either side of now.
const DAY_SECS: i64 = 86_400;

/// Now, in epoch seconds — what `TimePeriod$Start` / `TimePeriod$End` are sent as.
fn now_secs() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the test host's clock is after the epoch")
            .as_secs(),
    )
    .expect("an epoch that fits in i64")
}

/// The notification set the tables below update.
///
/// Its rows differ along what a notification's identity is made of: the notification
/// type and the comparison operator (the full cross product of the model's two
/// catalogs), the threshold (each published `@range` bound of `Notification$Threshold`
/// and a fraction between them) and the threshold type (each `ThresholdType` the model
/// lists, plus absent). The service-owned alarm state varies across the rows too, so a
/// table that matches on identity cannot be matching on the alarm state by accident.
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

/// The notification `target` is updated TO: every identity member rotated off the
/// catalog row, with a threshold no catalog row uses, so the replacement is a
/// notification the budget does not already hold however the rows line up.
fn replacement(target: usize) -> Value {
    let notification_type =
        NOTIFICATION_TYPE_VALUES[(target + 1) % NOTIFICATION_TYPE_VALUES.len()];
    let comparison_operator =
        COMPARISON_OPERATOR_VALUES[(target + 1) % COMPARISON_OPERATOR_VALUES.len()];
    let mut notification = json!({
        "NotificationType": notification_type,
        "ComparisonOperator": comparison_operator,
        // 1, 2, 3 … — inside the modelled range and distinct from every catalog row's
        // threshold.
        "Threshold": 1.0 + target as f64,
    });
    // Absent on every other row, so the rule is asserted for a replacement that names
    // its threshold type and one that leaves it to mean a percentage.
    if target.is_multiple_of(2) {
        notification["ThresholdType"] = json!(THRESHOLD_TYPE_VALUES[target % THRESHOLD_TYPE_VALUES.len()]);
    }
    notification
}

/// The members an update names a notification BY, with an absent `ThresholdType`
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

/// The subscribers attached to the notification at `position`: one email subscriber,
/// and an SNS subscriber as well on every other row, so the rows differ in subscriber
/// count and type. Every address names its position, so a read can be checked for
/// exactly whose subscribers are still attached.
fn subscribers(position: usize) -> Vec<Value> {
    let mut list = vec![json!({
        "SubscriptionType": "EMAIL",
        "Address": format!("n{position}@example.com")
    })];
    if !position.is_multiple_of(2) {
        list.push(json!({
            "SubscriptionType": "SNS",
            "Address": format!("arn:aws:sns:us-east-1:000000000000:n{position}")
        }));
    }
    list
}

/// Create `budget_name` holding `catalog`, each notification with [`subscribers`], in
/// catalog order, in ONE `CreateBudget`.
///
/// A budget holds at most [`MAX_NOTIFICATIONS_PER_BUDGET`] notifications however they
/// arrive, so a catalog longer than that could not be attached to one budget at all —
/// asserted up front rather than silently refused mid-setup if the model ever grows a
/// notification type or comparison operator and widens the catalog.
async fn create_with_notifications(
    router: &MultiServiceRouter,
    account_id: &str,
    budget_name: &str,
    catalog: &[Value],
) {
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

/// One `DescribeSubscribersForNotification` call: what is still attached to the
/// notification `notification` names.
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

/// One `UpdateNotification` call.
async fn update_notification(
    router: &MultiServiceRouter,
    account_id: &str,
    budget_name: &str,
    old: &Value,
    new: &Value,
) -> (u16, Value) {
    call(
        router,
        "UpdateNotification",
        &json!({
            "AccountId": account_id,
            "BudgetName": budget_name,
            "OldNotification": old,
            "NewNotification": new
        }),
    )
    .await
}

/// The identities a budget's notifications report, in order.
async fn reported_identities(
    router: &MultiServiceRouter,
    account_id: &str,
    budget_name: &str,
) -> Vec<Value> {
    reported_notifications(router, account_id, budget_name)
        .await
        .iter()
        .map(identity)
        .collect()
}

/// Assert the budget reports exactly `expected`, in order, comparing numbers by value.
fn assert_identities(case: &str, expected: &[Value], got: &[Value]) {
    assert_eq!(
        got.len(),
        expected.len(),
        "case {case:?}: expected {} notification(s), got {got:?}",
        expected.len()
    );
    for (position, want) in expected.iter().enumerate() {
        assert!(
            same(want, &got[position]),
            "case {case:?}: expected notification {position} to be {want}, got {}",
            got[position]
        );
    }
}

/// RULE: the update replaces exactly the named notification, in place — every other
/// notification of the budget keeps its value and its position, the replacement takes
/// the old one's position, and the subscribers held with it stay attached to the
/// replacement.
///
/// The rows are each position of the catalog in turn, so the rule is asserted for the
/// first, the middle and the last notification of a budget, over notifications that
/// differ in every identity member and in subscriber count.
#[tokio::test]
async fn updating_a_notification_replaces_it_in_place_and_keeps_its_subscribers() {
    let router = router();
    let account_id = caller_account();
    let catalog = notification_catalog();

    for target in 0..catalog.len() {
        let budget_name = format!("update-{target}");
        create_with_notifications(&router, &account_id, &budget_name, &catalog).await;
        let new = replacement(target);
        let case = format!(
            "target {target} of {}: {} -> {new}",
            catalog.len(),
            catalog[target]
        );

        let (status, body) =
            update_notification(&router, &account_id, &budget_name, &catalog[target], &new).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateNotification to answer 200, got {status} {body}"
        );

        let expected: Vec<Value> = catalog
            .iter()
            .enumerate()
            .map(|(position, held)| {
                if position == target {
                    identity(&new)
                } else {
                    identity(held)
                }
            })
            .collect();
        let got = reported_identities(&router, &account_id, &budget_name).await;
        assert_identities(&case, &expected, &got);

        // The subscribers followed the notification: they are read back under the NEW
        // identity, unchanged and in order.
        let (status, body) = read_subscribers(&router, &account_id, &budget_name, &new).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the replacement's subscribers to be readable, \
             got {status} {body}"
        );
        let want_subscribers = json!(subscribers(target));
        assert!(
            same(&body["Subscribers"], &want_subscribers),
            "case {case:?}: expected subscribers {want_subscribers}, got {}",
            body["Subscribers"]
        );

        // And the old notification is no longer a notification of this budget: the read
        // of its subscribers reports the empty set that identity now has — `Subscribers`
        // is `@length(min: 1)`, so the empty set is the member left out.
        let (status, body) =
            read_subscribers(&router, &account_id, &budget_name, &catalog[target]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the replaced notification to read as an empty \
             subscriber set, got {status} {body}"
        );
        assert!(
            body.get("Subscribers").is_none_or(Value::is_null),
            "case {case:?}: expected no Subscribers member for the replaced notification, \
             got {body}"
        );
        let (status, body) =
            update_notification(&router, &account_id, &budget_name, &catalog[target], &new).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a second update of the same old notification to be \
             refused, got {status} {body}"
        );
        assert!(
            is_error(&body, "NotFoundException"),
            "case {case:?}: expected NotFoundException on the second update, got {body}"
        );
    }
}

/// RULE, the duplicate side: what a notification alerts on IS its identity, so a budget
/// cannot end up holding two of the same notification — a `NewNotification` that names
/// another notification of the same budget is refused with `DuplicateRecordException`
/// and nothing is changed. Naming the notification being updated is not a collision
/// with itself: that call is accepted and leaves the identity where it was.
///
/// The rows are every ordered pair of distinct catalog notifications, plus every
/// notification updated to itself.
#[tokio::test]
async fn an_update_onto_another_notification_of_the_budget_is_a_duplicate() {
    let router = router();
    let account_id = caller_account();
    let catalog = notification_catalog();
    let expected: Vec<Value> = catalog.iter().map(identity).collect();
    let budget_name = "duplicate";
    create_with_notifications(&router, &account_id, budget_name, &catalog).await;

    for old in 0..catalog.len() {
        for new in 0..catalog.len() {
            let case = format!("{} -> {}", catalog[old], catalog[new]);
            let (status, body) = update_notification(
                &router,
                &account_id,
                budget_name,
                &catalog[old],
                &catalog[new],
            )
            .await;
            if old == new {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected an update onto itself to be accepted, \
                     got {status} {body}"
                );
            } else {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected the duplicate to be refused, got {status} {body}"
                );
                assert!(
                    is_error(&body, "DuplicateRecordException"),
                    "case {case:?}: expected DuplicateRecordException, got {body}"
                );
            }
            // Either way the budget still holds exactly the notifications it held, in
            // the order it held them.
            let got = reported_identities(&router, &account_id, budget_name).await;
            assert_identities(&case, &expected, &got);
            // An accepted self-update also leaves the subscribers attached.
            if old == new {
                let (status, body) =
                    read_subscribers(&router, &account_id, budget_name, &catalog[old]).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the subscribers to survive a self-update, \
                     got {status} {body}"
                );
                let want_subscribers = json!(subscribers(old));
                assert!(
                    same(&body["Subscribers"], &want_subscribers),
                    "case {case:?}: expected subscribers {want_subscribers}, got {}",
                    body["Subscribers"]
                );
            }
        }
    }
}

/// RULE, the alarm state: `NotificationState` "specifies whether this notification is
/// in alarm", a member the service owns, so it is NOT part of a notification's identity
/// — whichever state `OldNotification` carries (or none), the notification whose
/// identity members match is the one updated; and the state a read reports afterwards is
/// the one the SERVICE computes for what the notification now alerts on, so neither the
/// create nor the update can set it.
///
/// The rows cross the state the notification was CREATED with, the state the update
/// NAMES on `OldNotification`, and the state it names on `NewNotification` — each over
/// every `NotificationState` the model lists plus absent. The notification under test
/// alerts above 80% and then above 90% of a budget nothing has spent against, so the
/// state the service reports for it is `OK` throughout (`GREATER_THAN` against no spend);
/// `tests/all/notification_alarm_state.rs` is where an update that moves the state is
/// asserted.
#[tokio::test]
async fn the_alarm_state_is_not_identity_and_is_the_services_own_after_an_update() {
    let router = router();
    let account_id = caller_account();
    let states: Vec<Option<&str>> = NOTIFICATION_STATE_VALUES
        .iter()
        .map(|value| Some(*value))
        .chain(std::iter::once(None))
        .collect();
    let label = |state: &Option<&str>| state.unwrap_or("absent").to_string();

    for created in &states {
        for named in &states {
            for wanted in &states {
                let budget_name = format!(
                    "alarm-{}-{}-{}",
                    label(created),
                    label(named),
                    label(wanted)
                );
                let mut stored = json!({
                    "NotificationType": "ACTUAL",
                    "ComparisonOperator": "GREATER_THAN",
                    "Threshold": 80.0
                });
                if let Some(state) = created {
                    stored["NotificationState"] = json!(state);
                }
                create_with_notifications(
                    &router,
                    &account_id,
                    &budget_name,
                    std::slice::from_ref(&stored),
                )
                .await;

                let mut old = stored.clone();
                old.as_object_mut()
                    .expect("a notification object")
                    .remove("NotificationState");
                let mut new = old.clone();
                new["Threshold"] = json!(90.0);
                if let Some(state) = named {
                    old["NotificationState"] = json!(state);
                }
                if let Some(state) = wanted {
                    new["NotificationState"] = json!(state);
                }
                let case = format!(
                    "created {:?}, named {:?}, updated to {:?}",
                    label(created),
                    label(named),
                    label(wanted)
                );

                let (status, body) =
                    update_notification(&router, &account_id, &budget_name, &old, &new).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the update to answer 200, got {status} {body}"
                );

                // The alarm state is the service's own reading of the spend behind the
                // budget: nothing is spent over the 90% this notification now alerts
                // above, so it is out of alarm whatever the three requests named.
                let expected_state = "OK";
                let reported = reported_notifications(&router, &account_id, &budget_name).await;
                assert_eq!(
                    reported.len(),
                    1,
                    "case {case:?}: expected one notification, got {reported:?}"
                );
                assert!(
                    same(&reported[0]["Threshold"], &json!(90.0)),
                    "case {case:?}: expected the threshold to be updated to 90, got {}",
                    reported[0]
                );
                assert_eq!(
                    reported[0]["NotificationState"].as_str(),
                    Some(expected_state),
                    "case {case:?}: expected NotificationState {expected_state:?}, got {}",
                    reported[0]
                );
            }
        }
    }
}

/// RULE, the not-found side: an `OldNotification` that differs from a stored one in ANY
/// identity member names no notification of that budget, and a `BudgetName` the account
/// holds no budget under names no notifications at all — both are `NotFoundException`,
/// and the budget keeps everything it held. A `BudgetName` outside its modelled
/// `@pattern` is the request-validation refusal it is, not a not-found.
#[tokio::test]
async fn an_old_notification_or_budget_that_is_not_held_is_not_found() {
    let router = router();
    let owner = caller_account();
    let stored = json!({
        "NotificationType": "ACTUAL",
        "ComparisonOperator": "GREATER_THAN",
        "Threshold": 80.0,
        "ThresholdType": "PERCENTAGE"
    });
    let new = json!({
        "NotificationType": "FORECASTED",
        "ComparisonOperator": "LESS_THAN",
        "Threshold": 25.0
    });
    let budget_name = "near-miss";
    create_with_notifications(
        &router,
        &owner,
        budget_name,
        std::slice::from_ref(&stored),
    )
    .await;

    // One row per identity member, each varying that member alone — including each
    // published bound of the threshold's modelled `@range`.
    let mut rows: Vec<(&str, Value)> = Vec::new();
    for (member, value) in [
        ("NotificationType", json!("FORECASTED")),
        ("ComparisonOperator", json!("LESS_THAN")),
        ("ComparisonOperator", json!("EQUAL_TO")),
        ("Threshold", json!(80.5)),
        ("Threshold", json!(THRESHOLD_MIN)),
        ("Threshold", json!(THRESHOLD_MAX)),
        ("ThresholdType", json!("ABSOLUTE_VALUE")),
    ] {
        let mut near_miss = stored.clone();
        near_miss[member] = value;
        rows.push((member, near_miss));
    }

    for (member, near_miss) in rows {
        let case = format!("{member} differs: {near_miss}");
        let (status, body) =
            update_notification(&router, &owner, budget_name, &near_miss, &new).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the update to be refused, got {status} {body}"
        );
        assert!(
            is_error(&body, "NotFoundException"),
            "case {case:?}: expected NotFoundException, got {body}"
        );
        let got = reported_identities(&router, &owner, budget_name).await;
        assert_identities(&case, &[identity(&stored)], &got);
    }

    for (case, name) in [
        ("a name the account holds no budget under", "absent"),
        ("the stored name with a suffix", "near-miss-2"),
        ("the stored name in another case", "NEAR-MISS"),
    ] {
        let (status, body) = update_notification(&router, &owner, name, &stored, &new).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the update to be refused, got {status} {body}"
        );
        assert!(
            is_error(&body, "NotFoundException"),
            "case {case:?}: expected NotFoundException, got {body}"
        );
    }

    // A name outside `BudgetName`'s pattern is a validation refusal, which the
    // framework's regex engine cannot compile, so this handler answers it.
    for (case, name) in [
        ("a colon", "a:b"),
        ("a backslash", "a\\b"),
        ("the /action/ substring", "a/action/b"),
    ] {
        let (status, body) = update_notification(&router, &owner, name, &stored, &new).await;
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

    // And the notification that DOES match is still updatable afterwards.
    let (status, body) = update_notification(&router, &owner, budget_name, &stored, &new).await;
    assert_eq!(
        status, 200,
        "expected the matching notification to update after the refusals, got {status} {body}"
    );
    let got = reported_identities(&router, &owner, budget_name).await;
    assert_identities("the matching notification updates", &[identity(&new)], &got);
}

/// RULE, the modelled bounds: a notification's threshold is `@range(min: 0, max:
/// 15000000000000)` (`Notification$Threshold`), so an update to either published bound
/// is accepted and one past either bound is refused at request validation, leaving the
/// notification as it was.
#[tokio::test]
async fn a_replacement_threshold_outside_the_modelled_range_is_refused() {
    let router = router();
    let account_id = caller_account();

    for (case, threshold, accepted) in [
        ("the bottom of the range", THRESHOLD_MIN, true),
        ("the top of the range", THRESHOLD_MAX, true),
        ("one below the bottom", THRESHOLD_MIN - 1.0, false),
        ("one above the top", THRESHOLD_MAX + 1.0, false),
    ] {
        let budget_name = format!("threshold-{}", threshold as i64);
        let stored = json!({
            "NotificationType": "ACTUAL",
            "ComparisonOperator": "GREATER_THAN",
            "Threshold": 80.0,
            "ThresholdType": "PERCENTAGE"
        });
        create_with_notifications(
            &router,
            &account_id,
            &budget_name,
            std::slice::from_ref(&stored),
        )
        .await;

        let mut new = stored.clone();
        new["Threshold"] = json!(threshold);
        let (status, body) =
            update_notification(&router, &account_id, &budget_name, &stored, &new).await;
        let expected = if accepted { identity(&new) } else { identity(&stored) };
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the update to answer 200, got {status} {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected the update to be refused, got {status} {body}"
            );
            assert!(
                is_error(&body, "ValidationException"),
                "case {case:?}: expected ValidationException, got {body}"
            );
        }
        let got = reported_identities(&router, &account_id, &budget_name).await;
        assert_identities(case, &[expected], &got);
    }
}

/// RULE, the clock: "After the end date, Amazon Web Services deletes the budget and all
/// the associated notifications and subscribers" (`Budget$TimePeriod`), so a
/// notification of a budget whose `TimePeriod$End` has passed is as absent to this write
/// as it is to every read — `NotFoundException` — while the same update on a budget
/// whose end date is still ahead is served and the replacement reads back.
///
/// The rows differ along where the end date sits relative to now: a year past, a day
/// past, a second past, and a year ahead.
#[tokio::test]
async fn an_update_on_a_budget_past_its_end_date_is_not_found() {
    let router = router();
    let account_id = caller_account();
    let now = now_secs();
    let stored = json!({
        "NotificationType": "ACTUAL",
        "ComparisonOperator": "GREATER_THAN",
        "Threshold": 80.0,
        "ThresholdType": "PERCENTAGE"
    });
    let new = json!({
        "NotificationType": "ACTUAL",
        "ComparisonOperator": "GREATER_THAN",
        "Threshold": 95.0,
        "ThresholdType": "PERCENTAGE"
    });

    for (case, end, expired) in [
        ("a year past", now - 365 * DAY_SECS, true),
        ("a day past", now - DAY_SECS, true),
        ("a second past", now - 1, true),
        ("a year ahead", now + 365 * DAY_SECS, false),
    ] {
        let budget_name = case.replace(' ', "-");
        let (status, body) = call(
            &router,
            "CreateBudget",
            &json!({
                "AccountId": &account_id,
                "Budget": {
                    "BudgetName": &budget_name,
                    "TimeUnit": "MONTHLY",
                    "BudgetType": "COST",
                    "BudgetLimit": {"Amount": "100", "Unit": "USD"},
                    "TimePeriod": {"Start": now - 2 * 365 * DAY_SECS, "End": end}
                },
                "NotificationsWithSubscribers": [{
                    "Notification": stored.clone(),
                    "Subscribers": subscribers(0)
                }]
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "setup {case:?}: expected CreateBudget to answer 200, got {status} {body}"
        );

        let (status, body) =
            update_notification(&router, &account_id, &budget_name, &stored, &new).await;
        if expired {
            assert_eq!(
                status, 400,
                "case {case:?}: expected the update to be refused once the end date has \
                 passed, got {status} {body}"
            );
            assert!(
                is_error(&body, "NotFoundException"),
                "case {case:?}: expected NotFoundException, got {body}"
            );
            // The budget itself is gone with everything held inside it, so the read side
            // answers the same way.
            let (status, body) = call(
                &router,
                "DescribeNotificationsForBudget",
                &json!({"AccountId": &account_id, "BudgetName": &budget_name}),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected the read to be refused too, got {status} {body}"
            );
            assert!(
                is_error(&body, "NotFoundException"),
                "case {case:?}: expected NotFoundException from the read, got {body}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 while the end date is ahead, got {status} {body}"
            );
            let got = reported_identities(&router, &account_id, &budget_name).await;
            assert_identities(case, &[identity(&new)], &got);
        }
    }
}
