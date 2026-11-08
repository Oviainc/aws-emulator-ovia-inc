// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for `DescribeBudgetActionsForBudget`, driven over the wire through
//! the same `MultiServiceRouter` the production router serves from.
//!
//! The RULES asserted here:
//!
//! 1. The list reports every action the named budget holds, oldest first (the order they
//!    were created in), each exactly as `DescribeBudgetAction` reports that action — and
//!    a budget holding none is an empty list, not a refusal.
//! 2. `MaxResults` splits that order into pages, each continuing exactly where the
//!    previous `NextToken` left off, with a token returned only while actions remain, so
//!    walking the pages yields every action once whatever the page size.
//! 3. An action is a resource OF one budget of one account, so a list names only that
//!    budget's actions, and a budget the account does not hold is a `NotFoundException`.
//! 4. A `NextToken` this list never minted is an `InvalidNextTokenException`, and
//!    `MaxResults` outside its published `@range` is refused at request validation.
//! 5. The statuses listed are the ones the CLOCK already owed: an action whose work is
//!    in flight is listed in its `*_IN_PROGRESS` status and, once the settle falls due,
//!    in its settled one — with no call in between making that move — and repeating the
//!    list changes nothing a later caller sees.
//!
//! Each table spans those classes rather than one request: its rows differ along the
//! action type (and with it the `Definition` member the type names), the approval model,
//! the notification type, the threshold value and type, the subscriber count and types,
//! the presence of `ResourceTags`, the number of actions on the budget, and the page
//! size.
//!
//! The harness (`router`, `call`, `is_error`, `account`, `same`) and the action builders
//! are the sibling suites', so every suite in this target drives the service the same
//! way.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::time::Duration;

/// The enum catalogs are not re-typed here: these are the model-derived constants the
/// scaffold generated into `src/ops/types/shared_types.rs`, so a table built from them
/// cannot drift from the model.
use service_com_amazonaws_budgets_awsbudgetservicegateway::ops::types::{
    ACTION_TYPE_VALUES, APPROVAL_MODEL_VALUES, NOTIFICATION_TYPE_VALUES, THRESHOLD_TYPE_VALUES,
};
use service_com_amazonaws_budgets_awsbudgetservicegateway::state::{
    ACTION_SETTLES_AFTER, ACTION_STATUS_EXECUTION_IN_PROGRESS, ACTION_STATUS_EXECUTION_SUCCESS,
    EXECUTION_TYPE_APPROVE, MAX_ACTIONS_PER_BUDGET,
};

use super::budget_action_execution::{
    create_body, created_status, definition_for, describe_action, email_subscriber, execute_action,
    is_action_id, seed_action, seed_budget, sns_subscriber,
};
use super::budget_lifecycle::{caller_account, call, is_error, router, same};

/// One row of the action table: a case name and the `CreateBudgetAction` body it creates.
///
/// `pub(crate)` so the sibling account-wide suite spans the same action class rather
/// than re-typing the table.
pub(crate) struct Row {
    pub(crate) case: String,
    pub(crate) create: Value,
}

