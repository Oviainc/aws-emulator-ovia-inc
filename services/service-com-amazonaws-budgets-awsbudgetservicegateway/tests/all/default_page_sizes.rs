// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the page size a paginated read serves when the request names
//! no `MaxResults`, driven over the wire through the same `MultiServiceRouter` the
//! production router serves from.
//!
//! RULE: a paginated read whose `MaxResults` member documents a default page size
//! serves at most that many entries to a request that names none, and a `NextToken`
//! while entries remain; a read whose member documents no default serves a full page at
//! that member's published `@range(max)` — every remaining entry, for the lists this
//! suite pages, which are shorter than that maximum. Each test asserts that over a table
//! of operations and page sizes, not one request.
//!
//! The harness (`router`, `call`, `caller_account`) is the lifecycle suite's, so every
//! suite in this target drives the service exactly the same way.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::budget_lifecycle::{caller_account, call, router};

/// The page size `DescribeBudgets` serves a request that names none:
/// `DescribeBudgetsRequest$MaxResults` is documented as "An integer that represents how
/// many budgets a paginated response contains. The default is 100."
const DESCRIBE_BUDGETS_DEFAULT_PAGE: usize = 100;

/// The page size `DescribeBudgetNotificationsForAccount` serves a request that names
/// none: `DescribeBudgetNotificationsForAccountRequest$MaxResults` is documented as "An
/// integer that represents how many budgets a paginated response contains. The default
/// is 50."
const ACCOUNT_NOTIFICATIONS_DEFAULT_PAGE: usize = 50;

/// The identity of one served notification: its `Threshold`, the member that makes two
/// notifications of one budget distinct.
fn threshold_key(threshold: f64) -> String {
    format!("{threshold:.1}")
}

/// One notification, distinguished from its siblings by `Threshold`.
fn notification(threshold: u64) -> Value {
    json!({
        "NotificationType": "ACTUAL",
        "ComparisonOperator": "GREATER_THAN",
        "Threshold": threshold
    })
}

/// `count` distinct EMAIL subscribers.
fn subscribers(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| json!({"SubscriptionType": "EMAIL", "Address": address(index)}))
            .collect(),
    )
}

/// The address of the `index`th seeded subscriber — the key a subscriber read is
/// compared on.
fn address(index: usize) -> String {
    format!("ops{index:02}@example.com")
}

/// Create one budget, optionally carrying notifications, and fail the test if the setup
/// call did not take. `notifications` is `(threshold, how many subscribers)` per
/// notification.
async fn seed_budget(
    router: &MultiServiceRouter,
    account_id: &str,
    name: &str,
    notifications: &[(u64, usize)],
) {
    let mut request = json!({
        "AccountId": account_id,
        "Budget": {
            "BudgetName": name,
            "TimeUnit": "MONTHLY",
            "BudgetType": "COST",
            "BudgetLimit": {"Amount": "100", "Unit": "USD"}
        }
    });
    if !notifications.is_empty() {
        request["NotificationsWithSubscribers"] = Value::Array(
            notifications
                .iter()
                .map(|(threshold, how_many)| {
                    json!({
                        "Notification": notification(*threshold),
                        "Subscribers": subscribers(*how_many)
                    })
                })
                .collect(),
        );
    }
    let (status, body) = call(router, "CreateBudget", &request).await;
    assert_eq!(
        status, 200,
        "setup: expected CreateBudget({name}) to answer 200, got {status} {body}"
    );
}

/// One row of the table: a paginated read, what it pages over, and the page size its
/// `MaxResults` member documents as the default (`None` where the model documents none).
struct Row {
    case: &'static str,
    operation: &'static str,
    /// The output member carrying the page.
    list_member: &'static str,
    /// The default this operation's `MaxResults` documents, where it documents one.
    documented_default: Option<usize>,
    /// The published `@range` maximum of this operation's `MaxResults`, from the model.
    published_max: i64,
    /// The request, without `MaxResults` or `NextToken`.
    request: Value,
    /// The identity of one served entry, so a mis-paged entry is visible.
    key: fn(&Value) -> String,
    /// Every entry the read must serve, in the order it must serve them.
    expected: Vec<String>,
}

