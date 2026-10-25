// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the EXECUTION ROLE a budget action is written with, driven over
//! the wire through the same `MultiServiceRouter` the production router serves from.
//!
//! The RULE all three tests assert: the role an action carries lives in the same account
//! as the action — "the role passed for action execution and reversion. Roles and actions
//! must be in the same account" (`Action$ExecutionRoleArn`), so the account field of the
//! supplied ARN must be the `AccountId` the request names. The modelled `@pattern` fixes
//! the ARN's SHAPE only (`arn:aws…:iam::\d{12}:role/…`), so this rule holds on every
//! path that writes the member: `CreateBudgetAction` and `UpdateBudgetAction` each refuse
//! a foreign-account role with the client-error they model
//! (`InvalidParameterException`, HTTP 400) and persist nothing. An ARN whose account
//! field is no account id at all never reaches either handler: the modelled pattern
//! refuses it at request validation, which the third test pins.
//!
//! The table spans the class rather than one request: the accepted rows walk the
//! partition alternatives the modelled pattern lists, both of its role-resource forms
//! (with and without a path), the characters a role name may hold and the modelled length
//! bound, while the refused rows differ from the request's account in its last digit, in
//! its first digit, in every digit, and by naming an unrelated account — each still an ARN
//! the modelled pattern accepts, so the refusal is the cross-member rule's and not request
//! validation's.

use serde_json::json;
use std::sync::Arc;

use super::budget_action_execution::{
    create_body, describe_action, email_subscriber, iam_definition, seed_action, seed_budget,
};
use super::budget_action_history::read_history;
use super::budget_action_subscriber_limits::stored_action_count;
use super::budget_lifecycle::{caller_account, call, is_error, router_with_engine};

/// The partition alternatives `RoleArn`'s modelled pattern lists — the optional group of
/// `^arn:aws(-eusc|-cn|-us-gov|-iso|-iso-[a-z]{1})?:iam::\d{12}:role…`, read from
/// `smithy_query(query="shape",shape="RoleArn")`. The empty string is the bare `arn:aws`
/// the group's `?` allows.
const PARTITION_SUFFIXES: [&str; 6] = ["", "-eusc", "-cn", "-us-gov", "-iso", "-iso-b"];

/// A role ARN of exactly the modelled shape: the partition suffix, the account field, and
/// a role resource that is either `role/<name>` or `role/<path>/<name>` (the two
/// alternatives of the pattern's `role(\/[\x21-\x7F]+\/|\/)` group).
fn role_arn(partition_suffix: &str, account_id: &str, path: &str, name: &str) -> String {
    format!("arn:aws{partition_suffix}:iam::{account_id}:role/{path}{name}")
}

/// `account_id` with the digit at `index` bumped by one, so a row can be a FOREIGN
/// account that differs from the request's by a single digit — the smallest difference
/// between two account ids — at either end of the id.
fn digit_bumped(account_id: &str, index: usize) -> String {
    account_id
        .chars()
        .enumerate()
        .map(|(at, digit)| {
            if at == index {
                char::from_digit((digit.to_digit(10).unwrap_or(0) + 1) % 10, 10).unwrap_or(digit)
            } else {
                digit
            }
        })
        .collect()
}

/// One row of the execution-role table.
struct Row {
    case: String,
    role: String,
    /// Whether the role is in the account the request names, which is the whole rule.
    accepted: bool,
}

