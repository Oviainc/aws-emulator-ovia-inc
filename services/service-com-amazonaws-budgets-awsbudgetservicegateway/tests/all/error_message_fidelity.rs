// SPDX-License-Identifier: CC0-1.0

//! The WORDING and the CLASS of this service's refusals — the half of a refusal a client
//! can read.
//!
//! Budgets answers every missing resource with one error code (`NotFoundException`, "We
//! can't locate the resource that you specified"), so the message is the only thing that
//! tells a missing budget from a missing budget action, and a state conflict is told from
//! a bad input value by the error SHAPE. Four rules, each asserted over a table that
//! spans its class rather than over the one request that exposed it:
//!
//! 1. A budget this account does not hold is refused with ONE sentence, whichever
//!    operation addressed it, with the budget's name as the request spelled it.
//! 2. A budget action this account does not hold is refused with ONE sentence of its
//!    own, naming the account and the action id from the request.
//! 3. A method a budget action's CURRENT STATUS does not allow is refused as a conflict
//!    with that state — `ResourceLockedException`, naming only the status the action
//!    carries.
//! 4. The two cross-member rules a budget's amount and period carry are refused with
//!    their own fixed sentences, and the notification quota with its own.
//!
//! The harness (`router`, `call`, `is_error`, `caller_account`, `message`) and the
//! budget/action setup are the sibling suites', so every suite in this target drives the
//! service identically.

use serde_json::{json, Value};

/// The catalogs and the quota are the model-derived / service constants, so a table built
/// from them cannot drift from the code under test.
use service_com_amazonaws_budgets_awsbudgetservicegateway::ops::types::{
    ACTION_STATUS_VALUES, EXECUTION_TYPE_VALUES,
};
use service_com_amazonaws_budgets_awsbudgetservicegateway::state::MAX_NOTIFICATIONS_PER_BUDGET;

use super::budget_action_execution::{
    create_body, email_subscriber, execute_action, execution_role_arn, iam_definition, seed_action,
    seed_budget, seed_status, ALLOWED_EXECUTIONS, IN_PROGRESS_STATUSES,
};
use super::budget_lifecycle::{caller_account, call, civil_secs, is_error, router, router_with_engine};
use super::client_error_envelopes::message;
use super::resource_tags::{action_arn, budget_arn, tag_list};

/// A budget that satisfies every modelled constraint and every documented cross-member
/// rule, so a row varies only what it means to vary.
fn budget(name: &str) -> Value {
    json!({
        "BudgetName": name,
        "TimeUnit": "MONTHLY",
        "BudgetType": "COST",
        "BudgetLimit": {"Amount": "100", "Unit": "USD"}
    })
}

/// A `Notification` — all four members, which are its whole identity.
fn notification() -> Value {
    json!({
        "NotificationType": "ACTUAL",
        "ComparisonOperator": "GREATER_THAN",
        "Threshold": 80.0,
        "ThresholdType": "PERCENTAGE"
    })
}

/// A well-formed `ActionId` that no action carries: the modelled pattern is satisfied, so
/// the request reaches the handler rather than the validation layer in front of it.
const ABSENT_ACTION_IDS: [&str; 3] = [
    "00000000-0000-4000-8000-000000000000",
    "0a1b2c3d-4e5f-6071-8293-a4b5c6d7e8f9",
    "ABCDEF01-2345-6789-ABCD-EF0123456789",
];

/// The budget names a row addresses an absent budget by. They differ along what the
/// not-found rule generalises over — the form and the length of the name — and each one
/// satisfies `BudgetName`'s modelled `@length(1, 100)` and `@pattern`.
fn absent_names() -> Vec<String> {
    vec![
        "no-such-budget".to_string(),
        "a budget with spaces".to_string(),
        "予算-2026".to_string(),
        "x".to_string(),
        "n".repeat(100),
    ]
}