/// Seed one account and describe every paginated read of it that this rule covers.
///
/// The budgets are created in descending name order, so an ordering that follows
/// creation rather than `BudgetName` is a failed assertion. The notified half of the
/// account is one budget more than the account-notifications default page, and the whole
/// account one budget more than the `DescribeBudgets` default page, so both defaults are
/// observable.
async fn seed_rows(router: &MultiServiceRouter, account_id: &str) -> Vec<Row> {
    let total_budgets = DESCRIBE_BUDGETS_DEFAULT_PAGE + 1;
    let notified = ACCOUNT_NOTIFICATIONS_DEFAULT_PAGE + 1;
    // `NotificationWithSubscribersList` is @length(max: 10) and a notification takes at
    // most 10 EMAIL subscribers, so the richest budget seeded carries 10 notifications
    // and its first notification 10 subscribers.
    let rich_notifications: Vec<(u64, usize)> = (0..10)
        .map(|index| (index + 1, if index == 0 { 10 } else { 1 }))
        .collect();

    for index in (0..total_budgets).rev() {
        let name = budget_name(index);
        if index == 0 {
            seed_budget(router, account_id, &name, &rich_notifications).await;
        } else if index < notified {
            seed_budget(router, account_id, &name, &[(1, 1)]).await;
        } else {
            seed_budget(router, account_id, &name, &[]).await;
        }
    }

    let all_budgets: Vec<String> = (0..total_budgets).map(budget_name).collect();
    let notified_budgets: Vec<String> = (0..notified).map(budget_name).collect();

    vec![
        Row {
            case: "DescribeBudgets, documented default 100",
            operation: "DescribeBudgets",
            list_member: "Budgets",
            documented_default: Some(DESCRIBE_BUDGETS_DEFAULT_PAGE),
            // DescribeBudgetsRequest$MaxResults: @range(min: 1, max: 1000).
            published_max: 1000,
            request: json!({"AccountId": account_id}),
            key: |entry| entry["BudgetName"].as_str().unwrap_or_default().to_string(),
            expected: all_budgets,
        },
        Row {
            case: "DescribeBudgetNotificationsForAccount, documented default 50",
            operation: "DescribeBudgetNotificationsForAccount",
            list_member: "BudgetNotificationsForAccount",
            documented_default: Some(ACCOUNT_NOTIFICATIONS_DEFAULT_PAGE),
            // DescribeBudgetNotificationsForAccountRequest$MaxResults:
            // @range(min: 1, max: 1000).
            published_max: 1000,
            request: json!({"AccountId": account_id}),
            key: |entry| entry["BudgetName"].as_str().unwrap_or_default().to_string(),
            expected: notified_budgets,
        },
        Row {
            case: "DescribeNotificationsForBudget, no documented default",
            operation: "DescribeNotificationsForBudget",
            list_member: "Notifications",
            documented_default: None,
            // DescribeNotificationsForBudgetRequest$MaxResults: @range(min: 1, max: 100).
            published_max: 100,
            request: json!({"AccountId": account_id, "BudgetName": budget_name(0)}),
            // `Notification$Threshold` is modelled a double, so the key is formatted
            // rather than compared as JSON text.
            key: |entry| threshold_key(entry["Threshold"].as_f64().unwrap_or_default()),
            expected: rich_notifications
                .iter()
                .map(|(threshold, _)| threshold_key(*threshold as f64))
                .collect(),
        },
        Row {
            case: "DescribeSubscribersForNotification, no documented default",
            operation: "DescribeSubscribersForNotification",
            list_member: "Subscribers",
            documented_default: None,
            // DescribeSubscribersForNotificationRequest$MaxResults:
            // @range(min: 1, max: 100).
            published_max: 100,
            request: json!({
                "AccountId": account_id,
                "BudgetName": budget_name(0),
                "Notification": notification(1)
            }),
            key: |entry| entry["Address"].as_str().unwrap_or_default().to_string(),
            expected: (0..10).map(address).collect(),
        },
    ]
}

/// The name of the `index`th seeded budget. Zero-padded, so byte order is the order the
/// reads must serve them in.
fn budget_name(index: usize) -> String {
    format!("paged-{index:03}")
}

/// Walk every page of one read at `max_results`, returning the keys it served in order
/// and the size of each page.
async fn walk(
    router: &MultiServiceRouter,
    row: &Row,
    max_results: Option<i64>,
) -> (Vec<String>, Vec<usize>) {
    let mut served: Vec<String> = Vec::new();
    let mut sizes: Vec<usize> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = row.request.clone();
        if let Some(max) = max_results {
            request["MaxResults"] = json!(max);
        }
        if let Some(held) = &token {
            request["NextToken"] = json!(held);
        }
        let (status, body) = call(router, row.operation, &request).await;
        assert_eq!(
            status, 200,
            "case {:?} max={max_results:?} page {}: expected 200, got {status} {body}",
            row.case,
            sizes.len()
        );
        let page = body[row.list_member].as_array().cloned().unwrap_or_default();
        sizes.push(page.len());
        served.extend(page.iter().map(row.key));
        token = body["NextToken"].as_str().map(str::to_string);
        if token.is_none() {
            return (served, sizes);
        }
        assert!(
            sizes.len() <= row.expected.len() + 1,
            "case {:?} max={max_results:?}: the token walk did not terminate after {} pages",
            row.case,
            sizes.len()
        );
    }
}

