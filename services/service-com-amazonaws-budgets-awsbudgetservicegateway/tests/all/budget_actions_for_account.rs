// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for `DescribeBudgetActionsForAccount`, driven over the wire through
//! the same `MultiServiceRouter` the production router serves from.
//!
//! The RULES asserted here:
//!
//! 1. The account-wide list reports every budget action the account holds — the actions
//!    of ALL of its budgets — ordered by the owning `BudgetName` and, within one budget,
//!    by creation order, each exactly as `DescribeBudgetAction` reports that action. An
//!    account holding none is an empty list, not a refusal.
//! 2. It is therefore exactly the per-budget lists concatenated in `BudgetName` order:
//!    the account-wide read and `DescribeBudgetActionsForBudget` cannot disagree about
//!    the same actions.
//! 3. `MaxResults` splits that order into pages, each continuing where the previous
//!    `NextToken` left off, with a token returned only while actions remain, so walking
//!    the pages yields every action once whatever the page size — and a request naming
//!    no `MaxResults` is served at most what the output list holds, with a token for the
//!    rest.
//! 4. A `NextToken` this list never minted is an `InvalidNextTokenException`,
//!    `MaxResults` outside its published `@range` is refused at request validation, and
//!    an `AccountId` that is not the caller's keeps `AccessDeniedException`'s modelled
//!    403.
//! 5. The statuses listed are the ones the CLOCK already owed, and repeating the list
//!    changes nothing a later caller sees.
//! 6. An action is a resource OF the budget that owns it, so deleting that budget takes
//!    its actions out of the account-wide list as well.
//!
//! Each table spans those classes rather than one request: its rows differ along the
//! number of budgets the account holds, the number of actions each budget holds, the
//! action type (and with it the `Definition` member the type names), the approval model,
//! the notification type, the threshold value and type, the subscriber count and types,
//! the presence of `ResourceTags`, and the page size.
//!
//! The harness (`router`, `call`, `is_error`, `account`, `same`), the action table and
//! the action builders are the sibling suites', so every suite in this target drives the
//! service the same way.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::time::Duration;

use service_com_amazonaws_budgets_awsbudgetservicegateway::state::{
    ACTION_SETTLES_AFTER, ACTION_STATUS_EXECUTION_IN_PROGRESS, ACTION_STATUS_EXECUTION_SUCCESS,
    EXECUTION_TYPE_APPROVE, MAX_ACTIONS_PER_BUDGET,
};

use super::budget_action_execution::{
    created_status, describe_action, execute_action, seed_budget, simple_create_body,
};
use super::budget_actions_for_budget::{action_rows, create_action, ids_of, list_actions};
use super::budget_lifecycle::{account, caller_account, call, is_error, router, same};

/// The most actions one page of this operation can hold, and so the page a request
/// naming no `MaxResults` is served: `DescribeBudgetActionsForAccountResponse$Actions` is
/// `@length(min: 0, max: 100)` and `MaxResults` is `@range(min: 1, max: 100)` — "An
/// integer that represents how many entries a paginated response contains. The maximum
/// is 100." (`smithy_query(query="operation",operation="DescribeBudgetActionsForAccount")`).
const ACTIONS_MAX_PAGE: usize = 100;

/// One `DescribeBudgetActionsForAccount` call.
async fn list_for_account(
    router: &MultiServiceRouter,
    account_id: &str,
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> (u16, Value) {
    let mut request = json!({"AccountId": account_id});
    if let Some(max) = max_results {
        request["MaxResults"] = json!(max);
    }
    if let Some(token) = next_token {
        request["NextToken"] = json!(token);
    }
    call(router, "DescribeBudgetActionsForAccount", &request).await
}

/// Walk every page of the account-wide list at `max_results` and return the actions in
/// the order they were served, asserting that no page exceeds the effective page size
/// and that a token is handed back only after a non-empty page.
async fn walk_account_pages(
    router: &MultiServiceRouter,
    account_id: &str,
    max_results: Option<i32>,
    total: usize,
) -> Vec<Value> {
    let case = format!("total={total} max={max_results:?}");
    let effective = match max_results {
        Some(max) => usize::try_from(max).unwrap_or(1).min(ACTIONS_MAX_PAGE),
        None => ACTIONS_MAX_PAGE,
    };
    let mut served = Vec::new();
    let mut next: Option<String> = None;
    // One more request than there can be pages, so a token that never clears is a
    // failed assertion rather than a hang.
    for _ in 0..=total.div_ceil(effective.max(1)) + 1 {
        let (status, body) =
            list_for_account(router, account_id, max_results, next.as_deref()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body}"
        );
        let page = body["Actions"].as_array().cloned().unwrap_or_default();
        assert!(
            page.len() <= effective,
            "case {case:?}: a page may hold at most {effective} actions, got {}",
            page.len()
        );
        served.extend(page.iter().cloned());
        next = body
            .get("NextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        if next.is_none() {
            return served;
        }
        assert_eq!(
            page.len(),
            effective,
            "case {case:?}: a page followed by a NextToken must be full at {effective}, \
             got {}",
            page.len()
        );
    }
    panic!("case {case:?}: pagination never finished");
}

