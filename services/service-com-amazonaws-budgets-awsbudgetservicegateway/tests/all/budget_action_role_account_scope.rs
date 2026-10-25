// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for a budget action's execution role as a rule over the REQUEST:
//! what answers it, and what a refusal by it leaves behind. Driven over the wire through
//! the same `MultiServiceRouter` the production router serves from.
//!
//! The RULE: the role a budget action is written with lives in the account the action
//! lives in — "the role passed for action execution and reversion. Roles and actions must
//! be in the same account" (`Action$ExecutionRoleArn`) — a rule over two members of the
//! request, so it is answered from the request alone: a write naming a role in another
//! account is refused as the invalid input value it is, with a message naming the member
//! and both accounts, and the store is left exactly as it was.
//!
//! [`super::budget_action_execution_role`] spans the role VALUES the rule accepts and
//! refuses. This suite spans the other axes — what the refused request MEETS, and the
//! other members it CARRIES:
//!
//! 1. On the create path: a budget that does not exist, and a budget already at the
//!    action ceiling. The role refuses both, while the same request carrying a role in
//!    the request's own account reaches the store and is answered by what the store holds
//!    (`NotFoundException`, `ServiceQuotaExceededException`).
//! 2. On the update path: an action of every modelled `ActionType`, whose stored role is
//!    left as it was.
//! 3. A refused update applies NONE of the other members it carried, and appends no
//!    history record.
//!
//! Every refusal in this service is answered at HTTP 400 whatever 4xx its error shape's
//! `@httpError` declares (the rule [`super::client_error_envelopes`] pins), so the rows
//! are told apart by the error code the body carries.

use serde_json::{json, Value};
use std::sync::Arc;

/// The `ActionType` catalog is not re-typed here: this is the model-derived constant the
/// scaffold generated into `src/ops/types/shared_types.rs`, so a table built from it
/// cannot drift from the model.
use service_com_amazonaws_budgets_awsbudgetservicegateway::ops::types::ACTION_TYPE_VALUES;
use service_com_amazonaws_budgets_awsbudgetservicegateway::state::MAX_ACTIONS_PER_BUDGET;

use super::budget_action_execution::{
    create_body, definition_for, describe_action, email_subscriber, execution_role_arn,
    iam_definition, seed_action, seed_budget, simple_create_body, sns_subscriber,
};
use super::budget_action_history::read_history;
use super::budget_action_subscriber_limits::stored_action_count;
use super::budget_lifecycle::{account, caller_account, call, is_error, router_with_engine, same};

/// A role ARN of exactly the modelled `RoleArn` shape
/// (`smithy_query(query="shape",shape="RoleArn")`) in `account_id` — the same builder
/// [`execution_role_arn`] is, but for an account that is NOT the request's, which is the
/// whole point of each row below.
fn role_in(account_id: &str) -> String {
    execution_role_arn(account_id)
}

/// `preferred` unless that is the request's own account, in which case `fallback` — so a
/// row stays FOREIGN whatever account this emulator answers for.
fn other_than(preferred: u64, fallback: u64, request_account: &str) -> String {
    let chosen = account(preferred);
    if chosen == request_account {
        account(fallback)
    } else {
        chosen
    }
}

/// The accounts a foreign role is named in: the lowest id the modelled `^\d{12}$` admits,
/// the highest, and an unrelated one in between. Each is asserted to differ from the
/// request's account, so no row can quietly stop being foreign.
fn foreign_accounts(request_account: &str) -> Vec<(String, String)> {
    let rows = [
        (
            "the lowest account id the pattern admits".to_string(),
            other_than(0, 1, request_account),
        ),
        (
            "the highest account id the pattern admits".to_string(),
            other_than(999_999_999_999, 999_999_999_998, request_account),
        ),
        (
            "an unrelated account".to_string(),
            other_than(123_456_789_012, 210_987_654_321, request_account),
        ),
    ];
    for (case, candidate) in &rows {
        assert_ne!(
            candidate, request_account,
            "case {case:?}: the row must name an account other than the request's"
        );
    }
    rows.into_iter().collect()
}

