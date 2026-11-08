// SPDX-License-Identifier: CC0-1.0

//! The STATE an `ExecuteBudgetAction` approval is accepted from, and what a refused
//! approval leaves behind.
//!
//! `ExecutionType$APPROVE_BUDGET_ACTION` GIVES the approval a triggered execution of the
//! action is waiting for ("this specifies if the action needs manual or automatic
//! approval" — `Action$ApprovalModel`). So the RULE asserted here:
//!
//! > an approval is work only while the action is waiting for one — `ActionStatus$PENDING`
//! > — and an ARMED action (`ActionStatus$STANDBY`), whose trigger threshold the spend it
//! > watches has not reached, has no triggered execution to approve: the approval is
//! > refused for the state the resource is in (`ResourceLockedException`, naming that
//! > status) and the action keeps the status, the members and the history it already had —
//! > so it is still the settled action every other method accepts, the delete included.
//!
//! The wording of that refusal over the whole `ActionStatus` × `ExecutionType` cross
//! product is `error_message_fidelity.rs`'s; what this suite adds is the STATUS the
//! approval is gated on, the table of ways an action is created armed, and the state the
//! refusal leaves — the delete of an action that has just been refused an approval being
//! the case a client's teardown path depends on.
//!
//! The harness (`router`, `call`, `is_error`, `caller_account`) and the request builders
//! are the sibling action suites', so every suite in this target drives the service the
//! same way.

use serde_json::{json, Value};

use service_com_amazonaws_budgets_awsbudgetservicegateway::state::{
    ACTION_STATUS_EXECUTION_IN_PROGRESS, ACTION_STATUS_PENDING, ACTION_STATUS_STANDBY,
    EXECUTION_TYPE_APPROVE,
};

use super::budget_action_execution::{
    create_body, definition_for, email_subscriber, execute_action, iam_definition, read_status,
    seed_budget, seed_pending_action, sns_subscriber,
};
use super::budget_action_lifecycle::delete_action;
use super::budget_lifecycle::{caller_account, call, is_error, router};
use super::client_error_envelopes::message;

/// The sentence a method refused for the state of the action it names answers with, for
/// the armed status every row below creates its action in.
const ARMED_REFUSAL: &str = "This method is not allowed during [ActionStatus: Standby]";

/// One row of the armed-action table: what keeps a created action ARMED, and the
/// `CreateBudgetAction` body that makes it so.
struct Armed {
    case: &'static str,
    create: Value,
}

/// The actions an approval has nothing to do for: each is created ARMED
/// (`ActionStatus$STANDBY`), for a different reason the rule covers.
///
/// Every budget these rows hang off reports an `ACTUAL` spend of zero and no forecast at
/// all, so the rows differ along the threshold value (including the published `@range`
/// maximum of `NotificationThreshold`), the threshold type (both values of the modelled
/// `ThresholdType` — `smithy_query(query="shape",shape="ThresholdType")`), the
/// notification type, the approval model, the action type (and so the `Definition` member
/// that type names) and the subscriber set.
fn armed_rows(account_id: &str) -> Vec<Armed> {
    vec![
        Armed {
            case: "a percentage threshold the spend has not reached",
            create: create_body(
                account_id,
                "placeholder",
                "APPLY_IAM_POLICY",
                &iam_definition(),
                "MANUAL",
                "ACTUAL",
                &json!({"ActionThresholdValue": 80.0, "ActionThresholdType": "PERCENTAGE"}),
                &json!([email_subscriber(0)]),
                None,
            ),
        },
        Armed {
            case: "a percentage threshold at the published maximum",
            create: create_body(
                account_id,
                "placeholder",
                "APPLY_SCP_POLICY",
                &definition_for("APPLY_SCP_POLICY"),
                "MANUAL",
                "ACTUAL",
                &json!({
                    "ActionThresholdValue": 15_000_000_000_000.0_f64,
                    "ActionThresholdType": "PERCENTAGE"
                }),
                &json!([email_subscriber(1), sns_subscriber()]),
                None,
            ),
        },
        Armed {
            case: "an absolute threshold the spend has not reached",
            create: create_body(
                account_id,
                "placeholder",
                "RUN_SSM_DOCUMENTS",
                &definition_for("RUN_SSM_DOCUMENTS"),
                "MANUAL",
                "ACTUAL",
                &json!({"ActionThresholdValue": 25.0, "ActionThresholdType": "ABSOLUTE_VALUE"}),
                &json!([email_subscriber(2)]),
                None,
            ),
        },
        Armed {
            case: "a forecast the budget states nothing of",
            create: create_body(
                account_id,
                "placeholder",
                "APPLY_IAM_POLICY",
                &iam_definition(),
                "MANUAL",
                "FORECASTED",
                &json!({"ActionThresholdValue": 0.0, "ActionThresholdType": "PERCENTAGE"}),
                &json!([email_subscriber(3)]),
                None,
            ),
        },
        Armed {
            case: "an action whose execution needs no approval",
            create: create_body(
                account_id,
                "placeholder",
                "APPLY_IAM_POLICY",
                &iam_definition(),
                "AUTOMATIC",
                "ACTUAL",
                &json!({"ActionThresholdValue": 0.0, "ActionThresholdType": "PERCENTAGE"}),
                &json!([email_subscriber(4)]),
                Some(json!([{"Key": "owner", "Value": "fin-ops"}])),
            ),
        },
    ]
}