/// Seed one account with several budgets holding differing numbers of actions, and
/// answer `(BudgetName, ActionId)` per budget in `BudgetName` order — the order the
/// account-wide list must serve them in.
///
/// The budgets are CREATED in descending name order and hold different numbers of
/// actions (none, one, two, every row of the action table), so a list that follows
/// creation order instead of `BudgetName`, or that reports only one budget's actions, is
/// a failed assertion.
async fn seed_account(
    router: &MultiServiceRouter,
    account_id: &str,
) -> Vec<(String, Vec<String>)> {
    let table = action_rows(account_id, "placeholder");
    let counts = [0_usize, 1, 2, table.len().min(MAX_ACTIONS_PER_BUDGET)];
    let mut seeded: Vec<(String, Vec<String>)> = Vec::new();
    for (index, count) in counts.iter().enumerate().rev() {
        let budget_name = format!("acct-{index:03}");
        seed_budget(router, account_id, &budget_name).await;
        let mut created = Vec::new();
        for row in table.iter().take(*count) {
            let mut create = row.create.clone();
            create["BudgetName"] = json!(&budget_name);
            created.push(create_action(router, &create, &row.case).await);
        }
        seeded.push((budget_name, created));
    }
    seeded.sort_by(|left, right| left.0.cmp(&right.0));
    seeded
}

/// Every `ActionId` the account holds, in the order the list must serve them.
fn flattened(seeded: &[(String, Vec<String>)]) -> Vec<String> {
    seeded
        .iter()
        .flat_map(|(_, created)| created.iter().cloned())
        .collect()
}

/// Seed `total` actions across as few budgets as the per-budget ceiling allows, and
/// answer their `ActionId`s in the order the account-wide list must serve them. Used by
/// the rows that need more actions than one budget can hold.
async fn seed_bulk(router: &MultiServiceRouter, account_id: &str, total: usize) -> Vec<String> {
    let mut ids = Vec::new();
    let mut remaining = total;
    // Zero-padded names created in ascending order, so byte order is creation order and
    // the expected vector is simply the order they were created in.
    for index in 0..total.div_ceil(MAX_ACTIONS_PER_BUDGET) {
        let budget_name = format!("bulk-{index:03}");
        seed_budget(router, account_id, &budget_name).await;
        let holding = remaining.min(MAX_ACTIONS_PER_BUDGET);
        for position in 0..holding {
            let mut create = simple_create_body(account_id, &budget_name);
            // A distinct threshold per action, so a mis-paged action is visible in the
            // body a failed assertion prints.
            create["ActionThreshold"]["ActionThresholdValue"] =
                json!((index * MAX_ACTIONS_PER_BUDGET + position) as f64);
            ids.push(create_action(router, &create, &format!("{budget_name}/{position}")).await);
        }
        remaining -= holding;
    }
    ids
}