/// The action table, for actions of one budget. Rows differ along every member the list
/// generalises over: the action type (and with it the `Definition` member that type
/// names), the approval model, the notification type, the threshold type, the threshold
/// value (each published `@range` bound of `NotificationThreshold` — 0 and
/// 15000000000000 — and a fraction between them), the subscriber count and types, and
/// whether `ResourceTags` were supplied.
///
/// Bounds and value sets come from the model
/// (`smithy_query(query="shape",shape="Action,NotificationThreshold")`).
pub(crate) fn action_rows(account_id: &str, budget_name: &str) -> Vec<Row> {
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

/// One `DescribeBudgetActionsForBudget` call.
pub(crate) async fn list_actions(
    router: &MultiServiceRouter,
    account_id: &str,
    budget_name: &str,
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> (u16, Value) {
    let mut request = json!({"AccountId": account_id, "BudgetName": budget_name});
    if let Some(max) = max_results {
        request["MaxResults"] = json!(max);
    }
    if let Some(token) = next_token {
        request["NextToken"] = json!(token);
    }
    call(router, "DescribeBudgetActionsForBudget", &request).await
}

/// Every `Action` one unpaginated list reports, failing the test if the read was refused.
async fn listed_actions(
    router: &MultiServiceRouter,
    account_id: &str,
    budget_name: &str,
) -> Vec<Value> {
    let (status, body) = list_actions(router, account_id, budget_name, None, None).await;
    assert_eq!(
        status, 200,
        "expected DescribeBudgetActionsForBudget({budget_name}) to answer 200, got {status} {body}"
    );
    body["Actions"].as_array().cloned().unwrap_or_default()
}

/// The `ActionId`s of `actions`, in the order they were served.
pub(crate) fn ids_of(actions: &[Value]) -> Vec<String> {
    actions
        .iter()
        .map(|action| action["ActionId"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Create one action from `create` and answer the `ActionId` the service minted for it.
pub(crate) async fn create_action(
    router: &MultiServiceRouter,
    create: &Value,
    case: &str,
) -> String {
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

/// Walk every page of the list with the given page size and return the actions in the
/// order they were served, asserting that no page exceeds `max_results` and that a token
/// is handed back only after a non-empty page.
async fn walk_action_pages(
    router: &MultiServiceRouter,
    account_id: &str,
    budget_name: &str,
    max_results: Option<i32>,
    total: usize,
) -> Vec<Value> {
    let case = format!("{budget_name}: total={total} max={max_results:?}");
    let mut served = Vec::new();
    let mut next: Option<String> = None;
    // One more request than there can be pages, so a token that never clears is a
    // failed assertion rather than a hang.
    for _ in 0..=total + 1 {
        let (status, body) =
            list_actions(router, account_id, budget_name, max_results, next.as_deref()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body}"
        );
        let page = body["Actions"].as_array().cloned().unwrap_or_default();
        if let Some(max) = max_results {
            assert!(
                i32::try_from(page.len()).is_ok_and(|held| held <= max),
                "case {case:?}: a page held {} actions, got {body}",
                page.len()
            );
        }
        served.extend(page.iter().cloned());
        next = body
            .get("NextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        if next.is_none() {
            return served;
        }
        assert!(
            !page.is_empty(),
            "case {case:?}: a NextToken was returned after an empty page, got {body}"
        );
    }
    panic!("case {case:?}: pagination never finished");
}

/// RULE 1: the list reports every action of the named budget, oldest first, each exactly
/// as `DescribeBudgetAction` reports that same action — so the list is the store's own
/// view of the budget's actions rather than a separate projection of it.
///
/// Every row of the action table is created in turn, and the list is asserted after each
/// creation, so the rule holds for a budget holding one action and for one holding every
/// row, across action types and their definitions, approval models, notification types,
/// thresholds, subscriber sets, and tagged as well as untagged actions.
#[tokio::test]
async fn the_list_reports_every_action_of_the_budget_as_a_read_reports_it() {
    let router = router();
    let account_id = caller_account();
    let budget_name = "listed";
    seed_budget(&router, &account_id, budget_name).await;

    let table = action_rows(&account_id, budget_name);
    assert!(
        table.len() <= MAX_ACTIONS_PER_BUDGET,
        "the action table must fit one budget's {MAX_ACTIONS_PER_BUDGET}-action ceiling, \
         got {} rows",
        table.len()
    );

    let mut created: Vec<String> = Vec::new();
    for row in &table {
        let case = &row.case;
        created.push(create_action(&router, &row.create, case).await);

        let actions = listed_actions(&router, &account_id, budget_name).await;
        assert_eq!(
            ids_of(&actions),
            created,
            "case {case:?}: expected the list to report every created action in creation \
             order, got {:?}",
            ids_of(&actions)
        );
        for (position, action) in actions.iter().enumerate() {
            let action_id = &created[position];
            let (status, read) =
                describe_action(&router, &account_id, budget_name, action_id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected action {action_id} to be readable, got {status} {read}"
            );
            assert!(
                same(action, &read["Action"]),
                "case {case:?}: expected the listed action at position {position} to be the \
                 one a read reports, {}, got {action}",
                read["Action"]
            );
            assert!(
                same(&action["Status"], &json!(created_status(action))),
                "case {case:?}: expected a created action to be listed with the status its \
                 trigger threshold gives it, got {}",
                action["Status"]
            );
        }
    }
}

/// RULE 1, the empty half: a budget that holds no actions is served as an empty list and
/// HTTP 200 — an absent resource is the budget, not the list. The rows differ along how
/// the budget got to zero actions: never created one, and had its only action deleted.
#[tokio::test]
async fn a_budget_holding_no_actions_is_served_as_an_empty_list() {
    let router = router();
    let account_id = caller_account();

    seed_budget(&router, &account_id, "never-had-one").await;
    seed_budget(&router, &account_id, "had-one").await;
    let action_id = seed_action(&router, &account_id, "had-one").await;
    let (status, body) = call(
        &router,
        "DeleteBudgetAction",
        &json!({
            "AccountId": &account_id,
            "BudgetName": "had-one",
            "ActionId": &action_id
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: expected the action to delete, got {status} {body}"
    );

    for (case, budget_name) in [
        ("a budget that never held an action", "never-had-one"),
        ("a budget whose only action was deleted", "had-one"),
    ] {
        let (status, body) = list_actions(&router, &account_id, budget_name, None, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body}"
        );
        assert_eq!(
            body["Actions"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: expected an empty Actions list, got {body}"
        );
        assert!(
            body.get("NextToken").is_none_or(Value::is_null),
            "case {case:?}: expected no NextToken for an empty list, got {body}"
        );
    }
}

/// RULE 2: `MaxResults` splits the budget's actions into pages of at most that many,
/// each page continuing where the previous `NextToken` left off, and a token is returned
/// only while actions remain — so walking the pages yields every action exactly once, in
/// creation order, whatever the page size.
///
/// The rows are the cross product of the number of actions on the budget (none, one, a
/// few, every row of the table) and the page size (absent, each published `@range` bound
/// of `MaxResults` — 1 and 100 — and sizes above, below and equal to the total).
#[tokio::test]
async fn max_results_partitions_the_budgets_actions_in_creation_order() {
    let router = router();
    let account_id = caller_account();
    let table = action_rows(&account_id, "placeholder");

    for total in [0_usize, 1, 2, table.len()] {
        let budget_name = format!("paged-{total}");
        seed_budget(&router, &account_id, &budget_name).await;
        let mut created = Vec::new();
        for row in table.iter().take(total) {
            let mut create = row.create.clone();
            create["BudgetName"] = json!(&budget_name);
            created.push(create_action(&router, &create, &row.case).await);
        }

        for max_results in [None, Some(1), Some(2), Some(3), Some(100)] {
            let served =
                walk_action_pages(&router, &account_id, &budget_name, max_results, total).await;
            assert_eq!(
                ids_of(&served),
                created,
                "case total={total} max={max_results:?}: expected every action exactly once \
                 in creation order, got {:?}",
                ids_of(&served)
            );
        }
    }
}

/// RULE 3: an action is a resource OF one budget of the account, so the list names only
/// the actions of the budget it names — another budget of the same account reports its
/// own — and a name the account holds no budget under is refused with
/// `NotFoundException` (HTTP 400, the status this service sends for it). A `BudgetName`
/// outside its modelled `@pattern` is the request-validation refusal it is.
#[tokio::test]
async fn actions_are_listed_only_through_the_budget_that_owns_them() {
    let router = router();
    let owner = caller_account();

    seed_budget(&router, &owner, "owning").await;
    seed_budget(&router, &owner, "sibling").await;
    let owned = seed_action(&router, &owner, "owning").await;
    let sibling = seed_action(&router, &owner, "sibling").await;

    for (case, budget_name, expected) in [
        ("the owning budget", "owning", &owned),
        ("another budget of the same account", "sibling", &sibling),
    ] {
        let actions = listed_actions(&router, &owner, budget_name).await;
        assert_eq!(
            ids_of(&actions),
            vec![expected.clone()],
            "case {case:?}: expected the list to report only that budget's own action"
        );
    }

    for (case, budget_name) in [
        ("a budget nobody created", "never-created"),
        ("a stored name in another case", "OWNING"),
    ] {
        let (status, body) = list_actions(&router, &owner, budget_name, None, None).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the list to be refused, got {status} {body}"
        );
        assert!(
            is_error(&body, "NotFoundException"),
            "case {case:?}: expected NotFoundException, got {body}"
        );
    }

    // A name outside `BudgetName`'s pattern is a validation refusal: the framework's
    // regex engine cannot compile that pattern, so this handler answers it.
    for (case, budget_name) in [
        ("a colon", "a:b"),
        ("a backslash", "a\\b"),
        ("the /action/ substring", "a/action/b"),
    ] {
        let (status, body) = list_actions(&router, &owner, budget_name, None, None).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {body}"
        );
        assert!(
            is_error(&body, "ValidationException"),
            "case {case:?}: expected ValidationException, got {body}"
        );
    }
}

/// RULE 4: a `NextToken` this list never minted does not decode, so it is refused with
/// `InvalidNextTokenException` instead of silently serving the first page; and
/// `MaxResults` outside its published `@range(min: 1, max: 100)` is refused at request
/// validation, while both published bounds are accepted.
#[tokio::test]
async fn a_token_this_list_never_minted_and_a_page_size_out_of_range_are_refused() {
    let router = router();
    let account_id = caller_account();
    seed_budget(&router, &account_id, "tokens").await;
    seed_action(&router, &account_id, "tokens").await;

    // Tokens differing along form: empty, not hex, odd-length hex, hex of a payload this
    // service does not mint, a real token with a nibble removed, and a decodable token
    // whose offset is past the end of the list (stale or tampered with).
    let bad_tokens: Vec<String> = vec![
        String::new(),
        "not-a-token".to_string(),
        "zz".to_string(),
        hex::encode("offset:1"),
        hex::encode("other-service:offset:1"),
        hex::encode("budgets:offset:not-a-number"),
        hex::encode("budgets:offset:1")[1..].to_string(),
        hex::encode("budgets:offset:999999"),
    ];
    for token in &bad_tokens {
        let (status, body) =
            list_actions(&router, &account_id, "tokens", None, Some(token)).await;
        assert_eq!(
            status, 400,
            "case token={token:?}: expected 400, got {status} {body}"
        );
        assert!(
            is_error(&body, "InvalidNextTokenException"),
            "case token={token:?}: expected InvalidNextTokenException, got {body}"
        );
    }

    // One action and MaxResults=1 leaves nothing to continue, so no token is due.
    let (_, body) = list_actions(&router, &account_id, "tokens", Some(1), None).await;
    assert!(
        body.get("NextToken").is_none_or(Value::is_null),
        "expected no NextToken when the page exhausted the budget, got {body}"
    );

    // `MaxResults` is @range(min: 1, max: 100): one past each published bound.
    for max_results in [0, 101] {
        let (status, body) =
            list_actions(&router, &account_id, "tokens", Some(max_results), None).await;
        assert_eq!(
            status, 400,
            "case MaxResults={max_results}: expected 400, got {status} {body}"
        );
        assert!(
            is_error(&body, "ValidationException"),
            "case MaxResults={max_results}: expected ValidationException, got {body}"
        );
    }
    for max_results in [1, 100] {
        let (status, body) =
            list_actions(&router, &account_id, "tokens", Some(max_results), None).await;
        assert_eq!(
            status, 200,
            "case MaxResults={max_results}: a published bound must be accepted, \
             got {status} {body}"
        );
    }
}

/// RULE 5: the statuses the list reports are the ones the CLOCK already owed. Straight
/// after an execution the action is listed in its `*_IN_PROGRESS` status; once the settle
/// span has passed, the same list reports the settled status with no call in between
/// having made that move. Listing twice changes nothing a later caller sees.
///
/// The rows are the actions of the whole table, each executed in turn, so the rule holds
/// for an in-flight action at every position of a budget holding several while its
/// siblings stay armed.
#[tokio::test]
async fn the_list_reports_the_status_the_clock_owes() {
    let router = router();
    let account_id = caller_account();
    let table = action_rows(&account_id, "placeholder");

    for target in 0..table.len() {
        let budget_name = format!("clocked-{target}");
        seed_budget(&router, &account_id, &budget_name).await;
        let mut created = Vec::new();
        for row in &table {
            let mut create = row.create.clone();
            create["BudgetName"] = json!(&budget_name);
            created.push(create_action(&router, &create, &row.case).await);
        }
        let case = format!("target {target} of {}: {}", table.len(), table[target].case);

        let (status, body) = execute_action(
            &router,
            &account_id,
            &budget_name,
            &created[target],
            EXECUTION_TYPE_APPROVE,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the execution to be accepted, got {status} {body}"
        );

        // In flight: the executed action is listed as such, its siblings as armed.
        let in_flight = listed_actions(&router, &account_id, &budget_name).await;
        assert_eq!(
            ids_of(&in_flight),
            created,
            "case {case:?}: expected the execution to leave the list's order alone"
        );
        for (position, action) in in_flight.iter().enumerate() {
            let expected = if position == target {
                ACTION_STATUS_EXECUTION_IN_PROGRESS
            } else {
                created_status(action)
            };

            assert!(
                same(&action["Status"], &json!(expected)),
                "case {case:?}: expected position {position} to be listed as {expected}, got {}",
                action["Status"]
            );
        }
        // A second list reports the same thing: the read changed nothing.
        let again = listed_actions(&router, &account_id, &budget_name).await;
        assert!(
            same(&Value::Array(again.clone()), &Value::Array(in_flight.clone())),
            "case {case:?}: expected a repeated list to report the same actions, got \
             {again:?} after {in_flight:?}"
        );

        // The clock settles the work with no further call, and the next list says so.
        tokio::time::sleep(ACTION_SETTLES_AFTER + Duration::from_millis(250)).await;
        let settled = listed_actions(&router, &account_id, &budget_name).await;
        for (position, action) in settled.iter().enumerate() {
            let expected = if position == target {
                ACTION_STATUS_EXECUTION_SUCCESS
            } else {
                created_status(action)
            };
            assert!(
                same(&action["Status"], &json!(expected)),
                "case {case:?}: expected position {position} to be listed as {expected} after \
                 the settle, got {}",
                action["Status"]
            );
        }
        // And the single-action read agrees with the list, so neither reports a status
        // the other does not.
        for (position, action_id) in created.iter().enumerate() {
            let (status, read) =
                describe_action(&router, &account_id, &budget_name, action_id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected action {action_id} to stay readable, got {status} {read}"
            );
            assert!(
                same(&settled[position], &read["Action"]),
                "case {case:?}: expected the listed action at position {position} to match the \
                 read, {}, got {}",
                read["Action"],
                settled[position]
            );
        }
    }
}