/// Every assertion a refusal by the execution-role rule owes: the modelled client error
/// for an invalid input value (`InvalidParameterException`, HTTP 400), with a message
/// naming the member at fault and both accounts, since the rule is about the pair.
fn assert_role_refusal(
    case: &str,
    status: u16,
    body: &Value,
    request_account: &str,
    role_account: &str,
) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected the foreign-account role to be refused at 400, got \
         {status} {body}"
    );
    assert!(
        is_error(body, "InvalidParameterException"),
        "case {case:?}: expected InvalidParameterException, got {body}"
    );
    let message = body
        .get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    for needle in ["ExecutionRoleArn", request_account, role_account] {
        assert!(
            message.contains(needle),
            "case {case:?}: expected the refusal message to name {needle:?}, got \
             {message:?}"
        );
    }
}

/// RULE, on the create path: the role is checked against the request's `AccountId` from
/// the request alone, so a foreign-account role is refused as the invalid input value it
/// is whatever the store holds — for a budget that does not exist, and for a budget
/// already holding the maximum number of actions. The control row shows the store IS
/// what answers a request the rule accepts: the same request carrying a role in its own
/// account is answered `NotFoundException` and `ServiceQuotaExceededException`
/// respectively. Nothing is persisted either way.
///
/// Every refusal in this service is answered at HTTP 400 whatever 4xx its error shape's
/// `@httpError` declares (the rule `client_error_envelopes.rs` pins), so the rows are
/// told apart by the error code, not by the status.
#[tokio::test]
async fn a_foreign_role_is_refused_whatever_the_store_holds() {
    let (router, engine): (_, Arc<engine::InMemoryEngine>) = router_with_engine();
    let request_account = caller_account();

    // A budget that exists and is filled to the action ceiling, and a name that was
    // never created: the two store states a create can meet.
    let full_budget = "role-scope-full";
    seed_budget(&router, &request_account, full_budget).await;
    for _ in 0..MAX_ACTIONS_PER_BUDGET {
        seed_action(&router, &request_account, full_budget).await;
    }
    let missing_budget = "role-scope-never-created";

    // (the budget the row names, the error an ACCEPTABLE role gets there) — the control
    // that proves the row's refusal came from the role and not from the store.
    let states: [(&str, &str, &str); 2] = [
        (
            "a budget that does not exist",
            missing_budget,
            "NotFoundException",
        ),
        (
            "a budget at the action ceiling",
            full_budget,
            "ServiceQuotaExceededException",
        ),
    ];

    for (state_case, budget_name, control_error) in states {
        let stored_before = (budget_name != missing_budget)
            .then(|| stored_action_count(&engine, &request_account, budget_name));

        for (why, role_account) in foreign_accounts(&request_account) {
            let case = format!("{state_case}, role in {why}");
            let mut request = simple_create_body(&request_account, budget_name);
            request["ExecutionRoleArn"] = json!(role_in(&role_account));
            let (status, body) = call(&router, "CreateBudgetAction", &request).await;
            assert_role_refusal(&case, status, &body, &request_account, &role_account);
            assert!(
                body.get("ActionId").is_none(),
                "case {case:?}: expected no ActionId to be minted, got {body}"
            );
            if let Some(before) = stored_before {
                assert_eq!(
                    stored_action_count(&engine, &request_account, budget_name),
                    before,
                    "case {case:?}: expected a refused create to store nothing"
                );
            }
        }

        // The control: the same request with a role in the request's own account gets
        // past the rule and is answered by the store.
        let (status, body) = call(
            &router,
            "CreateBudgetAction",
            &simple_create_body(&request_account, budget_name),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {state_case:?}: expected a role in the request's own account to reach \
             the store and be refused at 400, got {status} {body}"
        );
        assert!(
            is_error(&body, control_error),
            "case {state_case:?}: expected {control_error}, got {body}"
        );
    }
}