/// Every operation that addresses a budget BY NAME, each with a request that satisfies
/// every modelled constraint so that it reaches the handler: the request's only fault is
/// the budget it names.
///
/// The ARN-addressed tagging operations are here too — a `ResourceARN` of the form
/// `…:budget/{BudgetName}` addresses the same budget by the same name.
fn requests_naming_a_budget(account_id: &str, name: &str) -> Vec<(&'static str, Value)> {
    let subscriber = email_subscriber(0);
    vec![
        (
            "DescribeBudget",
            json!({"AccountId": account_id, "BudgetName": name}),
        ),
        (
            "DeleteBudget",
            json!({"AccountId": account_id, "BudgetName": name}),
        ),
        (
            "UpdateBudget",
            json!({"AccountId": account_id, "NewBudget": budget(name)}),
        ),
        (
            "DescribeNotificationsForBudget",
            json!({"AccountId": account_id, "BudgetName": name}),
        ),
        (
            "DescribeBudgetPerformanceHistory",
            json!({"AccountId": account_id, "BudgetName": name}),
        ),
        (
            "DescribeBudgetActionsForBudget",
            json!({"AccountId": account_id, "BudgetName": name}),
        ),
        (
            "CreateNotification",
            json!({
                "AccountId": account_id,
                "BudgetName": name,
                "Notification": notification(),
                "Subscribers": [subscriber.clone()]
            }),
        ),
        (
            "UpdateNotification",
            json!({
                "AccountId": account_id,
                "BudgetName": name,
                "OldNotification": notification(),
                "NewNotification": notification()
            }),
        ),
        (
            "DeleteNotification",
            json!({
                "AccountId": account_id,
                "BudgetName": name,
                "Notification": notification()
            }),
        ),
        (
            "CreateSubscriber",
            json!({
                "AccountId": account_id,
                "BudgetName": name,
                "Notification": notification(),
                "Subscriber": subscriber.clone()
            }),
        ),
        (
            "UpdateSubscriber",
            json!({
                "AccountId": account_id,
                "BudgetName": name,
                "Notification": notification(),
                "OldSubscriber": subscriber.clone(),
                "NewSubscriber": subscriber.clone()
            }),
        ),
        (
            "DeleteSubscriber",
            json!({
                "AccountId": account_id,
                "BudgetName": name,
                "Notification": notification(),
                "Subscriber": subscriber.clone()
            }),
        ),
        // `DescribeSubscribersForNotification` is deliberately NOT a row: a separately
        // filed divergence has its own recorded answer for a notification that is gone,
        // so this suite does not pin that operation's not-found path.
        (
            "CreateBudgetAction",
            create_body(
                account_id,
                name,
                "APPLY_IAM_POLICY",
                &iam_definition(),
                "MANUAL",
                "ACTUAL",
                &json!({"ActionThresholdValue": 80.0, "ActionThresholdType": "PERCENTAGE"}),
                &json!([subscriber.clone()]),
                None,
            ),
        ),
        (
            "DescribeBudgetAction",
            json!({
                "AccountId": account_id,
                "BudgetName": name,
                "ActionId": ABSENT_ACTION_IDS[0]
            }),
        ),
        (
            "DescribeBudgetActionHistories",
            json!({
                "AccountId": account_id,
                "BudgetName": name,
                "ActionId": ABSENT_ACTION_IDS[0]
            }),
        ),
        (
            "UpdateBudgetAction",
            json!({
                "AccountId": account_id,
                "BudgetName": name,
                "ActionId": ABSENT_ACTION_IDS[0],
                "ApprovalModel": "AUTOMATIC",
                "ExecutionRoleArn": execution_role_arn(account_id)
            }),
        ),
        (
            "DeleteBudgetAction",
            json!({
                "AccountId": account_id,
                "BudgetName": name,
                "ActionId": ABSENT_ACTION_IDS[0]
            }),
        ),
        (
            "ExecuteBudgetAction",
            json!({
                "AccountId": account_id,
                "BudgetName": name,
                "ActionId": ABSENT_ACTION_IDS[0],
                "ExecutionType": "APPROVE_BUDGET_ACTION"
            }),
        ),
        (
            "TagResource",
            json!({
                "ResourceARN": budget_arn(account_id, name),
                "ResourceTags": tag_list(&[("owner".to_string(), "ops".to_string())])
            }),
        ),
        (
            "UntagResource",
            json!({
                "ResourceARN": budget_arn(account_id, name),
                "ResourceTagKeys": ["owner"]
            }),
        ),
        (
            "ListTagsForResource",
            json!({"ResourceARN": budget_arn(account_id, name)}),
        ),
    ]
}

