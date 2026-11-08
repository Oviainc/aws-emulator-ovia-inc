// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the END of a budget action's lifecycle: `DeleteBudgetAction`,
//! driven over the wire through the same `MultiServiceRouter` the production router
//! serves from. The create/read/execute half of an action's life is asserted by the
//! sibling `budget_action_execution` suite, whose request builders and readers this one
//! reuses rather than re-typing, so both suites drive the service identically.
//!
//! The RULES asserted here:
//!
//! 1. The delete removes exactly the action `(AccountId, BudgetName, ActionId)` names,
//!    and reports it as it stood — the same `Action` a read reported an instant before —
//!    after which no read locates it and a second delete is refused.
//! 2. The budget's other actions keep their value and their place, which is the order a
//!    paginated read indexes into.
//! 3. An action is a resource OF one budget of one account, so another account's budget,
//!    another budget of the same account, an id another budget's action carries, or an id
//!    no action carries locates nothing and removes nothing.
//! 4. An action whose work this service started and has not settled yet is the locked
//!    resource `ResourceLockedException` names, so the delete is refused until the clock
//!    settles it — and then it is accepted. Every settled status, in turn, locks nothing.
//!
//! Each table spans those classes rather than one request: its rows differ along the
//! action type (and with it the `Definition` member the type names), the approval model,
//! the notification type, the threshold value and type, the subscriber count and types,
//! the presence of `ResourceTags`, the position of the action inside its budget, and —
//! for the lock — every `ActionStatus` the model lists.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;
use std::time::Duration;

/// The enum catalogs are not re-typed here: these are the model-derived constants the
/// scaffold generated into `src/ops/types/shared_types.rs`, so a table built from them
/// cannot drift from the model.
use service_com_amazonaws_budgets_awsbudgetservicegateway::ops::types::{
    ACTION_STATUS_VALUES, ACTION_TYPE_VALUES, APPROVAL_MODEL_VALUES, NOTIFICATION_TYPE_VALUES,
    THRESHOLD_TYPE_VALUES,
};
use service_com_amazonaws_budgets_awsbudgetservicegateway::state::{
    action_is_in_progress, ComAmazonawsBudgetsAwsbudgetservicegatewayState, ACTION_SETTLES_AFTER,
    ACTION_STATUS_EXECUTION_IN_PROGRESS, ACTION_STATUS_EXECUTION_SUCCESS, ACTION_STATUS_PENDING,
    ACTION_STATUS_RESET_IN_PROGRESS, ACTION_STATUS_REVERSE_IN_PROGRESS,
    EXECUTION_TYPE_APPROVE, EXECUTION_TYPE_RESET, EXECUTION_TYPE_REVERSE,
};

use super::budget_action_execution::{
    create_body, created_status, definition_for, describe_action, email_subscriber, execute_action,
    is_action_id, seed_action, seed_budget, seed_status, sns_subscriber,
};
use super::budget_lifecycle::{caller_account, call, is_error, router, router_with_engine, same};

/// An `ActionId` that is well-formed but names no action: inside `ActionId`'s modelled
/// `@pattern`, so it reaches the handler instead of being refused at the boundary.
const ABSENT_ACTION_ID: &str = "11111111-2222-3333-4444-555555555555";

/// One row of the action table: a case name and the `CreateBudgetAction` body the action
/// under test is created with.
struct Row {
    case: String,
    create: Value,
}

