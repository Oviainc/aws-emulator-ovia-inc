// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the ONE page-size resolution every paginated read of this
//! service shares, driven over the wire through the same `MultiServiceRouter` the
//! production router serves from.
//!
//! RULE: a paginated read serves at most the page size the caller named, never more than
//! the `@range(max)` its own `MaxResults` member publishes; a request that names no
//! `MaxResults` is served the page size that member documents as the default, or — where
//! it documents none — a full page at that published maximum. So every one of these
//! reads resolves the same effective page size from the same inputs, and walking the
//! pages at any size yields every entry exactly once in the one order.
//!
//! The table's rows are one per operation that resolves a page size: the four action,
//! action-history and performance-history listings whose `MaxResults` documents no
//! default, and the two account-wide listings whose `MaxResults` documents one (100
//! budgets, 50 notified budgets). They differ along what is paged (actions of one budget,
//! actions of a whole account, an action's history records, a budget's performance
//! periods, budgets, notified budgets), how many entries there are, and the page size
//! asked for.
//!
//! The harness (`router`, `call`, `caller_account`, `civil_secs`), the budget seeding and
//! the action builders are the sibling suites', so every suite in this target drives the
//! service exactly the same way.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::budget_action_execution::{seed_budget, simple_create_body};
use super::budget_actions_for_budget::create_action;
use super::budget_lifecycle::{call, caller_account, civil_secs, router};

/// The published `@range(max)` of the `MaxResults` member the per-budget and
/// per-account action, action-history and performance-history listings share:
/// `@range(min: 1, max: 100)` — "An integer that represents how many entries a paginated
/// response contains. The maximum is 100."
/// (`smithy_query(query="operation",operation="DescribeBudgetActionsForAccount")`).
const SHARED_MAX_RESULTS: usize = 100;

/// The published `@range(max)` of `MaxResultsDescribeBudgets` and
/// `MaxResultsBudgetNotifications`, the page-size members of the two account-wide
/// listings: `@range(min: 1, max: 1000)`
/// (`smithy_query(query="operation",operation="DescribeBudgets,DescribeBudgetNotificationsForAccount")`).
const ACCOUNT_LISTING_MAX_RESULTS: usize = 1000;

/// The page size `DescribeBudgets` documents for a request that names none:
/// "An integer that represents how many budgets a paginated response contains. The
/// default is 100." (`DescribeBudgetsRequest$MaxResults`).
const BUDGETS_DEFAULT_PAGE: usize = 100;

/// The page size `DescribeBudgetNotificationsForAccount` documents for a request that
/// names none: "An integer that represents how many budgets a paginated response
/// contains. The default is 50."
/// (`DescribeBudgetNotificationsForAccountRequest$MaxResults`).
const ACCOUNT_NOTIFICATIONS_DEFAULT_PAGE: usize = 50;

/// How many performance records a `DAILY` budget retains, written from the documentation
/// of `DescribeBudgetPerformanceHistoryResponse$BudgetPerformanceHistory`: "For DAILY
/// budgets, the history saves the state of the budget for the last 60 days." Spelled out
/// here rather than read off the service, so the row cannot drift into agreeing with a
/// wrong implementation.
const DAILY_RETAINED_PERIODS: usize = 60;

/// A start date far enough back that every retained `DAILY` period falls inside the
/// budget's own `TimePeriod`.
const LONG_RUNNING_START: &str = "2000-01-01T00:00:00Z";

/// The budget whose actions the per-budget row pages over, and whose first action owns
/// the history the action-history row pages over.
const ACTION_BUDGET: &str = "page-a";

/// A second budget holding actions, so the account-wide row pages over the actions of
/// more than one budget.
const SECOND_ACTION_BUDGET: &str = "page-b";

/// The budget whose performance history the performance row pages over.
const HISTORY_BUDGET: &str = "page-p";

/// One row of the table: a paginated read, what identifies one entry of it, and the
/// page-size facts its own `MaxResults` member publishes.
struct Row {
    case: &'static str,
    operation: &'static str,
    /// The request, without `MaxResults` or `NextToken`.
    request: Value,
    /// The path to the output member carrying the page.
    page_path: &'static [&'static str],
    /// The identity of one served entry, so a mis-paged entry is visible.
    key: fn(&Value) -> String,
    /// The default page size this operation's `MaxResults` documents, where it documents
    /// one.
    documented_default: Option<usize>,
    /// The published `@range` maximum of this operation's `MaxResults`.
    modelled_max: usize,
    /// Every entry the read must serve, in the order it must serve them.
    expected: Vec<String>,
}