/// RULE 1: a budget this account does not hold is the one condition, so it is reported in
/// the one sentence — fixed text around the name the REQUEST spelled, and the same verb
/// whichever operation asked.
#[tokio::test]
async fn a_missing_budget_is_reported_in_the_same_words_by_every_operation() {
    let router = router();
    let account_id = caller_account();
    // One stored budget, so the store is not empty and a row's refusal is about the name
    // it named rather than about an empty service.
    let (status, body) = call(
        &router,
        "CreateBudget",
        &json!({"AccountId": &account_id, "Budget": budget("stored")}),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: expected CreateBudget to answer 200, got {status} {body}"
    );

    for name in absent_names() {
        let expected = format!("Unable to get budget: {name} - the budget doesn't exist.");
        for (operation, request) in requests_naming_a_budget(&account_id, &name) {
            let case = format!("{operation} naming {name:?}");
            let (status, body) = call(&router, operation, &request).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected the refusal at 400, got {status} {body}"
            );
            assert!(
                is_error(&body, "NotFoundException"),
                "case {case:?}: expected NotFoundException, got {body}"
            );
            assert_eq!(
                message(&body),
                expected,
                "case {case:?}: expected {expected:?}, got {body}"
            );
        }
    }
}

/// Every operation that addresses a budget ACTION by id, against a budget that DOES
/// exist, so the only thing the request cannot locate is the action.
fn requests_naming_an_action(
    account_id: &str,
    budget_name: &str,
    action_id: &str,
) -> Vec<(&'static str, Value)> {
    vec![
        (
            "DescribeBudgetAction",
            json!({
                "AccountId": account_id,
                "BudgetName": budget_name,
                "ActionId": action_id
            }),
        ),
        (
            "DescribeBudgetActionHistories",
            json!({
                "AccountId": account_id,
                "BudgetName": budget_name,
                "ActionId": action_id
            }),
        ),
        (
            "UpdateBudgetAction",
            json!({
                "AccountId": account_id,
                "BudgetName": budget_name,
                "ActionId": action_id,
                "ApprovalModel": "AUTOMATIC",
                "ExecutionRoleArn": execution_role_arn(account_id)
            }),
        ),
        (
            "DeleteBudgetAction",
            json!({
                "AccountId": account_id,
                "BudgetName": budget_name,
                "ActionId": action_id
            }),
        ),
        (
            "ExecuteBudgetAction",
            json!({
                "AccountId": account_id,
                "BudgetName": budget_name,
                "ActionId": action_id,
                "ExecutionType": "APPROVE_BUDGET_ACTION"
            }),
        ),
        (
            "TagResource",
            json!({
                "ResourceARN": action_arn(account_id, budget_name, action_id),
                "ResourceTags": tag_list(&[("owner".to_string(), "ops".to_string())])
            }),
        ),
        (
            "UntagResource",
            json!({
                "ResourceARN": action_arn(account_id, budget_name, action_id),
                "ResourceTagKeys": ["owner"]
            }),
        ),
        (
            "ListTagsForResource",
            json!({"ResourceARN": action_arn(account_id, budget_name, action_id)}),
        ),
    ]
}

