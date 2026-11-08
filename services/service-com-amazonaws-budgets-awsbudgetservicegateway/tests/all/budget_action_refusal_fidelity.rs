// SPDX-License-Identifier: CC0-1.0

//! The HTTP status this service pairs with the error shapes whose modelled `@httpError`
//! is NOT 400, which the budget-action plane is the only place to reach some of.
//!
//! `client_error_envelopes.rs` states the rule over the shapes it can reach from the
//! budget plane: a refusal of the request is answered with HTTP 400 whatever 4xx the error
//! shape's `@httpError` declares, keeping the model's own `__type` and members. This suite
//! holds the same rule to the two shapes that are NOT answered at 400 by their model —
//! `AccessDeniedException` (`@httpError` 403) and `ResourceLockedException` (423) — so no
//! shape of this service is left answering the status its model declares rather than the
//! status the service sends.
//!
//! The rows span what the rule generalises over: the error shape, the condition that
//! raises it (an account that is not the caller's, an action mid-flight, an action nobody
//! holds) and the operation that answers it.
//!
//! The harness (`router`, `router_with_engine`, `call`, `is_error`, `caller_account`) and
//! the request builders are the sibling action suites', so every suite in this target
//! drives the service the same way.

use serde_json::{json, Value};

use service_com_amazonaws_budgets_awsbudgetservicegateway::state::{
    ACTION_STATUS_EXECUTION_IN_PROGRESS, EXECUTION_TYPE_APPROVE,
};

use super::budget_action_execution::{seed_action, seed_budget, seed_status};
use super::budget_lifecycle::{account, caller_account, call, is_error, router_with_engine};
use super::client_error_envelopes::message;

/// The status every refusal of a request is answered with, whatever 4xx the error shape's
/// `@httpError` declares.
const REFUSAL_STATUS: u16 = 400;

/// One row: the condition, the request that raises it, the shape the model declares for it
/// and the `@httpError` that shape carries
/// (`smithy_query(query="shape",shape="AccessDeniedException,ResourceLockedException,NotFoundException")`).
struct Row {
    case: &'static str,
    operation: &'static str,
    request: Value,
    exception: &'static str,
    modeled_status: u16,
}

/// RULE: the status is the SERVICE's, not the shape's — every row answers
/// [`REFUSAL_STATUS`] with the model's own error code and a message a caller can read.
#[tokio::test]
async fn the_shapes_whose_modelled_status_is_not_400_are_answered_at_400() {
    let (router, engine) = router_with_engine();
    let account_id = caller_account();
    let budget_name = "status-fold";
    seed_budget(&router, &account_id, budget_name).await;
    let action_id = seed_action(&router, &account_id, budget_name).await;
    // Work this service started and has not settled yet, so the three methods the model
    // gives `ResourceLockedException` to refuse the action for the state it is in.
    seed_status(
        &engine,
        &account_id,
        budget_name,
        &action_id,
        ACTION_STATUS_EXECUTION_IN_PROGRESS,
    );
    let foreign = account(210_987_654_321);
    // Well-formed per `ActionId`'s modelled `@pattern`, minted for nothing.
    let absent_action = "3f2504e0-4f89-41d3-9a0c-0305e82c3301";

    let rows = vec![
        Row {
            case: "a read for an account that is not the caller's",
            operation: "DescribeBudgetAction",
            request: json!({
                "AccountId": &foreign,
                "BudgetName": budget_name,
                "ActionId": &action_id
            }),
            exception: "AccessDeniedException",
            modeled_status: 403,
        },
        Row {
            case: "a write for an account that is not the caller's",
            operation: "DeleteBudgetAction",
            request: json!({
                "AccountId": &foreign,
                "BudgetName": budget_name,
                "ActionId": &action_id
            }),
            exception: "AccessDeniedException",
            modeled_status: 403,
        },
        Row {
            case: "a tagging call whose ARN names another account",
            operation: "ListTagsForResource",
            request: json!({
                "ResourceARN": format!("arn:aws:budgets::{foreign}:budget/{budget_name}")
            }),
            exception: "AccessDeniedException",
            modeled_status: 403,
        },
        Row {
            case: "an execution of an action whose work is still running",
            operation: "ExecuteBudgetAction",
            request: json!({
                "AccountId": &account_id,
                "BudgetName": budget_name,
                "ActionId": &action_id,
                "ExecutionType": EXECUTION_TYPE_APPROVE
            }),
            exception: "ResourceLockedException",
            modeled_status: 423,
        },
        Row {
            case: "an update of an action whose work is still running",
            operation: "UpdateBudgetAction",
            request: json!({
                "AccountId": &account_id,
                "BudgetName": budget_name,
                "ActionId": &action_id,
                "ApprovalModel": "AUTOMATIC"
            }),
            exception: "ResourceLockedException",
            modeled_status: 423,
        },
        Row {
            case: "a delete of an action whose work is still running",
            operation: "DeleteBudgetAction",
            request: json!({
                "AccountId": &account_id,
                "BudgetName": budget_name,
                "ActionId": &action_id
            }),
            exception: "ResourceLockedException",
            modeled_status: 423,
        },
        // The 404 shape, already folded before this change, so the rows above cannot be
        // passing because the fold stopped applying to anything else.
        Row {
            case: "a read of an action nobody holds",
            operation: "DescribeBudgetAction",
            request: json!({
                "AccountId": &account_id,
                "BudgetName": budget_name,
                "ActionId": absent_action
            }),
            exception: "NotFoundException",
            modeled_status: 404,
        },
    ];

    for row in rows {
        let Row {
            case,
            operation,
            request,
            exception,
            modeled_status,
        } = row;
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            status, REFUSAL_STATUS,
            "case {case:?}: expected {operation} to answer {REFUSAL_STATUS} \
             (the shape's @httpError is {modeled_status}), got {status} {body}"
        );
        assert!(
            is_error(&body, exception),
            "case {case:?}: expected {exception}, got {body}"
        );
        assert!(
            !message(&body).is_empty(),
            "case {case:?}: expected a message a caller can read, got {body}"
        );
    }
}
