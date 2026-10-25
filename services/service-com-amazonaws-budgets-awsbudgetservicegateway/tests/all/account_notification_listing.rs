// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for `DescribeBudgetNotificationsForAccount`, driven over the wire
//! through the same `MultiServiceRouter` the production router serves from.
//!
//! Each test states a RULE and asserts it over a table that spans the class the rule
//! generalises over, not one request:
//!
//! 1. The read reports, for every budget of the named account that carries at least one
//!    notification, that budget's name and the notifications stored on it — the same
//!    notifications the per-budget read reports — ordered by `BudgetName`; a budget
//!    with no notification is not reported. (A request naming an account other than the
//!    one this service answers for is refused before it reads anything —
//!    `tests/all/account_scoping.rs`.)
//! 2. What it reports follows the stored state: a notification created after the budget
//!    appears, and one deleted disappears.
//! 3. It is paginated over budgets by `MaxResults` (default 50) with an opaque
//!    `NextToken`, so walking the pages yields every entry exactly once in order; a
//!    token this listing never minted is refused with `InvalidNextTokenException`.
//! 4. It is a pure read: repeating it, and the per-budget read after it, report the
//!    same thing.
//!
//! The harness (`router`, `call`, `is_error`, `caller_account`, `same`) is the lifecycle
//! suite's, so every suite in this target drives the service exactly the same way.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The token codec is the service's own (`src/state.rs`), so a test that needs a token
/// of the right FORM pointing at an offset this listing never had does not re-type it.
use service_com_amazonaws_budgets_awsbudgetservicegateway::state::encode_next_token;

use super::budget_lifecycle::{caller_account, call, is_error, router, same};

/// The notification members a caller supplies. `Threshold` is what makes two
/// notifications of one budget distinct, so each row varies it.
fn notification(notification_type: &str, operator: &str, threshold: u64) -> Value {
    json!({
        "NotificationType": notification_type,
        "ComparisonOperator": operator,
        "Threshold": threshold
    })
}

/// `count` distinct notifications, each with one email subscriber — the shape
/// `CreateBudget$NotificationsWithSubscribers` takes.
fn notifications_with_subscribers(count: u64) -> Value {
    Value::Array(
        (0..count)
            .map(|index| {
                json!({
                    "Notification": notification("ACTUAL", "GREATER_THAN", index + 1),
                    "Subscribers": [
                        {"SubscriptionType": "EMAIL", "Address": format!("ops{index}@example.com")}
                    ]
                })
            })
            .collect(),
    )
}

/// Create `name` under `account_id` with `count` notifications on it, and fail the test
/// if the setup call did not take.
async fn seed_budget(router: &MultiServiceRouter, account_id: &str, name: &str, count: u64) {
    let mut request = json!({
        "AccountId": account_id,
        "Budget": {
            "BudgetName": name,
            "TimeUnit": "MONTHLY",
            "BudgetType": "COST",
            "BudgetLimit": {"Amount": "100", "Unit": "USD"}
        }
    });
    if count > 0 {
        request["NotificationsWithSubscribers"] = notifications_with_subscribers(count);
    }
    let (status, body) = call(router, "CreateBudget", &request).await;
    assert_eq!(
        status, 200,
        "setup: expected CreateBudget({name}) with {count} notifications to answer 200, \
         got {status} {body}"
    );
}

/// One `DescribeBudgetNotificationsForAccount` call.
async fn describe_for_account(
    router: &MultiServiceRouter,
    account_id: &str,
    max_results: Option<i64>,
    next_token: Option<&str>,
) -> (u16, Value) {
    let mut request = json!({"AccountId": account_id});
    if let Some(max) = max_results {
        request["MaxResults"] = json!(max);
    }
    if let Some(token) = next_token {
        request["NextToken"] = json!(token);
    }
    call(router, "DescribeBudgetNotificationsForAccount", &request).await
}