impl Row {
    /// The effective page size this row is served for `max_results`: what the caller
    /// named, capped at the published maximum; or, when the caller named nothing, the
    /// documented default, or a full published-maximum page where the member documents
    /// no default.
    fn effective(&self, max_results: Option<usize>) -> usize {
        match max_results {
            Some(max) => max.min(self.modelled_max),
            None => self.documented_default.unwrap_or(self.modelled_max),
        }
    }
}

/// The list at `path` in `body`, or an empty list when the response carried none.
fn list_at(body: &Value, path: &[&str]) -> Vec<Value> {
    let mut at = body;
    for member in path {
        at = &at[*member];
    }
    at.as_array().cloned().unwrap_or_default()
}

/// A served action's identity.
fn action_id(action: &Value) -> String {
    action["ActionId"].as_str().unwrap_or_default().to_string()
}

/// A served budget's identity, and the identity of one account-wide notification entry
/// (one per notified budget).
fn budget_name(entry: &Value) -> String {
    entry["BudgetName"].as_str().unwrap_or_default().to_string()
}

/// A served history record's identity: the threshold of the action the record carries,
/// which every seeded write moved to a value of its own.
fn recorded_threshold(record: &Value) -> String {
    let value = record["ActionHistoryDetails"]["Action"]["ActionThreshold"]
        ["ActionThresholdValue"]
        .as_f64()
        .unwrap_or_default();
    format!("{value:.1}")
}

/// A served performance record's identity: the instant its own period begins, which the
/// awsJson wire renders as epoch seconds.
fn period_start(record: &Value) -> String {
    let secs = record["TimePeriod"]["Start"].as_f64().unwrap_or_default();
    format!("{secs:.0}")
}