/// The execution-role table for a request naming `account_id`.
fn role_rows(account_id: &str) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    // Accepted: the request's own account, across every partition the pattern lists.
    for suffix in PARTITION_SUFFIXES {
        rows.push(Row {
            case: format!("same account, partition aws{suffix}"),
            role: role_arn(suffix, account_id, "", "budget-action-role"),
            accepted: true,
        });
    }
    // Accepted: the role-resource form that carries a path, and the characters
    // `[\w+=,.@-]` a role name may hold.
    rows.push(Row {
        case: "same account, role name under a path".to_string(),
        role: role_arn("", account_id, "service-role/budgets/", "executor"),
        accepted: true,
    });
    rows.push(Row {
        case: "same account, role name using the whole permitted charset".to_string(),
        role: role_arn("", account_id, "", "budget.action+exec=role,v1@team-2_x"),
        accepted: true,
    });
    // Accepted: a role name at the length the modelled `@length(min: 32, max: 618)` tops
    // out at, so the bound itself is exercised from inside.
    const ROLE_ARN_MAX_LEN: usize = 618;
    let prefix = role_arn("", account_id, "", "");
    assert!(
        prefix.len() < ROLE_ARN_MAX_LEN,
        "the ARN prefix {prefix:?} must leave room for a role name inside the modelled \
         maximum length of {ROLE_ARN_MAX_LEN}"
    );
    rows.push(Row {
        case: "same account, ARN at the modelled maximum length".to_string(),
        role: format!("{prefix}{}", "r".repeat(ROLE_ARN_MAX_LEN - prefix.len())),
        accepted: true,
    });
    // Refused: another account — differing from the request's in its last digit, in its
    // first digit, in every digit, and an unrelated id — the last one with another
    // partition and a path, so the refusal is not keyed to one ARN form.
    let foreign = [
        (
            "one digit (the last) from the request's account".to_string(),
            digit_bumped(account_id, account_id.len() - 1),
            "",
            "",
        ),
        (
            "one digit (the first) from the request's account".to_string(),
            digit_bumped(account_id, 0),
            "",
            "",
        ),
        (
            "no digit shared with the request's account".to_string(),
            "999999999999".to_string(),
            "",
            "",
        ),
        (
            "another account, another partition and a path".to_string(),
            "123456789012".to_string(),
            "-cn",
            "service-role/budgets/",
        ),
    ];
    for (why, other_account, suffix, path) in foreign {
        assert_ne!(
            other_account, account_id,
            "case {why:?}: the row must name an account other than the request's"
        );
        rows.push(Row {
            case: format!("foreign account: {why}"),
            role: role_arn(suffix, &other_account, path, "budget-action-role"),
            accepted: false,
        });
    }
    rows
}

/// The role the stored action carries, read back through `DescribeBudgetAction`.
async fn stored_role(
    router: &smithy_server_runtime::MultiServiceRouter,
    account_id: &str,
    budget_name: &str,
    action_id: &str,
) -> String {
    let (status, body) = describe_action(router, account_id, budget_name, action_id).await;
    assert_eq!(
        status, 200,
        "expected the action under test to read back, got {status} {body}"
    );
    body["Action"]["ExecutionRoleArn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// RULE, on the create path: `CreateBudgetAction` stores an action whose
/// `ExecutionRoleArn` is in the account the request names and reads it back as supplied;
/// a role in any other account is refused with `InvalidParameterException` and no action
/// is minted.
#[tokio::test]
async fn creating_an_action_requires_a_role_in_the_actions_account() {
    let (router, engine): (_, Arc<engine::InMemoryEngine>) = router_with_engine();
    let account_id = caller_account();
    let budget_name = "role-checked-action";
    seed_budget(&router, &account_id, budget_name).await;

    for row in role_rows(&account_id) {
        let case = &row.case;
        let mut request = create_body(
            &account_id,
            budget_name,
            "APPLY_IAM_POLICY",
            &iam_definition(),
            "AUTOMATIC",
            "ACTUAL",
            &json!({"ActionThresholdValue": 80.0, "ActionThresholdType": "PERCENTAGE"}),
            &json!([email_subscriber(0)]),
            None,
        );
        request["ExecutionRoleArn"] = json!(&row.role);
        let before = stored_action_count(&engine, &account_id, budget_name);
        let (status, body) = call(&router, "CreateBudgetAction", &request).await;
        if row.accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the role {} to be accepted, got {status} {body}",
                row.role
            );
            let action_id = body["ActionId"].as_str().unwrap_or_default().to_string();
            assert_eq!(
                stored_role(&router, &account_id, budget_name, &action_id).await,
                row.role,
                "case {case:?}: expected the created action to carry the supplied role"
            );
            assert_eq!(
                stored_action_count(&engine, &account_id, budget_name),
                before + 1,
                "case {case:?}: expected the accepted create to store one action"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected the role {} to be refused at 400, got {status} {body}",
                row.role
            );
            assert!(
                is_error(&body, "InvalidParameterException"),
                "case {case:?}: expected InvalidParameterException, got {body}"
            );
            assert_eq!(
                stored_action_count(&engine, &account_id, budget_name),
                before,
                "case {case:?}: expected a refused create to store nothing"
            );
        }
    }
}