/// RULE 2: a budget action this account does not hold is reported in ITS OWN sentence —
/// naming the account and the action id the request carried — so a client can tell it
/// from a missing budget, which shares the one error code.
#[tokio::test]
async fn a_missing_budget_action_is_reported_in_its_own_words() {
    let router = router();
    let account_id = caller_account();
    let budget_name = "action-not-found";
    seed_budget(&router, &account_id, budget_name).await;
    // One stored action, so a row's refusal is about the id it named.
    let held = seed_action(&router, &account_id, budget_name).await;

    for action_id in ABSENT_ACTION_IDS {
        assert_ne!(
            action_id, held,
            "the absent ids must not name the action the setup created"
        );
        let expected =
            format!("Action for [AccountId: {account_id}, ActionId: {action_id}] is not found!");
        for (operation, request) in requests_naming_an_action(&account_id, budget_name, action_id) {
            let case = format!("{operation} naming {action_id}");
            let (status, body) = call(&router, operation, &request).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected the refusal at 400, got {status} {body}"
            );
            assert!(
                is_error(&body, "NotFoundException"),
                "case {case:?}: expected NotFoundException, got {body}"
            );
            assert_eq!(
                message(&body),
                expected,
                "case {case:?}: expected {expected:?}, got {body}"
            );
        }
    }
}

/// The modelled `ActionStatus` catalog paired with the spelling a status-conflict refusal
/// carries it in: the wire value is upper snake case, the sentence words it in upper
/// camel case. Written out here rather than computed, so the expectation is this suite's
/// own; [`the_status_spellings_cover_the_whole_catalog`] holds it to the generated
/// catalog so it cannot drift from the model.
const STATUS_SPELLINGS: [(&str, &str); 10] = [
    ("STANDBY", "Standby"),
    ("PENDING", "Pending"),
    ("EXECUTION_IN_PROGRESS", "ExecutionInProgress"),
    ("EXECUTION_SUCCESS", "ExecutionSuccess"),
    ("EXECUTION_FAILURE", "ExecutionFailure"),
    ("REVERSE_IN_PROGRESS", "ReverseInProgress"),
    ("REVERSE_SUCCESS", "ReverseSuccess"),
    ("REVERSE_FAILURE", "ReverseFailure"),
    ("RESET_IN_PROGRESS", "ResetInProgress"),
    ("RESET_FAILURE", "ResetFailure"),
];

/// The sentence a method refused for the state of the action it names answers with.
fn state_conflict_message(status: &str) -> String {
    let (_, spelled) = STATUS_SPELLINGS
        .iter()
        .find(|(wire, _)| *wire == status)
        .unwrap_or_else(|| panic!("no spelling is written down for the status {status:?}"));
    format!("This method is not allowed during [ActionStatus: {spelled}]")
}

/// The spelling table covers the generated `ActionStatus` catalog exactly, so a status
/// the model gains or renames fails here rather than silently going untested.
#[test]
fn the_status_spellings_cover_the_whole_catalog() {
    let mut spelled: Vec<&str> = STATUS_SPELLINGS.iter().map(|(wire, _)| *wire).collect();
    spelled.sort_unstable();
    let mut modelled: Vec<&str> = ACTION_STATUS_VALUES.to_vec();
    modelled.sort_unstable();
    assert_eq!(
        spelled, modelled,
        "expected a spelling for every modelled ActionStatus, got {spelled:?} for {modelled:?}"
    );
}