/// The action table, for actions of `budget_name`. Its rows differ along every member
/// the delete generalises over: the action type (and with it the `Definition` member
/// that type names), the approval model, the notification type, the threshold type, the
/// threshold value (each published `@range` bound of `NotificationThreshold` — 0 and
/// 15000000000000 — and a fraction between them), the subscriber count and types, and
/// whether `ResourceTags` were supplied.
fn action_rows(account_id: &str, budget_name: &str) -> Vec<Row> {
    let thresholds = [0.0_f64, 42.5, 15_000_000_000_000.0];
    let mut rows = Vec::new();
    for (type_index, action_type) in ACTION_TYPE_VALUES.iter().enumerate() {
        for (model_index, approval_model) in APPROVAL_MODEL_VALUES.iter().enumerate() {
            let position = type_index * APPROVAL_MODEL_VALUES.len() + model_index;
            let notification_type =
                NOTIFICATION_TYPE_VALUES[position % NOTIFICATION_TYPE_VALUES.len()];
            let threshold_type = THRESHOLD_TYPE_VALUES[position % THRESHOLD_TYPE_VALUES.len()];
            let mut subscribers: Vec<Value> = (0..=position % 3).map(email_subscriber).collect();
            if !position.is_multiple_of(2) {
                subscribers.push(sns_subscriber());
            }
            let tags = position
                .is_multiple_of(3)
                .then(|| json!([{"Key": format!("owner-{position}"), "Value": "fin-ops"}]));
            rows.push(Row {
                case: format!(
                    "{action_type} / {approval_model} / {notification_type} / \
                     {threshold_type} with {} subscriber(s){}",
                    subscribers.len(),
                    if tags.is_some() { " and tags" } else { "" }
                ),
                create: create_body(
                    account_id,
                    budget_name,
                    action_type,
                    &definition_for(action_type),
                    approval_model,
                    notification_type,
                    &json!({
                        "ActionThresholdValue": thresholds[position % thresholds.len()],
                        "ActionThresholdType": threshold_type
                    }),
                    &Value::Array(subscribers),
                    tags,
                ),
            });
        }
    }
    rows
}

/// Create one action from `create` and answer the `ActionId` the service minted for it.
async fn create_action(router: &MultiServiceRouter, create: &Value, case: &str) -> String {
    let (status, body) = call(router, "CreateBudgetAction", create).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateBudgetAction to answer 200, got {status} {body}"
    );
    let action_id = body["ActionId"].as_str().unwrap_or_default().to_string();
    assert!(
        is_action_id(&action_id),
        "case {case:?}: expected a minted UUID ActionId, got {action_id:?}"
    );
    action_id
}

/// One `DeleteBudgetAction` call.
///
/// `pub(crate)` so the sibling suites in this test target delete an action the same way.
pub(crate) async fn delete_action(
    router: &MultiServiceRouter,
    account_id: &str,
    budget_name: &str,
    action_id: &str,
) -> (u16, Value) {
    call(
        router,
        "DeleteBudgetAction",
        &json!({
            "AccountId": account_id,
            "BudgetName": budget_name,
            "ActionId": action_id
        }),
    )
    .await
}

/// The `ActionId`s the STORE holds for a budget, in order, each with the tags held
/// alongside it. Read from the state struct directly, so what the delete did to the
/// store is asserted independently of any read operation's own projection.
fn stored_actions(
    engine: &Arc<engine::InMemoryEngine>,
    account_id: &str,
    budget_name: &str,
) -> Vec<(String, Vec<(String, String)>)> {
    let state = engine.state::<ComAmazonawsBudgetsAwsbudgetservicegatewayState>();
    let store = state.lock();
    store
        .budgets
        .get(&(account_id.to_string(), budget_name.to_string()))
        .expect("the budget under test is stored")
        .actions
        .iter()
        .map(|held| {
            (
                held.action.action_id.clone(),
                held.tags
                    .iter()
                    .map(|tag| (tag.key.clone(), tag.value.clone()))
                    .collect(),
            )
        })
        .collect()
}

/// Just the stored `ActionId`s, in order.
fn stored_action_ids(
    engine: &Arc<engine::InMemoryEngine>,
    account_id: &str,
    budget_name: &str,
) -> Vec<String> {
    stored_actions(engine, account_id, budget_name)
        .into_iter()
        .map(|(action_id, _)| action_id)
        .collect()
}