/// RULE 1: the account-wide list reports every action of EVERY budget the account holds,
/// ordered by the owning `BudgetName` and by creation order within one budget, each
/// exactly as `DescribeBudgetAction` reports that same action — so the list is the
/// store's own view of the account's actions rather than a separate projection of it.
#[tokio::test]
async fn the_account_list_reports_every_action_of_every_budget_in_budget_name_order() {
    let router = router();
    let account_id = caller_account();
    let seeded = seed_account(&router, &account_id).await;
    let expected = flattened(&seeded);

    let served = walk_account_pages(&router, &account_id, None, expected.len()).await;
    assert_eq!(
        ids_of(&served),
        expected,
        "expected every action of every budget exactly once, in BudgetName then creation \
         order, got {:?}",
        ids_of(&served)
    );

    let mut position = 0;
    for (budget_name, created) in &seeded {
        for action_id in created {
            let case = format!("{budget_name}/{action_id} at position {position}");
            let (status, read) = describe_action(&router, &account_id, budget_name, action_id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the action to be readable, got {status} {read}"
            );
            assert!(
                same(&served[position], &read["Action"]),
                "case {case:?}: expected the listed action to be the one a read reports, \
                 {}, got {}",
                read["Action"],
                served[position]
            );
            assert!(
                same(
                    &served[position]["Status"],
                    &json!(created_status(&served[position]))
                ),
                "case {case:?}: expected a created action to be listed with the status its \
                 trigger threshold gives it, got {}",
                served[position]["Status"]
            );
            position += 1;
        }
    }
}