/// RULE, on the update path: the role an action is left holding is checked against the
/// request's `AccountId` alone, so the refusal does not depend on the action being
/// updated — asserted over an action of EVERY modelled `ActionType`, each on its own
/// budget, and over the role the update would have replaced, which stays as it was.
/// A replacement role in the request's own account is served, so the type of the action
/// is not what refused the rows.
#[tokio::test]
async fn a_foreign_replacement_role_is_refused_for_an_action_of_any_type() {
    let (router, _engine) = router_with_engine();
    let request_account = caller_account();

    for action_type in ACTION_TYPE_VALUES {
        let budget_name = format!("role-scope-{}", action_type.to_ascii_lowercase());
        seed_budget(&router, &request_account, &budget_name).await;
        let mut create = create_body(
            &request_account,
            &budget_name,
            action_type,
            &definition_for(action_type),
            "MANUAL",
            "ACTUAL",
            &json!({"ActionThresholdValue": 90.0, "ActionThresholdType": "PERCENTAGE"}),
            &json!([email_subscriber(0)]),
            None,
        );
        create["ExecutionRoleArn"] = json!(execution_role_arn(&request_account));
        let (status, body) = call(&router, "CreateBudgetAction", &create).await;
        assert_eq!(
            status, 200,
            "setup {action_type}: expected the create to answer 200, got {status} {body}"
        );
        let action_id = body["ActionId"].as_str().unwrap_or_default().to_string();
        let held = execution_role_arn(&request_account);

        for (why, role_account) in foreign_accounts(&request_account) {
            let case = format!("{action_type} action, replacement role in {why}");
            let (status, body) = call(
                &router,
                "UpdateBudgetAction",
                &json!({
                    "AccountId": &request_account,
                    "BudgetName": &budget_name,
                    "ActionId": &action_id,
                    "ExecutionRoleArn": role_in(&role_account)
                }),
            )
            .await;
            assert_role_refusal(&case, status, &body, &request_account, &role_account);

            let (status, body) =
                describe_action(&router, &request_account, &budget_name, &action_id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the action to still read back, got {status} {body}"
            );
            assert_eq!(
                body["Action"]["ExecutionRoleArn"].as_str(),
                Some(held.as_str()),
                "case {case:?}: expected the stored role to be {held}, got {}",
                body["Action"]["ExecutionRoleArn"]
            );
        }

        // The control: a replacement role in the request's own account is served, so the
        // action's type is not what refused the rows above.
        let replacement = format!("arn:aws:iam::{request_account}:role/budget-action-second");
        let (status, body) = call(
            &router,
            "UpdateBudgetAction",
            &json!({
                "AccountId": &request_account,
                "BudgetName": &budget_name,
                "ActionId": &action_id,
                "ExecutionRoleArn": &replacement
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "control {action_type}: expected a role in the request's own account to be \
             served, got {status} {body}"
        );
        assert_eq!(
            body["NewAction"]["ExecutionRoleArn"].as_str(),
            Some(replacement.as_str()),
            "control {action_type}: expected the replacement to be applied, got {body}"
        );
    }
}

/// RULE, on the update path: the rule is checked before anything is written, so an update
/// refused for its role applies NONE of the other members it carried — asserted over each
/// other updatable member on its own and over all of them at once, against the whole
/// stored action and its history.
#[tokio::test]
async fn an_update_refused_for_its_role_applies_none_of_its_other_changes() {
    let (router, _engine) = router_with_engine();
    let request_account = caller_account();
    let budget_name = "role-scope-atomic";
    seed_budget(&router, &request_account, budget_name).await;
    let action_id = seed_action(&router, &request_account, budget_name).await;

    // The action as stored before any refused update, and the history it has: neither may
    // move. The action was created `APPLY_IAM_POLICY`, so a replacement `Definition` is
    // an IAM one — "this field also determines the format for definition".
    let (status, body) = describe_action(&router, &request_account, budget_name, &action_id).await;
    assert_eq!(
        status, 200,
        "setup: expected the seeded action to read back, got {status} {body}"
    );
    let held = body["Action"].clone();
    let records = read_history(&router, &request_account, budget_name, &action_id, None, "setup")
        .await
        .len();

    // Every other member `UpdateBudgetAction` takes, each carrying a value that differs
    // from the stored one, so applying it would be visible.
    let changes: Vec<(&str, Value)> = vec![
        ("NotificationType", json!("FORECASTED")),
        ("ApprovalModel", json!("AUTOMATIC")),
        (
            "ActionThreshold",
            json!({"ActionThresholdValue": 42.0, "ActionThresholdType": "ABSOLUTE_VALUE"}),
        ),
        ("Definition", iam_definition()),
        (
            "Subscribers",
            json!([sns_subscriber(), email_subscriber(7)]),
        ),
    ];

    // Each member on its own, then all of them together.
    let mut rows: Vec<(String, serde_json::Map<String, Value>)> = Vec::new();
    for (member, value) in &changes {
        let mut one = serde_json::Map::new();
        one.insert((*member).to_string(), value.clone());
        rows.push(((*member).to_string(), one));
    }
    let mut everything = serde_json::Map::new();
    for (member, value) in &changes {
        everything.insert((*member).to_string(), value.clone());
    }
    rows.push(("every other member at once".to_string(), everything));

    for (case, extra) in rows {
        let (why, role_account) = foreign_accounts(&request_account)
            .into_iter()
            .next()
            .expect("the foreign-account table is not empty");
        let mut request = json!({
            "AccountId": &request_account,
            "BudgetName": budget_name,
            "ActionId": &action_id,
            "ExecutionRoleArn": role_in(&role_account)
        });
        for (member, value) in &extra {
            request[member] = value.clone();
        }
        let case = format!("{case} alongside a role in {why}");
        let (status, body) = call(&router, "UpdateBudgetAction", &request).await;
        assert_role_refusal(&case, status, &body, &request_account, &role_account);

        let (status, body) =
            describe_action(&router, &request_account, budget_name, &action_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the action to still read back, got {status} {body}"
        );
        assert!(
            same(&body["Action"], &held),
            "case {case:?}: expected the stored action to be unchanged at {held}, got {}",
            body["Action"]
        );
        let history =
            read_history(&router, &request_account, budget_name, &action_id, None, &case).await;
        assert_eq!(
            history.len(),
            records,
            "case {case:?}: expected a refused update to append no history record, so \
             {records} records, got {}",
            history.len()
        );
    }

    // The control: an update whose role IS in the request's account takes, so the rows
    // above were refused by the rule and not by something that refuses every update.
    let (status, body) = call(
        &router,
        "UpdateBudgetAction",
        &json!({
            "AccountId": &request_account,
            "BudgetName": budget_name,
            "ActionId": &action_id,
            "NotificationType": "FORECASTED"
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "control: expected an update carrying no foreign role to be served, got \
         {status} {body}"
    );
    assert_eq!(
        body["NewAction"]["NotificationType"].as_str(),
        Some("FORECASTED"),
        "control: expected the served update to apply its change, got {body}"
    );

    // And the create path answers the same way for a request whose only fault is the
    // role, so nothing above depended on the member being optional.
    let (why, role_account) = foreign_accounts(&request_account)
        .into_iter()
        .next_back()
        .expect("the foreign-account table is not empty");
    let mut request = create_body(
        &request_account,
        budget_name,
        "APPLY_IAM_POLICY",
        &iam_definition(),
        "MANUAL",
        "ACTUAL",
        &json!({"ActionThresholdValue": 80.0, "ActionThresholdType": "PERCENTAGE"}),
        &json!([email_subscriber(0)]),
        None,
    );
    request["ExecutionRoleArn"] = json!(role_in(&role_account));
    let (status, body) = call(&router, "CreateBudgetAction", &request).await;
    assert_role_refusal(
        &format!("create with a role in {why}"),
        status,
        &body,
        &request_account,
        &role_account,
    );
}