/// RULE 3, over the WHOLE cross product of the modelled `ActionStatus` and
/// `ExecutionType` catalogs: an execution the action's current status does not allow is
/// refused as a conflict with that STATE — `ResourceLockedException`, the shape AWS gives
/// "the server rejected that particular method for the requested resource" — in a
/// sentence that names only the status the action carries, not the method and not the
/// execution type.
#[tokio::test]
async fn an_execution_its_status_disallows_is_refused_as_a_state_conflict() {
    let (router, engine) = router_with_engine();
    let account_id = caller_account();
    let budget_name = "execution-state-conflict";
    seed_budget(&router, &account_id, budget_name).await;
    let action_id = seed_action(&router, &account_id, budget_name).await;

    for status in ACTION_STATUS_VALUES {
        for execution_type in EXECUTION_TYPE_VALUES {
            let allowed = !IN_PROGRESS_STATUSES.contains(status)
                && ALLOWED_EXECUTIONS
                    .iter()
                    .any(|(from, execution, _, _)| from == status && execution == execution_type);
            if allowed {
                continue;
            }
            let case = format!("{execution_type} while {status}");
            seed_status(&engine, &account_id, budget_name, &action_id, status);
            let (http, body) = execute_action(
                &router,
                &account_id,
                budget_name,
                &action_id,
                execution_type,
            )
            .await;
            assert_eq!(
                http, 400,
                "case {case:?}: expected the refusal at 400, got {http} {body}"
            );
            assert!(
                is_error(&body, "ResourceLockedException"),
                "case {case:?}: expected ResourceLockedException at {http}, got {body}"
            );
            let expected = state_conflict_message(status);
            assert_eq!(
                message(&body),
                expected,
                "case {case:?}: expected {expected:?}, got {body}"
            );
        }
    }
}

/// RULE 3 for the other two methods the model gives `ResourceLockedException` to: a
/// delete and an update of an action whose work is still running are refused for the same
/// state, in the same sentence — the refusal is about the resource, not about the method.
#[tokio::test]
async fn a_delete_or_update_mid_flight_is_refused_in_the_same_words() {
    let (router, engine) = router_with_engine();
    let account_id = caller_account();
    let budget_name = "method-state-conflict";
    seed_budget(&router, &account_id, budget_name).await;
    let action_id = seed_action(&router, &account_id, budget_name).await;

    for status in IN_PROGRESS_STATUSES {
        let requests: Vec<(&str, Value)> = vec![
            (
                "DeleteBudgetAction",
                json!({
                    "AccountId": &account_id,
                    "BudgetName": budget_name,
                    "ActionId": &action_id
                }),
            ),
            (
                "UpdateBudgetAction",
                json!({
                    "AccountId": &account_id,
                    "BudgetName": budget_name,
                    "ActionId": &action_id,
                    "ApprovalModel": "AUTOMATIC"
                }),
            ),
        ];
        for (operation, request) in requests {
            let case = format!("{operation} while {status}");
            seed_status(&engine, &account_id, budget_name, &action_id, status);
            let (http, body) = call(&router, operation, &request).await;
            assert_eq!(
                http, 400,
                "case {case:?}: expected the refusal at 400, got {http} {body}"
            );
            assert!(
                is_error(&body, "ResourceLockedException"),
                "case {case:?}: expected ResourceLockedException at {http}, got {body}"
            );
            let expected = state_conflict_message(status);
            assert_eq!(
                message(&body),
                expected,
                "case {case:?}: expected {expected:?}, got {body}"
            );
        }
    }
}

/// The three members of a `Budget` that can determine its amount, each as the request
/// carries it. The planned map's key is the start of a month, "`UTC` in epoch seconds"
/// (`Budget$PlannedBudgetLimits`), which a monthly cost budget plans by.
fn amount_members() -> Vec<(&'static str, Value)> {
    vec![
        (
            "BudgetLimit",
            json!({"BudgetLimit": {"Amount": "100", "Unit": "USD"}}),
        ),
        (
            "PlannedBudgetLimits",
            json!({
                "PlannedBudgetLimits": {
                    civil_secs("2024-02-01T00:00:00Z").to_string(): {
                        "Amount": "100", "Unit": "USD"
                    }
                }
            }),
        ),
        (
            "AutoAdjustData",
            json!({"AutoAdjustData": {"AutoAdjustType": "FORECAST"}}),
        ),
    ]
}