/// The reported entries of one (unpaginated) account read, failing the test on anything
/// but a 200.
async fn entries(router: &MultiServiceRouter, account_id: &str) -> Vec<Value> {
    let (status, body) = describe_for_account(router, account_id, Some(1000), None).await;
    assert_eq!(
        status, 200,
        "expected DescribeBudgetNotificationsForAccount to answer 200, got {status} {body}"
    );
    body["BudgetNotificationsForAccount"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The budget names an account read reported, in the order it reported them.
fn reported_names(entries: &[Value]) -> Vec<String> {
    entries
        .iter()
        .map(|entry| entry["BudgetName"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// What the per-budget read reports for `budget_name` — the notifications the account
/// read must group under that name.
async fn per_budget_notifications(
    router: &MultiServiceRouter,
    account_id: &str,
    budget_name: &str,
) -> Value {
    let (status, body) = call(
        router,
        "DescribeNotificationsForBudget",
        &json!({"AccountId": account_id, "BudgetName": budget_name, "MaxResults": 100}),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected DescribeNotificationsForBudget({budget_name}) to answer 200, \
         got {status} {body}"
    );
    body["Notifications"].clone()
}

/// One row of the grouping table: the budgets to create under one account, as
/// `(BudgetName, how many notifications)`, in creation order.
struct Row {
    case: &'static str,
    budgets: Vec<(&'static str, u64)>,
}

/// The grouping table. Rows differ along what rule 1 generalises over: how many budgets
/// the account holds, how many notifications each carries, whether some carry none at
/// all, and whether creation order already matches `BudgetName` order.
fn grouping_rows() -> Vec<Row> {
    vec![
        Row {
            case: "one budget with one notification",
            budgets: vec![("solo", 1)],
        },
        Row {
            case: "one budget with no notification at all",
            budgets: vec![("quiet", 0)],
        },
        Row {
            case: "several budgets, created out of name order, with differing counts",
            budgets: vec![("zeta", 1), ("alpha", 3), ("mid", 2)],
        },
        Row {
            case: "notified and un-notified budgets mixed",
            budgets: vec![("aaa", 0), ("bbb", 2), ("ccc", 0), ("ddd", 1)],
        },
        Row {
            case: "every budget carrying the per-create maximum of notifications",
            budgets: vec![("five-a", 5), ("five-b", 5)],
        },
    ]
}

/// RULE: the read reports one entry per budget of the named account that holds at least
/// one notification — that budget's name and exactly the notifications the per-budget
/// read reports for it — ordered by `BudgetName`, and nothing for a budget that holds
/// none.
#[tokio::test]
async fn an_account_read_groups_every_stored_notification_under_its_budget() {
    for row in grouping_rows() {
        let Row { case, budgets } = row;
        // One store per row, so a row sees only its own budgets.
        let router = router();
        let account_id = caller_account();
        for (name, count) in &budgets {
            seed_budget(&router, &account_id, name, *count).await;
        }

        let found = entries(&router, &account_id).await;

        // Expected: the notified budgets' names, in byte order — the same order the
        // handler sorts by.
        let mut expected_names: Vec<String> = budgets
            .iter()
            .filter(|(_, count)| *count > 0)
            .map(|(name, _)| (*name).to_string())
            .collect();
        expected_names.sort();
        assert_eq!(
            reported_names(&found),
            expected_names,
            "case {case:?}: expected the notified budgets in name order, got {found:?}"
        );

        for (name, count) in &budgets {
            let entry = found
                .iter()
                .find(|entry| entry["BudgetName"] == json!(name))
                .cloned();
            if *count == 0 {
                assert!(
                    entry.is_none(),
                    "case {case:?}: expected no entry for the un-notified budget {name:?}, \
                     got {entry:?}"
                );
                continue;
            }
            let entry = entry.unwrap_or(Value::Null);
            let expected = per_budget_notifications(&router, &account_id, name).await;
            assert_eq!(
                entry["Notifications"].as_array().map(Vec::len),
                Some(*count as usize),
                "case {case:?}: expected {count} notifications under {name:?}, got {entry:?}"
            );
            assert!(
                same(&entry["Notifications"], &expected),
                "case {case:?}: expected {name:?} to carry the per-budget read's \
                 {expected:?}, got {:?}",
                entry["Notifications"]
            );
        }
    }
}

/// RULE: the read reports every budget of the account that holds a notification, so a
/// budget whose notifications are all gone is not reported while its neighbours still
/// are.
#[tokio::test]
async fn an_account_read_reports_every_notified_budget_of_the_account() {
    let router = router();
    let account_id = caller_account();
    // Rows differ in how many notifications a budget carries, including none at all.
    let rows = [("one", 1_u64), ("four", 4), ("none", 0)];
    for (name, count) in &rows {
        seed_budget(&router, &account_id, name, *count).await;
    }

    let found = entries(&router, &account_id).await;
    let mut expected: Vec<String> = rows
        .iter()
        .filter(|(_, count)| *count > 0)
        .map(|(name, _)| (*name).to_string())
        .collect();
    expected.sort();
    assert_eq!(
        reported_names(&found),
        expected,
        "case the notified budgets of the account: expected {expected:?}, got {found:?}"
    );
    for (name, count) in rows.iter().filter(|(_, count)| *count > 0) {
        let entry = found
            .iter()
            .find(|entry| entry["BudgetName"] == json!(name))
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            entry["Notifications"].as_array().map(Vec::len),
            Some(*count as usize),
            "case budget {name:?}: expected its own {count} notifications, got {entry:?}"
        );
    }
}

/// RULE: the read follows the stored state — a notification created on a budget after
/// the budget exists brings that budget into the listing, and deleting the last one
/// takes it back out.
#[tokio::test]
async fn an_account_read_follows_what_is_stored() {
    let router = router();
    let account_id = caller_account();
    let name = "late";
    seed_budget(&router, &account_id, name, 0).await;
    assert!(
        entries(&router, &account_id).await.is_empty(),
        "case before any notification: expected an empty listing"
    );

    // Rows: the notifications added one at a time, differing in every identity member.
    let added = [
        notification("ACTUAL", "GREATER_THAN", 10),
        notification("FORECASTED", "LESS_THAN", 20),
        notification("ACTUAL", "EQUAL_TO", 30),
    ];
    for (index, alert) in added.iter().enumerate() {
        let (status, body) = call(
            &router,
            "CreateNotification",
            &json!({
                "AccountId": &account_id,
                "BudgetName": name,
                "Notification": alert,
                "Subscribers": [{"SubscriptionType": "EMAIL", "Address": "ops@example.com"}]
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case add {index}: expected CreateNotification to answer 200, got {status} {body}"
        );
        let found = entries(&router, &account_id).await;
        assert_eq!(
            reported_names(&found),
            vec![name.to_string()],
            "case add {index}: expected {name:?} to be listed, got {found:?}"
        );
        assert_eq!(
            found[0]["Notifications"].as_array().map(Vec::len),
            Some(index + 1),
            "case add {index}: expected {} notifications, got {found:?}",
            index + 1
        );
    }

    for (index, alert) in added.iter().enumerate() {
        let (status, body) = call(
            &router,
            "DeleteNotification",
            &json!({"AccountId": &account_id, "BudgetName": name, "Notification": alert}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case delete {index}: expected DeleteNotification to answer 200, got {status} {body}"
        );
        let found = entries(&router, &account_id).await;
        let left = added.len() - index - 1;
        if left == 0 {
            assert!(
                found.is_empty(),
                "case delete {index}: expected the budget to drop out of the listing, \
                 got {found:?}"
            );
        } else {
            assert_eq!(
                found[0]["Notifications"].as_array().map(Vec::len),
                Some(left),
                "case delete {index}: expected {left} notifications left, got {found:?}"
            );
        }
    }
}

/// The documented default page size: "An integer that represents how many budgets a
/// paginated response contains. The default is 50."
/// (`DescribeBudgetNotificationsForAccountRequest$MaxResults`).
const DEFAULT_PAGE: usize = 50;

/// RULE: the listing is paginated over budgets — a request's `MaxResults` (the
/// documented default of 50 when it names none) bounds a page, and following the
/// `NextToken` walks every entry exactly once, in `BudgetName` order.
#[tokio::test]
async fn the_account_listing_is_paginated_over_budgets() {
    let router = router();
    let account_id = caller_account();
    // One more budget than the documented default page, so the default is observable,
    // and each with its own notification count so a mis-paged entry is visible.
    let total = DEFAULT_PAGE + 1;
    for index in 0..total {
        let count = (index % 3) as u64 + 1;
        seed_budget(&router, &account_id, &format!("paged-{index:03}"), count).await;
    }
    let all: Vec<String> = (0..total).map(|index| format!("paged-{index:03}")).collect();

    // Page sizes spanning the class: one at a time, a size that does not divide the
    // total, the exact total, the published `@range` maximum, and none at all (the
    // documented default).
    for page_size in [Some(1_i64), Some(7), Some(total as i64), Some(1000), None] {
        let case = format!("MaxResults {page_size:?}");
        let expected_page = page_size.unwrap_or(DEFAULT_PAGE as i64) as usize;
        let mut collected: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let (status, body) =
                describe_for_account(&router, &account_id, page_size, token.as_deref()).await;
            assert_eq!(
                status, 200,
                "case {case:?} page {pages}: expected 200, got {status} {body}"
            );
            let page = body["BudgetNotificationsForAccount"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert!(
                page.len() <= expected_page,
                "case {case:?} page {pages}: expected at most {expected_page} entries, \
                 got {}",
                page.len()
            );
            collected.extend(reported_names(&page));
            pages += 1;
            token = body["NextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
            assert_eq!(
                page.len(),
                expected_page,
                "case {case:?} page {pages}: a page followed by a token must be full at \
                 {expected_page}, got {}",
                page.len()
            );
            assert!(
                pages <= total + 1,
                "case {case:?}: the token walk did not terminate after {pages} pages"
            );
        }
        assert_eq!(
            collected, all,
            "case {case:?}: expected every budget once in name order over {pages} pages"
        );
        let expected_pages = total.div_ceil(expected_page);
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} pages for {total} entries, got {pages}"
        );
    }
}

/// RULE: a `NextToken` this listing never minted is refused with
/// `InvalidNextTokenException` (HTTP 400), and `MaxResults` outside the modelled
/// `@range(min: 1, max: 1000)` is refused at the boundary.
#[tokio::test]
async fn the_account_listing_refuses_a_token_or_page_size_it_never_minted() {
    let router = router();
    let account_id = caller_account();
    for index in 0..3 {
        seed_budget(&router, &account_id, &format!("token-{index}"), 1).await;
    }

    // A token the listing minted, to show the good case is the same call shape.
    let (status, body) = describe_for_account(&router, &account_id, Some(1), None).await;
    assert_eq!(status, 200, "setup: expected 200, got {status} {body}");
    let minted = body["NextToken"]
        .as_str()
        .expect("a page of 1 of 3 entries carries a token")
        .to_string();
    let (status, body) =
        describe_for_account(&router, &account_id, Some(1), Some(&minted)).await;
    assert_eq!(
        status, 200,
        "case the minted token: expected 200, got {status} {body}"
    );

    // Rows: not hex at all, hex that decodes to foreign text, a token of this listing's
    // own form but pointing past the end, and the empty string.
    for (case, token) in [
        ("not a token at all", "nonsense".to_string()),
        ("hex of foreign text", hex::encode("other:offset:1")),
        ("past the end of the listing", encode_next_token(99)),
        ("the empty string", String::new()),
    ] {
        let (status, body) =
            describe_for_account(&router, &account_id, Some(1), Some(&token)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the refusal at 400, got {status} {body}"
        );
        assert!(
            is_error(&body, "InvalidNextTokenException"),
            "case {case:?}: expected InvalidNextTokenException, got {body}"
        );
    }

    // The published `@range` bounds, and one past each.
    for (case, max_results, expected) in [
        ("the minimum", 1_i64, 200_u16),
        ("the maximum", 1000, 200),
        ("one below the minimum", 0, 400),
        ("one past the maximum", 1001, 400),
    ] {
        let (status, body) =
            describe_for_account(&router, &account_id, Some(max_results), None).await;
        assert_eq!(
            status, expected,
            "case MaxResults {case:?} ({max_results}): expected {expected}, got {status} {body}"
        );
    }
}

/// RULE: the account read reports and never mutates — repeating it answers the same
/// thing, and the per-budget read is unchanged by it.
#[tokio::test]
async fn the_account_listing_is_a_pure_read() {
    let router = router();
    let account_id = caller_account();
    // Rows: a budget with one notification, one with several, and one with none.
    for (name, count) in [("a", 1_u64), ("b", 3), ("c", 0)] {
        seed_budget(&router, &account_id, name, count).await;
    }

    let before_budget = per_budget_notifications(&router, &account_id, "b").await;
    let first = entries(&router, &account_id).await;
    let second = entries(&router, &account_id).await;
    assert!(
        same(&json!(first), &json!(second)),
        "case repeated read: expected {first:?}, got {second:?}"
    );
    let after_budget = per_budget_notifications(&router, &account_id, "b").await;
    assert!(
        same(&before_budget, &after_budget),
        "case per-budget read after the account read: expected {before_budget:?}, \
         got {after_budget:?}"
    );
}