/// RULE 1, the empty half: an account that holds no actions is served an empty list and
/// HTTP 200 — not a refusal. The rows differ along how the account got to zero actions:
/// no budgets at all, budgets that never held an action, and a budget whose only action
/// was deleted.
#[tokio::test]
async fn an_account_holding_no_actions_is_served_as_an_empty_list() {
    for (case, prepare) in [
        ("an account with no budgets at all", 0_usize),
        ("budgets that never held an action", 2),
    ] {
        let router = router();
        let account_id = caller_account();
        for index in 0..prepare {
            seed_budget(&router, &account_id, &format!("empty-{index}")).await;
        }
        let (status, body) = list_for_account(&router, &account_id, None, None).await;
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

    // And the same once the account's only action has been deleted.
    let router = router();
    let account_id = caller_account();
    seed_budget(&router, &account_id, "had-one").await;
    let action_id = create_action(
        &router,
        &simple_create_body(&account_id, "had-one"),
        "the account's only action",
    )
    .await;
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
    let (status, body) = list_for_account(&router, &account_id, None, None).await;
    assert_eq!(
        status, 200,
        "case \"the account's only action was deleted\": expected 200, got {status} {body}"
    );
    assert_eq!(
        body["Actions"].as_array().map(Vec::len),
        Some(0),
        "case \"the account's only action was deleted\": expected an empty list, got {body}"
    );
}

/// RULE 2: the account-wide list is the per-budget lists concatenated in `BudgetName`
/// order, so the two reads cannot disagree about the same actions. Asserted over every
/// seeded budget, including the ones holding no actions.
#[tokio::test]
async fn the_account_list_is_the_per_budget_lists_in_budget_name_order() {
    let router = router();
    let account_id = caller_account();
    let seeded = seed_account(&router, &account_id).await;

    let served = walk_account_pages(&router, &account_id, None, flattened(&seeded).len()).await;
    let mut position = 0;
    for (budget_name, created) in &seeded {
        let (status, body) = list_actions(&router, &account_id, budget_name, None, None).await;
        assert_eq!(
            status, 200,
            "case {budget_name:?}: expected the per-budget list to answer 200, \
             got {status} {body}"
        );
        let own = body["Actions"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            ids_of(&own),
            *created,
            "case {budget_name:?}: expected the per-budget list to report its own actions, \
             got {:?}",
            ids_of(&own)
        );
        let slice = &served[position..position + own.len()];
        assert!(
            same(
                &Value::Array(slice.to_vec()),
                &Value::Array(own.clone())
            ),
            "case {budget_name:?}: expected the account list to carry this budget's own \
             list at offset {position}, {:?}, got {slice:?}",
            own
        );
        position += own.len();
    }
    assert_eq!(
        position,
        served.len(),
        "expected the account list to hold nothing beyond the budgets' own lists, got {} \
         actions after {position}",
        served.len()
    );
}

/// RULE 3: `MaxResults` splits the account's actions into pages of at most that many,
/// each continuing where the previous `NextToken` left off, so walking the pages yields
/// every action exactly once in the one order, whatever the page size.
///
/// The rows are the cross product of the number of actions the account holds (none, one,
/// a few, several budgets' worth) and the page size (absent, each published `@range`
/// bound of `MaxResults` — 1 and 100 — and sizes between).
#[tokio::test]
async fn max_results_partitions_the_accounts_actions_in_one_order() {
    for total in [0_usize, 1, 2, MAX_ACTIONS_PER_BUDGET + 3] {
        // One store per row, so a row sees only its own actions.
        let router = router();
        let account_id = caller_account();
        let expected = seed_bulk(&router, &account_id, total).await;

        for max_results in [None, Some(1), Some(2), Some(7), Some(100)] {
            let served = walk_account_pages(&router, &account_id, max_results, total).await;
            assert_eq!(
                ids_of(&served),
                expected,
                "case total={total} max={max_results:?}: expected every action exactly once \
                 in the one order, got {:?}",
                ids_of(&served)
            );
        }
    }
}

/// RULE 3, the default: a request naming no `MaxResults` is served at most what the
/// output list holds (`Actions` is `@length(max: 100)`) and a `NextToken` while actions
/// remain — asserted on each side of that boundary, with the rest reachable only by
/// following the token.
#[tokio::test]
async fn an_absent_max_results_is_capped_at_the_page_the_output_list_holds() {
    for total in [ACTIONS_MAX_PAGE - 1, ACTIONS_MAX_PAGE, ACTIONS_MAX_PAGE + 1] {
        // One store per row, so a row sees only its own actions.
        let router = router();
        let account_id = caller_account();
        let expected = seed_bulk(&router, &account_id, total).await;

        let (status, body) = list_for_account(&router, &account_id, None, None).await;
        assert_eq!(
            status, 200,
            "case total={total}: expected 200, got {status} {body}"
        );
        let first = body["Actions"].as_array().cloned().unwrap_or_default();
        let expected_first = total.min(ACTIONS_MAX_PAGE);
        assert_eq!(
            first.len(),
            expected_first,
            "case total={total}: expected the first page to hold {expected_first} actions, \
             got {}",
            first.len()
        );
        let token = body["NextToken"].as_str().map(str::to_string);
        assert_eq!(
            token.is_some(),
            total > ACTIONS_MAX_PAGE,
            "case total={total}: expected a NextToken only while actions remain, got {token:?}"
        );

        let served = walk_account_pages(&router, &account_id, None, total).await;
        assert_eq!(
            ids_of(&served),
            expected,
            "case total={total}: expected the token walk to reach every action exactly once, \
             got {} of them",
            served.len()
        );
    }
}

/// RULE 4: a `NextToken` this list never minted does not decode, so it is refused with
/// `InvalidNextTokenException` instead of silently serving the first page; `MaxResults`
/// outside its published `@range(min: 1, max: 100)` is refused at request validation
/// while both published bounds are accepted; and an `AccountId` that is not the caller's
/// keeps `AccessDeniedException`'s modelled 403.
#[tokio::test]
async fn bad_tokens_page_sizes_and_accounts_are_refused() {
    let router = router();
    let account_id = caller_account();
    seed_budget(&router, &account_id, "tokens").await;
    create_action(
        &router,
        &simple_create_body(&account_id, "tokens"),
        "the one action the token rows page over",
    )
    .await;

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
        let (status, body) = list_for_account(&router, &account_id, None, Some(token)).await;
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
    let (_, body) = list_for_account(&router, &account_id, Some(1), None).await;
    assert!(
        body.get("NextToken").is_none_or(Value::is_null),
        "expected no NextToken when the page exhausted the account, got {body}"
    );

    // `MaxResults` is @range(min: 1, max: 100): one past each published bound, then each
    // bound itself.
    for max_results in [0, 101] {
        let (status, body) = list_for_account(&router, &account_id, Some(max_results), None).await;
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
        let (status, body) = list_for_account(&router, &account_id, Some(max_results), None).await;
        assert_eq!(
            status, 200,
            "case MaxResults={max_results}: a published bound must be accepted, \
             got {status} {body}"
        );
    }

    // Accounts differing along value, none of them the caller's: the account the actions
    // were created under is reachable, every other is not.
    for other in [account(1), account(999_999_999_999), account(123_456_789_012)] {
        let (status, body) = list_for_account(&router, &other, None, None).await;
        assert_eq!(
            status, 403,
            "case AccountId={other:?}: expected the modelled 403, got {status} {body}"
        );
        assert!(
            is_error(&body, "AccessDeniedException"),
            "case AccountId={other:?}: expected AccessDeniedException, got {body}"
        );
    }
}

/// RULE 5: the statuses the account-wide list reports are the ones the CLOCK already
/// owed. Straight after an execution the action is listed in its `*_IN_PROGRESS` status;
/// once the settle span has passed the same list reports the settled status with no call
/// in between having made that move. Listing twice changes nothing a later caller sees.
///
/// The target action is taken from each budget of the account in turn, so the rule holds
/// for an in-flight action at every position of the account's order while its siblings
/// stay armed.
#[tokio::test]
async fn the_account_list_reports_the_status_the_clock_owes() {
    let seeded_shape = seed_account(&router(), &caller_account()).await;
    let targets: Vec<usize> = (0..flattened(&seeded_shape).len()).collect();

    for target in targets {
        // One store per row, so each row executes exactly one action of a fresh account.
        let router = router();
        let account_id = caller_account();
        let seeded = seed_account(&router, &account_id).await;
        let expected = flattened(&seeded);
        let owner = seeded
            .iter()
            .find(|(_, created)| created.contains(&expected[target]))
            .map(|(budget_name, _)| budget_name.clone())
            .unwrap_or_default();
        let case = format!("action {target} of {}, held by {owner:?}", expected.len());

        let (status, body) = execute_action(
            &router,
            &account_id,
            &owner,
            &expected[target],
            EXECUTION_TYPE_APPROVE,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the execution to be accepted, got {status} {body}"
        );

        // In flight: the executed action is listed as such, its siblings as armed.
        let in_flight = walk_account_pages(&router, &account_id, None, expected.len()).await;
        assert_eq!(
            ids_of(&in_flight),
            expected,
            "case {case:?}: expected the execution to leave the list's order alone"
        );
        for (position, action) in in_flight.iter().enumerate() {
            let want = if position == target {
                ACTION_STATUS_EXECUTION_IN_PROGRESS
            } else {
                created_status(action)
            };
            assert!(
                same(&action["Status"], &json!(want)),
                "case {case:?}: expected position {position} to be listed as {want}, got {}",
                action["Status"]
            );
        }

        // A second list reports the same thing: the read changed nothing.
        let again = walk_account_pages(&router, &account_id, None, expected.len()).await;
        assert!(
            same(
                &Value::Array(again.clone()),
                &Value::Array(in_flight.clone())
            ),
            "case {case:?}: expected a repeated list to report the same actions, got \
             {again:?} after {in_flight:?}"
        );

        // The clock settles the work with no further call, and the next list says so.
        tokio::time::sleep(ACTION_SETTLES_AFTER + Duration::from_millis(250)).await;
        let settled = walk_account_pages(&router, &account_id, None, expected.len()).await;
        for (position, action) in settled.iter().enumerate() {
            let want = if position == target {
                ACTION_STATUS_EXECUTION_SUCCESS
            } else {
                created_status(action)
            };
            assert!(
                same(&action["Status"], &json!(want)),
                "case {case:?}: expected position {position} to be listed as {want} after the \
                 settle, got {}",
                action["Status"]
            );
        }
    }
}

/// RULE 6: an action is a resource OF the budget that owns it, so deleting that budget
/// takes its actions out of the account-wide list too — and leaves every other budget's
/// actions exactly where they were. Asserted by deleting each seeded budget in turn.
#[tokio::test]
async fn deleting_a_budget_takes_its_actions_out_of_the_account_list() {
    let router = router();
    let account_id = caller_account();
    let mut seeded = seed_account(&router, &account_id).await;

    while !seeded.is_empty() {
        let (budget_name, _) = seeded.remove(0);
        let (status, body) = call(
            &router,
            "DeleteBudget",
            &json!({"AccountId": &account_id, "BudgetName": &budget_name}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {budget_name:?}: expected the budget to delete, got {status} {body}"
        );

        let expected = flattened(&seeded);
        let served = walk_account_pages(&router, &account_id, None, expected.len()).await;
        assert_eq!(
            ids_of(&served),
            expected,
            "case {budget_name:?}: expected the account list to hold only the remaining \
             budgets' actions, got {:?}",
            ids_of(&served)
        );
    }
}