/// The `ResourceTags` a row supplied, as the store holds them.
fn supplied_tags(create: &Value) -> Vec<(String, String)> {
    create
        .get("ResourceTags")
        .and_then(Value::as_array)
        .map(|tags| {
            tags.iter()
                .map(|tag| {
                    (
                        tag["Key"].as_str().unwrap_or_default().to_string(),
                        tag["Value"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// RULE 1: the delete removes the action its id names and reports it as it stood — the
/// same `Action` `DescribeBudgetAction` reported an instant earlier, with the minted
/// `ActionId` and the status the create left it in. Afterwards no read
/// locates it and a second delete is refused with `NotFoundException`.
///
/// Every row of the action table is one case, so the rule holds across action types and
/// their definitions, approval models, notification types, thresholds, subscriber sets
/// and tagged as well as untagged actions.
#[tokio::test]
async fn deleting_an_action_reports_what_went_away_and_leaves_nothing_behind() {
    let (router, engine) = router_with_engine();
    let account_id = caller_account();

    for (position, row) in action_rows(&account_id, "placeholder").iter().enumerate() {
        let budget_name = format!("deleted-{position}");
        seed_budget(&router, &account_id, &budget_name).await;
        let case = &row.case;
        let mut create = row.create.clone();
        create["BudgetName"] = json!(&budget_name);

        let action_id = create_action(&router, &create, case).await;
        let (status, read) = describe_action(&router, &account_id, &budget_name, &action_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the created action to be readable, got {status} {read}"
        );
        assert!(
            same(&read["Action"]["Status"], &json!(created_status(&create))),
            "case {case:?}: expected a created action to carry the status its trigger \
             threshold gives it, got {}",
            read["Action"]["Status"]
        );
        // The tags supplied with the action are persisted with it, which
        // `ListTagsForResource` reports (asserted by the `resource_tags` suite).
        assert_eq!(
            stored_actions(&engine, &account_id, &budget_name),
            vec![(action_id.clone(), supplied_tags(&create))],
            "case {case:?}: expected the store to hold exactly the created action and its tags"
        );

        let (status, body) = delete_action(&router, &account_id, &budget_name, &action_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DeleteBudgetAction to answer 200, got {status} {body}"
        );
        assert!(
            same(&body["AccountId"], &json!(&account_id))
                && same(&body["BudgetName"], &json!(&budget_name)),
            "case {case:?}: expected the response to name the account and the budget, got {body}"
        );
        assert!(
            same(&body["Action"], &read["Action"]),
            "case {case:?}: expected the deleted action to be the one a read reported, {}, got {}",
            read["Action"],
            body["Action"]
        );

        assert!(
            stored_action_ids(&engine, &account_id, &budget_name).is_empty(),
            "case {case:?}: expected the store to hold no action after the delete, got {:?}",
            stored_action_ids(&engine, &account_id, &budget_name)
        );
        let (status, body) = describe_action(&router, &account_id, &budget_name, &action_id).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the deleted action to be unreadable, got {status} {body}"
        );
        assert!(
            is_error(&body, "NotFoundException"),
            "case {case:?}: expected NotFoundException from the read, got {body}"
        );
        let (status, body) = delete_action(&router, &account_id, &budget_name, &action_id).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a second delete to be refused, got {status} {body}"
        );
        assert!(
            is_error(&body, "NotFoundException"),
            "case {case:?}: expected NotFoundException on a second delete, got {body}"
        );
    }
}

/// RULE 2: only the named action goes — every other action of that budget keeps its
/// value and its place, and stays readable.
///
/// The rows are each position of the action table in turn, so the rule is asserted for
/// the first, the middle and the last action of a budget holding several.
#[tokio::test]
async fn deleting_an_action_removes_only_the_one_it_names() {
    let (router, engine) = router_with_engine();
    let account_id = caller_account();
    let table = action_rows(&account_id, "placeholder");

    for target in 0..table.len() {
        let budget_name = format!("many-{target}");
        seed_budget(&router, &account_id, &budget_name).await;
        let mut ids = Vec::new();
        for row in &table {
            let mut create = row.create.clone();
            create["BudgetName"] = json!(&budget_name);
            ids.push(create_action(&router, &create, &row.case).await);
        }
        let case = format!("target {target} of {}: {}", table.len(), table[target].case);
        assert_eq!(
            stored_action_ids(&engine, &account_id, &budget_name),
            ids,
            "case {case:?}: expected the store to hold every created action in order"
        );

        let (status, body) = delete_action(&router, &account_id, &budget_name, &ids[target]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the delete to answer 200, got {status} {body}"
        );
        assert!(
            same(&body["Action"]["ActionId"], &json!(&ids[target])),
            "case {case:?}: expected the response to carry the deleted ActionId {}, got {body}",
            ids[target]
        );

        let expected: Vec<String> = ids
            .iter()
            .enumerate()
            .filter(|(position, _)| *position != target)
            .map(|(_, id)| id.clone())
            .collect();
        let remaining = stored_action_ids(&engine, &account_id, &budget_name);
        assert_eq!(
            remaining, expected,
            "case {case:?}: expected only the named action to go, leaving {expected:?}"
        );
        for id in &expected {
            let (status, body) = describe_action(&router, &account_id, &budget_name, id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected action {id} to stay readable, got {status} {body}"
            );
        }
    }
}

/// RULE 3: an action is a resource OF one budget of the account, named by its
/// `BudgetName` and its `ActionId`. A request that gets either wrong locates nothing: it
/// is refused with `NotFoundException` and removes nothing. A `BudgetName` outside its
/// modelled `@pattern` is the request-validation refusal it is.
#[tokio::test]
async fn an_action_is_reached_only_through_its_budget_and_its_id() {
    let (router, engine) = router_with_engine();
    let owner = caller_account();

    seed_budget(&router, &owner, "owning").await;
    seed_budget(&router, &owner, "sibling").await;
    let owned = seed_action(&router, &owner, "owning").await;
    let sibling = seed_action(&router, &owner, "sibling").await;

    for (case, budget_name, action_id) in [
        ("a budget the account does not hold", "absent", &owned),
        ("the owning budget's name in another case", "OWNING", &owned),
        ("another budget of the same account", "sibling", &owned),
        (
            "an id no action carries",
            "owning",
            &ABSENT_ACTION_ID.to_string(),
        ),
        ("another budget's action id", "owning", &sibling),
    ] {
        let (status, body) = delete_action(&router, &owner, budget_name, action_id).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the delete to be refused, got {status} {body}"
        );
        assert!(
            is_error(&body, "NotFoundException"),
            "case {case:?}: expected NotFoundException, got {body}"
        );
    }

    // Not one of those refusals removed anything.
    for (budget_name, expected) in [("owning", &owned), ("sibling", &sibling)] {
        let held = stored_action_ids(&engine, &owner, budget_name);
        assert_eq!(
            held,
            vec![expected.clone()],
            "case {budget_name:?}: expected the refusals to remove nothing"
        );
    }

    // A name outside `BudgetName`'s pattern is a validation refusal: the framework's
    // regex engine cannot compile that pattern, so this handler answers it.
    for (case, budget_name) in [
        ("a colon", "a:b"),
        ("a backslash", "a\\b"),
        ("the /action/ substring", "a/action/b"),
    ] {
        let (status, body) = delete_action(&router, &owner, budget_name, &owned).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {body}"
        );
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

    // And the action all three DO name is still deletable after the near misses.
    let (status, body) = delete_action(&router, &owner, "owning", &owned).await;
    assert_eq!(
        status, 200,
        "expected the owner's own action to delete after the near misses, got {status} {body}"
    );
}

/// RULE 4: an action whose work this service started and has not settled yet is the
/// locked resource `ResourceLockedException` names ("the server rejected that particular
/// method for the requested resource"), so the delete is refused while the work is in
/// flight and accepted once the clock has settled it — with no call in between making
/// that settle.
///
/// The rows are every `*_IN_PROGRESS` status the model lists, each reached by the
/// execution type that starts that work from the settled status it is accepted from.
#[tokio::test]
async fn an_action_whose_work_is_in_flight_is_locked_against_deletion() {
    let (router, engine) = router_with_engine();
    let account_id = caller_account();

    for (from_status, execution_type, in_progress) in [
        (
            ACTION_STATUS_PENDING,
            EXECUTION_TYPE_APPROVE,
            ACTION_STATUS_EXECUTION_IN_PROGRESS,
        ),
        (
            ACTION_STATUS_EXECUTION_SUCCESS,
            EXECUTION_TYPE_REVERSE,
            ACTION_STATUS_REVERSE_IN_PROGRESS,
        ),
        (
            ACTION_STATUS_EXECUTION_SUCCESS,
            EXECUTION_TYPE_RESET,
            ACTION_STATUS_RESET_IN_PROGRESS,
        ),
    ] {
        let budget_name = format!("locked-{in_progress}");
        seed_budget(&router, &account_id, &budget_name).await;
        let action_id = seed_action(&router, &account_id, &budget_name).await;
        // The settled status this execution type is accepted from.
        seed_status(&engine, &account_id, &budget_name, &action_id, from_status);
        let case = format!("{execution_type} from {from_status} leaves {in_progress}");

        let (status, body) = execute_action(
            &router,
            &account_id,
            &budget_name,
            &action_id,
            execution_type,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the execution to be accepted, got {status} {body}"
        );

        // Straight after the call the work is still in flight, so the delete is refused.
        let (status, body) = delete_action(&router, &account_id, &budget_name, &action_id).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the delete to be refused while in flight, got {status} {body}"
        );
        assert!(
            is_error(&body, "ResourceLockedException"),
            "case {case:?}: expected ResourceLockedException, got {body}"
        );
        assert_eq!(
            stored_action_ids(&engine, &account_id, &budget_name),
            vec![action_id.clone()],
            "case {case:?}: expected the refused delete to remove nothing"
        );
        let (status, read) = describe_action(&router, &account_id, &budget_name, &action_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the locked action to stay readable, got {status} {read}"
        );
        assert!(
            same(&read["Action"]["Status"], &json!(in_progress)),
            "case {case:?}: expected the action to be {in_progress}, got {}",
            read["Action"]["Status"]
        );

        // The clock settles the work with no further call, and the settled action is
        // deletable.
        tokio::time::sleep(ACTION_SETTLES_AFTER + Duration::from_millis(250)).await;
        let (status, body) = delete_action(&router, &account_id, &budget_name, &action_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the settled action to delete, got {status} {body}"
        );
        assert!(
            stored_action_ids(&engine, &account_id, &budget_name).is_empty(),
            "case {case:?}: expected the settled action to be gone"
        );
    }
}

/// RULE 4, the other half: a status with no outstanding work of this service's never
/// locks the delete. The rows are derived from the model's own `ActionStatus` catalog —
/// every value that is not in progress — and seeded directly, so the statuses no request
/// here mints (the `*_FAILURE` ones, since nothing here fails an action's work) are
/// covered too.
#[tokio::test]
async fn a_settled_action_is_never_locked_against_deletion() {
    let (router, engine) = router_with_engine();
    let account_id = caller_account();

    let settled: Vec<&str> = ACTION_STATUS_VALUES
        .iter()
        .copied()
        .filter(|status| !action_is_in_progress(status))
        .collect();
    assert!(
        !settled.is_empty(),
        "expected the model's ActionStatus catalog to carry settled statuses, got \
         {ACTION_STATUS_VALUES:?}"
    );

    for status_value in settled {
        let budget_name = format!("settled-{status_value}");
        seed_budget(&router, &account_id, &budget_name).await;
        let action_id = seed_action(&router, &account_id, &budget_name).await;
        seed_status(&engine, &account_id, &budget_name, &action_id, status_value);

        let (status, body) = delete_action(&router, &account_id, &budget_name, &action_id).await;
        assert_eq!(
            status, 200,
            "case {status_value:?}: expected a settled action to delete, got {status} {body}"
        );
        assert!(
            same(&body["Action"]["Status"], &json!(status_value)),
            "case {status_value:?}: expected the response to report the status the action \
             carried, got {}",
            body["Action"]["Status"]
        );
        assert!(
            stored_action_ids(&engine, &account_id, &budget_name).is_empty(),
            "case {status_value:?}: expected the action to be gone"
        );
    }
}

/// RULE 3, the budget half on its own: a delete naming a budget nobody created locates
/// nothing, so it is refused with `NotFoundException`.
#[tokio::test]
async fn deleting_from_a_budget_that_does_not_exist_is_not_found() {
    let router = router();
    let account_id = caller_account();

    let (status, body) =
        delete_action(&router, &account_id, "never-created", ABSENT_ACTION_ID).await;
    assert_eq!(
        status, 400,
        "expected the delete to be refused when no budget exists, got {status} {body}"
    );
    assert!(
        is_error(&body, "NotFoundException"),
        "expected NotFoundException, got {body}"
    );
}