/// A monthly cost budget carrying exactly the amount members `supplied` names.
fn budget_with(name: &str, supplied: &[(&'static str, Value)]) -> Value {
    let mut budget = json!({
        "BudgetName": name,
        "TimeUnit": "MONTHLY",
        "BudgetType": "COST"
    });
    for (_, members) in supplied {
        for (member, value) in members.as_object().expect("a member map") {
            budget[member] = value.clone();
        }
    }
    budget
}

/// RULE 4a: the members that determine a budget's amount are mutually exclusive AS A SET,
/// so a budget supplying more than one of them is refused — whichever pair, and whichever
/// write path — with the fixed sentence that names all three. One member alone is not
/// refused.
#[tokio::test]
async fn a_budget_may_determine_its_amount_in_only_one_way() {
    let router = router();
    let account_id = caller_account();
    let expected = "Unable to create/update budget - please provide only one of the followings: \
                    Budget Limit/ Planned Budget Limit/ Auto Adjust Data";
    let members = amount_members();

    // Every subset of the three of size two or more, plus every single member on its own.
    let mut subsets: Vec<Vec<(&'static str, Value)>> = Vec::new();
    for mask in 1u8..8 {
        subsets.push(
            members
                .iter()
                .enumerate()
                .filter(|(index, _)| mask & (1 << index) != 0)
                .map(|(_, member)| member.clone())
                .collect(),
        );
    }

    for subset in subsets {
        let supplied: Vec<&str> = subset.iter().map(|(name, _)| *name).collect();
        let exclusive = subset.len() > 1;
        for operation in ["CreateBudget", "UpdateBudget"] {
            let case = format!("{operation} supplying {supplied:?}");
            let name = format!("exclusivity-{}-{}", operation, supplied.join("-"));
            let member = if operation == "CreateBudget" {
                "Budget"
            } else {
                "NewBudget"
            };
            let request = json!({
                "AccountId": &account_id,
                member.to_string(): budget_with(&name, &subset)
            });
            let (status, body) = call(&router, operation, &request).await;
            if exclusive {
                assert!(
                    is_error(&body, "InvalidParameterException"),
                    "case {case:?}: expected InvalidParameterException at {status}, got {body}"
                );
                assert_eq!(
                    message(&body),
                    expected,
                    "case {case:?}: expected {expected:?}, got {body}"
                );
            } else {
                // One member alone determines the amount, so the rule has nothing to say:
                // the create is served, and the update reaches the budget lookup instead.
                assert!(
                    !message(&body).contains("only one of the followings"),
                    "case {case:?}: expected no exclusivity refusal, got {body}"
                );
            }
        }
    }
}

/// RULE 4b: a budget whose period runs backwards is refused with its own fixed sentence,
/// over rows that differ along how far the end falls before the start and along the write
/// path.
#[tokio::test]
async fn a_backwards_time_period_is_refused_in_fixed_words() {
    let router = router();
    let account_id = caller_account();
    let expected = "Unable to create/update budget - end time has to be after start time.";
    let start = civil_secs("2030-01-01T00:00:00Z");
    let rows: Vec<(&str, Value)> = vec![
        (
            "the end a year before the start",
            json!({"Start": start, "End": civil_secs("2029-01-01T00:00:00Z")}),
        ),
        (
            "the end a day before the start",
            json!({"Start": start, "End": start - 86_400}),
        ),
        (
            "the end one second before the start",
            json!({"Start": start, "End": start - 1}),
        ),
        (
            "the end exactly at the start",
            json!({"Start": start, "End": start}),
        ),
        (
            "the start a fraction of a second after the end",
            json!({"Start": start as f64 + 0.5, "End": start}),
        ),
    ];

    for (case, period) in rows {
        for operation in ["CreateBudget", "UpdateBudget"] {
            let member = if operation == "CreateBudget" {
                "Budget"
            } else {
                "NewBudget"
            };
            let mut carried = budget("backwards-period");
            carried["TimePeriod"] = period.clone();
            let request = json!({
                "AccountId": &account_id,
                member.to_string(): carried
            });
            let (status, body) = call(&router, operation, &request).await;
            assert!(
                is_error(&body, "InvalidParameterException"),
                "case {case:?} via {operation}: expected InvalidParameterException at {status}, \
                 got {body}"
            );
            assert_eq!(
                message(&body),
                expected,
                "case {case:?} via {operation}: expected {expected:?}, got {body}"
            );
        }
    }
}

/// RULE 4c: the notification quota belongs to the budget, and its refusal says so — the
/// fixed sentence naming the limit, with nothing from the request interpolated. Asserted
/// on the budget whose notifications arrived one at a time AND on the one that was
/// created carrying them, so the same sentence covers both ways to fill a budget up.
#[tokio::test]
async fn the_notification_quota_refusal_names_the_limit_and_nothing_else() {
    let router = router();
    let account_id = caller_account();
    let expected = format!(
        "Unable to create notification - one budget can only have \
         {MAX_NOTIFICATIONS_PER_BUDGET} notifications"
    );

    // A notification's identity is what it alerts on, so each one differs in its
    // threshold; the thresholds span the percentage range the quota is reached across.
    let thresholds: Vec<f64> = (1..=MAX_NOTIFICATIONS_PER_BUDGET)
        .map(|step| step as f64 * 10.0)
        .collect();
    let attached: Vec<Value> = thresholds
        .iter()
        .map(|threshold| {
            json!({
                "Notification": {
                    "NotificationType": "ACTUAL",
                    "ComparisonOperator": "GREATER_THAN",
                    "Threshold": threshold,
                    "ThresholdType": "PERCENTAGE"
                },
                "Subscribers": [email_subscriber(0)]
            })
        })
        .collect();

    // One budget filled one call at a time, one filled by the create itself.
    let filled_one_at_a_time = "quota-one-at-a-time";
    seed_budget(&router, &account_id, filled_one_at_a_time).await;
    for threshold in &thresholds {
        let (status, body) = call(
            &router,
            "CreateNotification",
            &json!({
                "AccountId": &account_id,
                "BudgetName": filled_one_at_a_time,
                "Notification": {
                    "NotificationType": "ACTUAL",
                    "ComparisonOperator": "GREATER_THAN",
                    "Threshold": threshold,
                    "ThresholdType": "PERCENTAGE"
                },
                "Subscribers": [email_subscriber(0)]
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "setup: expected the notification at {threshold} to be accepted, got {status} {body}"
        );
    }

    let filled_at_create = "quota-at-create";
    let (status, body) = call(
        &router,
        "CreateBudget",
        &json!({
            "AccountId": &account_id,
            "Budget": budget(filled_at_create),
            "NotificationsWithSubscribers": attached
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: expected the budget carrying its notifications to be created, got {status} {body}"
    );

    for budget_name in [filled_one_at_a_time, filled_at_create] {
        let (status, body) = call(
            &router,
            "CreateNotification",
            &json!({
                "AccountId": &account_id,
                "BudgetName": budget_name,
                "Notification": {
                    "NotificationType": "ACTUAL",
                    "ComparisonOperator": "GREATER_THAN",
                    "Threshold": 110.0,
                    "ThresholdType": "PERCENTAGE"
                },
                "Subscribers": [email_subscriber(0)]
            }),
        )
        .await;
        assert!(
            is_error(&body, "CreationLimitExceededException"),
            "case {budget_name:?}: expected CreationLimitExceededException at {status}, got {body}"
        );
        assert_eq!(
            message(&body),
            expected,
            "case {budget_name:?}: expected {expected:?}, got {body}"
        );
        assert!(
            !message(&body).contains(budget_name),
            "case {budget_name:?}: expected the quota sentence to name no budget, got {body}"
        );
    }
}