/// RULE: a request that names no `MaxResults` is served the page size its operation's
/// model documents as the default — not every remaining entry — so a `NextToken` is
/// minted while entries remain; a request that names one is served that, and an
/// operation documenting no default serves everything remaining. Walking the pages
/// yields every entry exactly once, in order, at every page size.
#[tokio::test]
async fn an_absent_max_results_is_served_the_page_size_the_model_documents() {
    let router = router();
    let account_id = caller_account();
    let rows = seed_rows(&router, &account_id).await;

    for row in &rows {
        // Page sizes spanning the class: none at all (the rule under test), one at a
        // time, a size that divides no total here, and the published `@range` maximum.
        for max_results in [None, Some(1_i64), Some(7), Some(row.published_max)] {
            let effective = match max_results {
                Some(max) => usize::try_from(max).expect("a positive page size"),
                None => row.documented_default.unwrap_or(row.expected.len().max(1)),
            };
            let (served, sizes) = walk(&router, row, max_results).await;
            assert_eq!(
                served, row.expected,
                "case {:?} max={max_results:?}: expected every entry exactly once in \
                 order, got {served:?}",
                row.case
            );
            for (index, size) in sizes.iter().enumerate() {
                assert!(
                    *size <= effective,
                    "case {:?} max={max_results:?}: page {index} may hold at most \
                     {effective} entries, got {size}",
                    row.case
                );
                let last = index + 1 == sizes.len();
                assert!(
                    last || *size == effective,
                    "case {:?} max={max_results:?}: a page followed by a NextToken must \
                     be full at {effective}, page {index} held {size}",
                    row.case
                );
            }
            let expected_pages = row.expected.len().div_ceil(effective).max(1);
            assert_eq!(
                sizes.len(),
                expected_pages,
                "case {:?} max={max_results:?}: expected {expected_pages} pages for {} \
                 entries, got {sizes:?}",
                row.case,
                row.expected.len()
            );
        }
    }
}

/// RULE: `DescribeBudgets` with no `MaxResults` serves at most the documented default of
/// 100 budgets and a `NextToken` while more remain — asserted on each side of that
/// boundary, and the remaining pages reachable only through the token.
#[tokio::test]
async fn describe_budgets_caps_an_unbounded_call_at_the_documented_default() {
    // Totals spanning the boundary: empty, well below, one below, exactly the default,
    // one past it, and half again past it.
    for total in [0_usize, 1, 99, 100, 101, 150] {
        // One store per row, so a row sees only its own budgets.
        let router = router();
        let account_id = caller_account();
        // Created in descending name order, so the order served is the budget names'
        // and not the order they were created in.
        for index in (0..total).rev() {
            seed_budget(&router, &account_id, &budget_name(index), &[]).await;
        }
        let expected: Vec<String> = (0..total).map(budget_name).collect();

        let (status, body) = call(
            &router,
            "DescribeBudgets",
            &json!({"AccountId": &account_id}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case total={total}: expected DescribeBudgets to answer 200, got {status} {body}"
        );
        let first = body["Budgets"].as_array().cloned().unwrap_or_default();
        let expected_first = total.min(DESCRIBE_BUDGETS_DEFAULT_PAGE);
        assert_eq!(
            first.len(),
            expected_first,
            "case total={total}: expected the first page to hold {expected_first} \
             budgets, got {}",
            first.len()
        );
        let token = body["NextToken"].as_str().map(str::to_string);
        assert_eq!(
            token.is_some(),
            total > DESCRIBE_BUDGETS_DEFAULT_PAGE,
            "case total={total}: expected a NextToken only while budgets remain, \
             got {token:?}"
        );

        // The rest of the account is reachable by following the token, and only then.
        let mut served: Vec<String> = first
            .iter()
            .map(|budget| budget["BudgetName"].as_str().unwrap_or_default().to_string())
            .collect();
        let mut next = token;
        let mut pages = 1;
        while let Some(held) = next {
            let (status, body) = call(
                &router,
                "DescribeBudgets",
                &json!({"AccountId": &account_id, "NextToken": held}),
            )
            .await;
            assert_eq!(
                status, 200,
                "case total={total} page {pages}: expected 200, got {status} {body}"
            );
            let page = body["Budgets"].as_array().cloned().unwrap_or_default();
            assert!(
                page.len() <= DESCRIBE_BUDGETS_DEFAULT_PAGE,
                "case total={total} page {pages}: expected at most {DESCRIBE_BUDGETS_DEFAULT_PAGE} \
                 budgets, got {}",
                page.len()
            );
            served.extend(
                page.iter()
                    .map(|budget| budget["BudgetName"].as_str().unwrap_or_default().to_string()),
            );
            next = body["NextToken"].as_str().map(str::to_string);
            pages += 1;
            assert!(
                pages <= total + 1,
                "case total={total}: the token walk did not terminate after {pages} pages"
            );
        }
        assert_eq!(
            served, expected,
            "case total={total}: expected every budget exactly once in BudgetName order \
             over {pages} pages, got {served:?}"
        );
        let expected_pages = total.div_ceil(DESCRIBE_BUDGETS_DEFAULT_PAGE).max(1);
        assert_eq!(
            pages, expected_pages,
            "case total={total}: expected {expected_pages} pages at the documented \
             default, got {pages}"
        );
    }
}