/// RULE: an armed action has no triggered execution to approve, so the approval is refused
/// for that state, the action still reads back armed afterwards, and the refusal left it
/// deletable — the delete reports the action as it stood and no read locates it after.
#[tokio::test]
async fn an_approval_of_an_armed_action_is_refused_and_leaves_it_deletable() {
    let router = router();
    let account_id = caller_account();
    for (index, row) in armed_rows(&account_id).into_iter().enumerate() {
        let Armed { case, create } = row;
        let budget_name = format!("armed-{index}");
        seed_budget(&router, &account_id, &budget_name).await;
        let mut create = create;
        create["BudgetName"] = json!(&budget_name);
        let (http, body) = call(&router, "CreateBudgetAction", &create).await;
        assert_eq!(
            http, 200,
            "case {case:?}: expected CreateBudgetAction to answer 200, got {http} {body}"
        );
        let action_id = body["ActionId"].as_str().unwrap_or_default().to_string();
        let before = read_status(&router, &account_id, &budget_name, &action_id).await;
        assert_eq!(
            before, ACTION_STATUS_STANDBY,
            "case {case:?}: expected the created action to be armed, got {before:?}"
        );

        let (http, body) = execute_action(
            &router,
            &account_id,
            &budget_name,
            &action_id,
            EXECUTION_TYPE_APPROVE,
        )
        .await;
        assert_eq!(
            http, 400,
            "case {case:?}: expected the approval to be refused at HTTP 400, got {http} {body}"
        );
        assert!(
            is_error(&body, "ResourceLockedException"),
            "case {case:?}: expected ResourceLockedException, got {body}"
        );
        assert_eq!(
            message(&body),
            ARMED_REFUSAL,
            "case {case:?}: expected the refusal to name the armed status, got {body}"
        );
        // The refused call changed nothing: the action is still armed, and still
        // deletable — a refusal must not park it in a status it can never leave.
        let after = read_status(&router, &account_id, &budget_name, &action_id).await;
        assert_eq!(
            after, ACTION_STATUS_STANDBY,
            "case {case:?}: expected the refused approval to leave the action armed, got \
             {after:?}"
        );
        let (http, body) = delete_action(&router, &account_id, &budget_name, &action_id).await;
        assert_eq!(
            http, 200,
            "case {case:?}: expected the delete to be served, got {http} {body}"
        );
        assert_eq!(
            body["Action"]["ActionId"].as_str(),
            Some(action_id.as_str()),
            "case {case:?}: expected the delete to report the action it removed, got {body}"
        );
        assert_eq!(
            body["Action"]["Status"].as_str(),
            Some(ACTION_STATUS_STANDBY),
            "case {case:?}: expected the removed action to carry the status it had, got {body}"
        );
        let (http, body) = call(
            &router,
            "DescribeBudgetAction",
            &json!({
                "AccountId": &account_id,
                "BudgetName": &budget_name,
                "ActionId": &action_id
            }),
        )
        .await;
        assert_eq!(
            http, 400,
            "case {case:?}: expected the deleted action to be gone, got {http} {body}"
        );
        assert!(
            is_error(&body, "NotFoundException"),
            "case {case:?}: expected NotFoundException after the delete, got {body}"
        );
    }
}

/// RULE, the other side: the gate is the STATUS, not the method — the same approval of an
/// action that IS waiting for one is served and takes the action into the execution it
/// approved.
#[tokio::test]
async fn the_same_approval_of_an_action_waiting_for_one_is_served() {
    let router = router();
    let account_id = caller_account();
    let budget_name = "waiting-for-approval";
    seed_budget(&router, &account_id, budget_name).await;
    let action_id = seed_pending_action(&router, &account_id, budget_name).await;
    let before = read_status(&router, &account_id, budget_name, &action_id).await;
    assert_eq!(
        before, ACTION_STATUS_PENDING,
        "expected an action whose threshold triggered and whose execution needs approving \
         to be waiting for it, got {before:?}"
    );

    let (http, body) = execute_action(
        &router,
        &account_id,
        budget_name,
        &action_id,
        EXECUTION_TYPE_APPROVE,
    )
    .await;
    assert_eq!(
        http, 200,
        "expected the approval to be served, got {http} {body}"
    );
    let after = read_status(&router, &account_id, budget_name, &action_id).await;
    assert_eq!(
        after, ACTION_STATUS_EXECUTION_IN_PROGRESS,
        "expected the approved execution to be under way, got {after:?}"
    );
}