/// RULE, on the update path: a replacement `ExecutionRoleArn` is held to the same rule,
/// so an update cannot leave an action holding a role a create would have been refused.
/// A role in the action's account replaces the stored one and reads back; a role in
/// another account is refused with `InvalidParameterException`, the stored action keeps
/// the role it had, and no `UPDATE_ACTION` record is appended.
#[tokio::test]
async fn updating_an_action_requires_a_role_in_the_actions_account() {
    let (router, _engine) = router_with_engine();
    let account_id = caller_account();
    let budget_name = "role-updated-action";
    seed_budget(&router, &account_id, budget_name).await;
    let action_id = seed_action(&router, &account_id, budget_name).await;

    // The role the action holds when a row starts, which a refused update must leave
    // alone, and the history it has, which a refused update must not grow.
    let mut held = stored_role(&router, &account_id, budget_name, &action_id).await;
    let mut records = read_history(&router, &account_id, budget_name, &action_id, None, "setup")
        .await
        .len();

    for row in role_rows(&account_id) {
        let case = &row.case;
        let (status, body) = call(
            &router,
            "UpdateBudgetAction",
            &json!({
                "AccountId": &account_id,
                "BudgetName": budget_name,
                "ActionId": &action_id,
                "ExecutionRoleArn": &row.role
            }),
        )
        .await;
        if row.accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the role {} to be accepted, got {status} {body}",
                row.role
            );
            assert_eq!(
                body["OldAction"]["ExecutionRoleArn"].as_str(),
                Some(held.as_str()),
                "case {case:?}: expected OldAction to carry the role it replaced, got {}",
                body["OldAction"]["ExecutionRoleArn"]
            );
            assert_eq!(
                body["NewAction"]["ExecutionRoleArn"].as_str(),
                Some(row.role.as_str()),
                "case {case:?}: expected NewAction to carry the replacement, got {}",
                body["NewAction"]["ExecutionRoleArn"]
            );
            held = row.role.clone();
            records += 1;
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected the role {} to be refused at 400, got {status} {body}",
                row.role
            );
            assert!(
                is_error(&body, "InvalidParameterException"),
                "case {case:?}: expected InvalidParameterException, got {body}"
            );
            assert!(
                body.get("OldAction").is_none() && body.get("NewAction").is_none(),
                "case {case:?}: expected a refused update to report no action, got {body}"
            );
        }
        assert_eq!(
            stored_role(&router, &account_id, budget_name, &action_id).await,
            held,
            "case {case:?}: expected the stored role to be {held}"
        );
        let history = read_history(&router, &account_id, budget_name, &action_id, None, case).await;
        assert_eq!(
            history.len(),
            records,
            "case {case:?}: expected {records} history records, got {}",
            history.len()
        );
    }
}

/// The modelled `RoleArn` `@pattern` requires the account field to be twelve digits, so
/// an ARN carrying anything else never reaches a handler: it is refused at request
/// validation, before the cross-member rule above is ever consulted. Asserted on both
/// write paths, over an account field that is letters, one that is empty and one that is
/// eleven digits.
#[tokio::test]
async fn a_role_whose_account_field_is_no_account_id_is_refused_by_request_validation() {
    let (router, engine): (_, Arc<engine::InMemoryEngine>) = router_with_engine();
    let account_id = caller_account();
    let budget_name = "role-shape-checked";
    seed_budget(&router, &account_id, budget_name).await;
    let action_id = seed_action(&router, &account_id, budget_name).await;
    let held = stored_role(&router, &account_id, budget_name, &action_id).await;

    for role in [
        "arn:aws:iam::abcdefghijkl:role/budget-action-role",
        "arn:aws:iam:::role/budget-action-role",
        "arn:aws:iam::32109876543:role/budget-action-role",
    ] {
        let mut request = create_body(
            &account_id,
            budget_name,
            "APPLY_IAM_POLICY",
            &iam_definition(),
            "AUTOMATIC",
            "ACTUAL",
            &json!({"ActionThresholdValue": 80.0, "ActionThresholdType": "PERCENTAGE"}),
            &json!([email_subscriber(0)]),
            None,
        );
        request["ExecutionRoleArn"] = json!(role);
        let before = stored_action_count(&engine, &account_id, budget_name);
        let (status, body) = call(&router, "CreateBudgetAction", &request).await;
        assert_eq!(
            status, 400,
            "case {role:?}: expected the create to be refused at 400, got {status} {body}"
        );
        assert!(
            is_error(&body, "ValidationException"),
            "case {role:?}: expected ValidationException, got {body}"
        );
        assert_eq!(
            stored_action_count(&engine, &account_id, budget_name),
            before,
            "case {role:?}: expected a refused create to store nothing"
        );

        let (status, body) = call(
            &router,
            "UpdateBudgetAction",
            &json!({
                "AccountId": &account_id,
                "BudgetName": budget_name,
                "ActionId": &action_id,
                "ExecutionRoleArn": role
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {role:?}: expected the update to be refused at 400, got {status} {body}"
        );
        assert!(
            is_error(&body, "ValidationException"),
            "case {role:?}: expected ValidationException, got {body}"
        );
        assert_eq!(
            stored_role(&router, &account_id, budget_name, &action_id).await,
            held,
            "case {role:?}: expected the stored role to be unchanged"
        );
    }
}