/// Create the `DAILY` budget whose performance history has a record per retained day.
async fn seed_daily_budget(router: &MultiServiceRouter, account_id: &str, name: &str) {
    let (status, body) = call(
        router,
        "CreateBudget",
        &json!({
            "AccountId": account_id,
            "Budget": {
                "BudgetName": name,
                "BudgetType": "COST",
                "TimeUnit": "DAILY",
                "BudgetLimit": {"Amount": "5", "Unit": "USD"},
                "TimePeriod": {"Start": civil_secs(LONG_RUNNING_START)},
            }
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: expected CreateBudget({name}) to answer 200, got {status} {body}"
    );
}

/// Attach one notification to `name`, so the account-wide notification listing holds an
/// entry for it.
async fn seed_notification(router: &MultiServiceRouter, account_id: &str, name: &str) {
    let (status, body) = call(
        router,
        "CreateNotification",
        &json!({
            "AccountId": account_id,
            "BudgetName": name,
            "Notification": {
                "NotificationType": "ACTUAL",
                "ComparisonOperator": "GREATER_THAN",
                "Threshold": 80,
                "ThresholdType": "PERCENTAGE"
            },
            "Subscribers": [{"SubscriptionType": "EMAIL", "Address": "ops0@example.com"}]
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: expected CreateNotification({name}) to answer 200, got {status} {body}"
    );
}

/// Move `action_id`'s threshold to `threshold`, which appends one history record
/// carrying the action as that write left it.
async fn update_threshold(
    router: &MultiServiceRouter,
    account_id: &str,
    budget: &str,
    action_id: &str,
    threshold: f64,
) {
    let (status, body) = call(
        router,
        "UpdateBudgetAction",
        &json!({
            "AccountId": account_id,
            "BudgetName": budget,
            "ActionId": action_id,
            "ActionThreshold": {
                "ActionThresholdValue": threshold,
                "ActionThresholdType": "PERCENTAGE"
            }
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: expected UpdateBudgetAction({threshold}) to answer 200, got {status} {body}"
    );
}

/// Create one action on `budget` whose threshold is `threshold`, and answer its minted
/// `ActionId`.
async fn seed_action(
    router: &MultiServiceRouter,
    account_id: &str,
    budget: &str,
    threshold: f64,
) -> String {
    let mut create = simple_create_body(account_id, budget);
    create["ActionThreshold"]["ActionThresholdValue"] = json!(threshold);
    create_action(router, &create, &format!("{budget} at {threshold}")).await
}

/// Seed one account and answer the table of paginated reads over it.
async fn seed_rows(router: &MultiServiceRouter, account_id: &str) -> Vec<Row> {
    // Created in descending name order, so a listing that follows creation order instead
    // of `BudgetName` is a failed assertion.
    seed_daily_budget(router, account_id, HISTORY_BUDGET).await;
    seed_budget(router, account_id, SECOND_ACTION_BUDGET).await;
    seed_budget(router, account_id, ACTION_BUDGET).await;

    // Three actions on the first budget and two on the second, so the account-wide list
    // spans two budgets' actions and the per-budget list only its own.
    let mut first_budget_actions = Vec::new();
    for threshold in [11.0_f64, 12.0, 13.0] {
        first_budget_actions.push(seed_action(router, account_id, ACTION_BUDGET, threshold).await);
    }
    let mut second_budget_actions = Vec::new();
    for threshold in [21.0_f64, 22.0] {
        second_budget_actions
            .push(seed_action(router, account_id, SECOND_ACTION_BUDGET, threshold).await);
    }
    let account_actions: Vec<String> = first_budget_actions
        .iter()
        .chain(second_budget_actions.iter())
        .cloned()
        .collect();

    // The history of the first action: the CREATE record the creation wrote, then one
    // UPDATE record per write below, each carrying its own threshold.
    let history_action = first_budget_actions[0].clone();
    let updates = [31.0_f64, 32.0, 33.0, 34.0];
    for threshold in updates {
        update_threshold(
            router,
            account_id,
            ACTION_BUDGET,
            &history_action,
            threshold,
        )
        .await;
    }
    let mut history_keys = vec![format!("{:.1}", 11.0_f64)];
    history_keys.extend(updates.iter().map(|threshold| format!("{threshold:.1}")));

    seed_notification(router, account_id, ACTION_BUDGET).await;
    seed_notification(router, account_id, SECOND_ACTION_BUDGET).await;

    let mut budgets = vec![
        ACTION_BUDGET.to_string(),
        SECOND_ACTION_BUDGET.to_string(),
        HISTORY_BUDGET.to_string(),
    ];
    budgets.sort();

    let performance_keys = performance_periods(router, account_id).await;

    vec![
        Row {
            case: "DescribeBudgetActionsForBudget, no documented default",
            operation: "DescribeBudgetActionsForBudget",
            request: json!({"AccountId": account_id, "BudgetName": ACTION_BUDGET}),
            page_path: &["Actions"],
            key: action_id,
            documented_default: None,
            modelled_max: SHARED_MAX_RESULTS,
            expected: first_budget_actions,
        },
        Row {
            case: "DescribeBudgetActionsForAccount, no documented default",
            operation: "DescribeBudgetActionsForAccount",
            request: json!({"AccountId": account_id}),
            page_path: &["Actions"],
            key: action_id,
            documented_default: None,
            modelled_max: SHARED_MAX_RESULTS,
            expected: account_actions,
        },
        Row {
            case: "DescribeBudgetActionHistories, no documented default",
            operation: "DescribeBudgetActionHistories",
            request: json!({
                "AccountId": account_id,
                "BudgetName": ACTION_BUDGET,
                "ActionId": &history_action
            }),
            page_path: &["ActionHistories"],
            key: recorded_threshold,
            documented_default: None,
            modelled_max: SHARED_MAX_RESULTS,
            expected: history_keys,
        },
        Row {
            case: "DescribeBudgetPerformanceHistory, no documented default",
            operation: "DescribeBudgetPerformanceHistory",
            request: json!({"AccountId": account_id, "BudgetName": HISTORY_BUDGET}),
            page_path: &["BudgetPerformanceHistory", "BudgetedAndActualAmountsList"],
            key: period_start,
            documented_default: None,
            modelled_max: SHARED_MAX_RESULTS,
            expected: performance_keys,
        },
        Row {
            case: "DescribeBudgets, documented default 100",
            operation: "DescribeBudgets",
            request: json!({"AccountId": account_id}),
            page_path: &["Budgets"],
            key: budget_name,
            documented_default: Some(BUDGETS_DEFAULT_PAGE),
            modelled_max: ACCOUNT_LISTING_MAX_RESULTS,
            expected: budgets,
        },
        Row {
            case: "DescribeBudgetNotificationsForAccount, documented default 50",
            operation: "DescribeBudgetNotificationsForAccount",
            request: json!({"AccountId": account_id}),
            page_path: &["BudgetNotificationsForAccount"],
            key: budget_name,
            documented_default: Some(ACCOUNT_NOTIFICATIONS_DEFAULT_PAGE),
            modelled_max: ACCOUNT_LISTING_MAX_RESULTS,
            expected: vec![
                ACTION_BUDGET.to_string(),
                SECOND_ACTION_BUDGET.to_string(),
            ],
        },
    ]
}

/// The periods the seeded `DAILY` budget's performance history holds, read in one page
/// at the published maximum page size — more than the documented retained window, so the
/// whole series arrives at once — with the series length pinned to what the
/// documentation says a `DAILY` budget retains.
async fn performance_periods(router: &MultiServiceRouter, account_id: &str) -> Vec<String> {
    let (status, body) = call(
        router,
        "DescribeBudgetPerformanceHistory",
        &json!({
            "AccountId": account_id,
            "BudgetName": HISTORY_BUDGET,
            "MaxResults": SHARED_MAX_RESULTS
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: expected the whole performance history to answer 200, got {status} {body}"
    );
    let records = list_at(&body, &["BudgetPerformanceHistory", "BudgetedAndActualAmountsList"]);
    assert_eq!(
        records.len(),
        DAILY_RETAINED_PERIODS,
        "setup: expected the documented DAILY retained window of {DAILY_RETAINED_PERIODS} \
         records, got {}",
        records.len()
    );
    records.iter().map(period_start).collect()
}

/// Walk every page of one read at `max_results`, returning the keys it served in order
/// and the size of each page.
async fn walk(
    router: &MultiServiceRouter,
    row: &Row,
    max_results: Option<usize>,
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
        let page = list_at(&body, row.page_path);
        sizes.push(page.len());
        served.extend(page.iter().map(row.key));
        token = body.get("NextToken").and_then(Value::as_str).map(str::to_string);
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

/// RULE: every one of these reads resolves its effective page size the same way — the
/// size the caller named capped at the published `@range(max)`, and for a request naming
/// none the documented default or a full published-maximum page where the member
/// documents no default. So no page holds more than that size, a page followed by a
/// `NextToken` holds exactly it, the walk takes as many pages as that size needs, and
/// every entry is served exactly once in the one order.
#[tokio::test]
async fn every_paginated_read_resolves_the_same_effective_page_size() {
    let router = router();
    let account_id = caller_account();
    let rows = seed_rows(&router, &account_id).await;

    for row in &rows {
        // Page sizes spanning the class: none at all, the published minimum, a size that
        // divides some of the seeded series and not others, one past the longest series,
        // and the published maximum.
        for max_results in [
            None,
            Some(1),
            Some(2),
            Some(7),
            Some(row.expected.len() + 1),
            Some(row.modelled_max),
        ] {
            let effective = row.effective(max_results);
            let (served, sizes) = walk(&router, row, max_results).await;
            assert_eq!(
                served, row.expected,
                "case {:?} max={max_results:?}: expected every entry exactly once in order, \
                 got {served:?}",
                row.case
            );
            for (index, size) in sizes.iter().enumerate() {
                assert!(
                    *size <= effective,
                    "case {:?} max={max_results:?}: page {index} may hold at most {effective} \
                     entries, got {size}",
                    row.case
                );
                let last = index + 1 == sizes.len();
                assert!(
                    last || *size == effective,
                    "case {:?} max={max_results:?}: a page followed by a NextToken must be \
                     full at {effective}, page {index} held {size}",
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

/// RULE, the bounds: each published bound of an operation's own `MaxResults` is accepted
/// and serves a page no bigger than it, and a size one past either bound is refused at
/// request validation rather than silently resolved to something else.
#[tokio::test]
async fn each_published_page_size_bound_is_served_and_one_past_it_is_refused() {
    let router = router();
    let account_id = caller_account();
    let rows = seed_rows(&router, &account_id).await;

    for row in &rows {
        // `MaxResults` is `@range(min: 1, max: <modelled_max>)`: each bound itself, then
        // one past each of them.
        for max_results in [1, row.modelled_max] {
            let (served, sizes) = walk(&router, row, Some(max_results)).await;
            assert_eq!(
                served, row.expected,
                "case {:?} max={max_results}: a published bound must serve every entry \
                 exactly once in order, got {served:?}",
                row.case
            );
            assert!(
                sizes.iter().all(|size| *size <= max_results),
                "case {:?} max={max_results}: expected no page past the bound, got {sizes:?}",
                row.case
            );
        }
        for max_results in [0, row.modelled_max + 1] {
            let mut request = row.request.clone();
            request["MaxResults"] = json!(max_results);
            let (status, body) = call(&router, row.operation, &request).await;
            assert_eq!(
                status, 400,
                "case {:?} max={max_results}: expected a page size outside the published \
                 range to be refused, got {status} {body}",
                row.case
            );
        }
    }
}
