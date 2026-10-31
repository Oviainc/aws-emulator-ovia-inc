// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the page size the two PER-NOTIFICATION paginated reads serve —
//! `DescribeNotificationsForBudget` and `DescribeSubscribersForNotification` — driven over
//! the wire through the same `MultiServiceRouter` the production router serves from.
//!
//! RULE: a paginated read serves at most the page size the caller named, never more than
//! the `@range(max)` its own `MaxResults` member publishes; a request naming no
//! `MaxResults` is served the page size that member documents as the default, or — where
//! it documents none — a full page at that published maximum. Both of these reads
//! document no default and publish the same maximum, so each resolves its effective page
//! size exactly as every other paginated read of this service does, and a request naming
//! no size is answered byte-for-byte as one naming that published maximum.
//!
//! The table's rows are one per (operation, list length): the notifications of a budget
//! holding the per-budget maximum, three and one, and the subscribers of a notification
//! holding the modelled `Subscribers` maximum, five and one — so the rows differ along
//! which list is paged and how many entries it has, and are read at page sizes spanning
//! the published range.
//!
//! The harness (`router`, `call`, `caller_account`) is the lifecycle suite's, so every
//! suite in this test target drives the service exactly the same way. The rows live here
//! rather than in the sibling page-size suite's table because that suite's own table,
//! `Row` and walk are private to it and its assertions are the ones this refactor must
//! leave untouched; nothing of it is reachable without editing it.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::budget_lifecycle::{call, caller_account, router};

/// The published `@range(max)` of the `MaxResults` member both of these reads share:
/// `@range(min: 1, max: 100)` — "An integer that represents how many entries a paginated
/// response contains. The maximum is 100."
/// (`smithy_query(query="operation",operation="DescribeNotificationsForBudget,DescribeSubscribersForNotification")`).
const PUBLISHED_MAX_RESULTS: usize = 100;

/// The published `@range(min)` of that same `MaxResults` member.
const PUBLISHED_MIN_RESULTS: usize = 1;

/// How many notifications one budget holds: "Each budget can have up to 10 notifications"
/// (`CreateNotification`), which is also the `@length(max: 10)` of
/// `NotificationWithSubscribersList`.
const MAX_NOTIFICATIONS_PER_BUDGET: usize = 10;

/// How many subscribers one notification holds: "A notification can have one SNS
/// subscriber and up to 10 email subscribers, for a total of 11 subscribers"
/// (`Notification`), the modelled `Subscribers` `@length(min: 1, max: 11)`.
const MAX_SUBSCRIBERS_PER_NOTIFICATION: usize = 11;

/// The budget holding the per-budget maximum of notifications, whose first notification
/// holds the per-notification maximum of subscribers and whose second holds five.
const WIDE_BUDGET: &str = "page-notified-wide";

/// A budget holding three notifications, each with one subscriber.
const NARROW_BUDGET: &str = "page-notified-narrow";

/// A budget holding a single notification with a single subscriber.
const SINGLE_BUDGET: &str = "page-notified-single";

/// The identity of one served notification or of the notification a subscriber row reads:
/// its `Threshold`, the member that makes two notifications of one budget distinct.
/// `Notification$Threshold` is modelled a double, so the key is formatted rather than
/// compared as JSON text.
fn threshold_key(threshold: f64) -> String {
    format!("{threshold:.1}")
}

/// One notification, distinguished from its siblings by `Threshold`.
fn notification(threshold: usize) -> Value {
    json!({
        "NotificationType": "ACTUAL",
        "ComparisonOperator": "GREATER_THAN",
        "Threshold": threshold,
        "ThresholdType": "PERCENTAGE"
    })
}

/// The address of the `index`th EMAIL subscriber of one set.
fn email_address(index: usize) -> String {
    format!("ops{index:02}@example.com")
}

/// The ARN of the SNS topic one set's single SNS subscriber publishes to.
fn sns_address() -> String {
    format!("arn:aws:sns:us-east-1:{}:budget-alerts", caller_account())
}

/// A subscriber set of `count` subscribers, in the order a read must serve them: the one
/// SNS subscriber a set may hold, then EMAIL subscribers up to the documented ten.
fn subscriber_set(count: usize) -> Vec<Value> {
    (0..count)
        .map(|index| {
            if index == 0 {
                json!({"SubscriptionType": "SNS", "Address": sns_address()})
            } else {
                json!({"SubscriptionType": "EMAIL", "Address": email_address(index)})
            }
        })
        .collect()
}

/// The addresses of [`subscriber_set`], in that same order — the keys a subscriber read is
/// compared on.
fn subscriber_keys(count: usize) -> Vec<String> {
    (0..count)
        .map(|index| {
            if index == 0 {
                sns_address()
            } else {
                email_address(index)
            }
        })
        .collect()
}

/// Create one budget carrying `notifications` — `(threshold, how many subscribers)` per
/// notification — and fail the test if the setup call did not take.
async fn seed_budget(
    router: &MultiServiceRouter,
    account_id: &str,
    name: &str,
    notifications: &[(usize, usize)],
) {
    let request = json!({
        "AccountId": account_id,
        "Budget": {
            "BudgetName": name,
            "TimeUnit": "MONTHLY",
            "BudgetType": "COST",
            "BudgetLimit": {"Amount": "100", "Unit": "USD"}
        },
        "NotificationsWithSubscribers": Value::Array(
            notifications
                .iter()
                .map(|(threshold, how_many)| {
                    json!({
                        "Notification": notification(*threshold),
                        "Subscribers": Value::Array(subscriber_set(*how_many))
                    })
                })
                .collect(),
        )
    });
    let (status, body) = call(router, "CreateBudget", &request).await;
    assert_eq!(
        status, 200,
        "setup: expected CreateBudget({name}) to answer 200, got {status} {body}"
    );
}

/// One row of the table: a paginated read, what it pages over, and every entry it must
/// serve in the order it must serve them.
struct Row {
    case: &'static str,
    operation: &'static str,
    /// The output member carrying the page.
    list_member: &'static str,
    /// The request, without `MaxResults` or `NextToken`.
    request: Value,
    /// The identity of one served entry, so a mis-paged entry is visible.
    key: fn(&Value) -> String,
    /// Every entry the read must serve, in order.
    expected: Vec<String>,
}

/// Seed one account and answer the table of per-notification reads over it.
async fn seed_rows(router: &MultiServiceRouter, account_id: &str) -> Vec<Row> {
    // The first notification of the wide budget holds the per-notification maximum of
    // subscribers, the second five, the rest one each.
    let wide: Vec<(usize, usize)> = (1..=MAX_NOTIFICATIONS_PER_BUDGET)
        .map(|threshold| match threshold {
            1 => (threshold, MAX_SUBSCRIBERS_PER_NOTIFICATION),
            2 => (threshold, 5),
            _ => (threshold, 1),
        })
        .collect();
    let narrow: Vec<(usize, usize)> = (1..=3).map(|threshold| (threshold, 1)).collect();

    seed_budget(router, account_id, WIDE_BUDGET, &wide).await;
    seed_budget(router, account_id, NARROW_BUDGET, &narrow).await;
    seed_budget(router, account_id, SINGLE_BUDGET, &[(1, 1)]).await;

    let notification_keys = |held: &[(usize, usize)]| -> Vec<String> {
        held.iter()
            .map(|(threshold, _)| threshold_key(*threshold as f64))
            .collect()
    };
    let notification_row = |case: &'static str, budget: &str, held: &[(usize, usize)]| Row {
        case,
        operation: "DescribeNotificationsForBudget",
        list_member: "Notifications",
        request: json!({"AccountId": account_id, "BudgetName": budget}),
        key: |entry| threshold_key(entry["Threshold"].as_f64().unwrap_or_default()),
        expected: notification_keys(held),
    };
    let subscriber_row = |case: &'static str, budget: &str, threshold: usize, count: usize| Row {
        case,
        operation: "DescribeSubscribersForNotification",
        list_member: "Subscribers",
        request: json!({
            "AccountId": account_id,
            "BudgetName": budget,
            "Notification": notification(threshold)
        }),
        key: |entry| entry["Address"].as_str().unwrap_or_default().to_string(),
        expected: subscriber_keys(count),
    };

    vec![
        notification_row(
            "DescribeNotificationsForBudget over the per-budget maximum of notifications",
            WIDE_BUDGET,
            &wide,
        ),
        notification_row(
            "DescribeNotificationsForBudget over three notifications",
            NARROW_BUDGET,
            &narrow,
        ),
        notification_row(
            "DescribeNotificationsForBudget over one notification",
            SINGLE_BUDGET,
            &[(1, 1)],
        ),
        subscriber_row(
            "DescribeSubscribersForNotification over the per-notification maximum",
            WIDE_BUDGET,
            1,
            MAX_SUBSCRIBERS_PER_NOTIFICATION,
        ),
        subscriber_row(
            "DescribeSubscribersForNotification over five subscribers",
            WIDE_BUDGET,
            2,
            5,
        ),
        subscriber_row(
            "DescribeSubscribersForNotification over one subscriber",
            SINGLE_BUDGET,
            1,
            1,
        ),
    ]
}

/// The request one row makes at `max_results`, following `token` when there is one.
fn request_for(row: &Row, max_results: Option<usize>, token: Option<&str>) -> Value {
    let mut request = row.request.clone();
    if let Some(max) = max_results {
        request["MaxResults"] = json!(max);
    }
    if let Some(held) = token {
        request["NextToken"] = json!(held);
    }
    request
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
        let request = request_for(row, max_results, token.as_deref());
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

/// The effective page size a row is served for `max_results`: what the caller named,
/// capped at the published maximum; and for a caller that named nothing, a full page at
/// that published maximum, since neither member documents a default.
fn effective(max_results: Option<usize>) -> usize {
    max_results
        .unwrap_or(PUBLISHED_MAX_RESULTS)
        .clamp(PUBLISHED_MIN_RESULTS, PUBLISHED_MAX_RESULTS)
}

/// RULE: both per-notification reads resolve their effective page size the one way every
/// paginated read of this service does — the size the caller named capped at the published
/// `@range(max)`, and a full published-maximum page for a request naming none, since
/// neither member documents a default. So no page holds more than that size, a page
/// followed by a `NextToken` holds exactly it, the walk takes as many pages as that size
/// needs, and every entry is served exactly once in the one order.
#[tokio::test]
async fn both_per_notification_reads_resolve_the_shared_effective_page_size() {
    let router = router();
    let account_id = caller_account();
    let rows = seed_rows(&router, &account_id).await;

    for row in &rows {
        // Page sizes spanning the class: none at all, the published minimum, a size that
        // divides some of the seeded lists and not others, one past the longest list, and
        // the published maximum.
        for max_results in [
            None,
            Some(PUBLISHED_MIN_RESULTS),
            Some(2),
            Some(4),
            Some(row.expected.len() + 1),
            Some(PUBLISHED_MAX_RESULTS),
        ] {
            let effective = effective(max_results);
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
                    "case {:?} max={max_results:?}: a page followed by a NextToken must be full \
                     at {effective}, page {index} held {size}",
                    row.case
                );
            }
            let expected_pages = row.expected.len().div_ceil(effective).max(1);
            assert_eq!(
                sizes.len(),
                expected_pages,
                "case {:?} max={max_results:?}: expected {expected_pages} pages for {} entries, \
                 got {sizes:?}",
                row.case,
                row.expected.len()
            );
        }
    }
}

/// RULE, the absent size: a request naming no `MaxResults` is served the page an operation
/// whose member documents no default resolves to — a full page at the published maximum —
/// so its whole response is the one a request naming that maximum is answered with.
#[tokio::test]
async fn an_absent_page_size_answers_as_the_published_maximum_does() {
    let router = router();
    let account_id = caller_account();
    let rows = seed_rows(&router, &account_id).await;

    for row in &rows {
        let (absent_status, absent) =
            call(&router, row.operation, &request_for(row, None, None)).await;
        let (named_status, named) = call(
            &router,
            row.operation,
            &request_for(row, Some(PUBLISHED_MAX_RESULTS), None),
        )
        .await;
        assert_eq!(
            absent_status, 200,
            "case {:?}: expected no MaxResults to answer 200, got {absent_status} {absent}",
            row.case
        );
        assert_eq!(
            named_status, 200,
            "case {:?}: expected the published maximum to answer 200, got {named_status} {named}",
            row.case
        );
        assert_eq!(
            absent, named,
            "case {:?}: expected an absent MaxResults to answer exactly as the published \
             maximum of {PUBLISHED_MAX_RESULTS} does, got {absent} and {named}",
            row.case
        );
        assert!(
            absent["NextToken"].is_null(),
            "case {:?}: a list shorter than the published maximum is served whole, so no \
             NextToken is minted, got {}",
            row.case,
            absent["NextToken"]
        );
    }
}

/// RULE, the bounds: each published bound of `MaxResults` is accepted and serves a page no
/// bigger than it, and a size one past either bound is refused at request validation
/// rather than silently resolved to something else.
#[tokio::test]
async fn each_published_bound_of_the_shared_page_size_is_served_and_one_past_it_is_refused() {
    let router = router();
    let account_id = caller_account();
    let rows = seed_rows(&router, &account_id).await;

    for row in &rows {
        for max_results in [PUBLISHED_MIN_RESULTS, PUBLISHED_MAX_RESULTS] {
            let (served, sizes) = walk(&router, row, Some(max_results)).await;
            assert_eq!(
                served, row.expected,
                "case {:?} max={max_results}: a published bound must serve every entry exactly \
                 once in order, got {served:?}",
                row.case
            );
            assert!(
                sizes.iter().all(|size| *size <= max_results),
                "case {:?} max={max_results}: expected no page past the bound, got {sizes:?}",
                row.case
            );
        }
        for max_results in [PUBLISHED_MIN_RESULTS - 1, PUBLISHED_MAX_RESULTS + 1] {
            let request = request_for(row, Some(max_results), None);
            let (status, body) = call(&router, row.operation, &request).await;
            assert_eq!(
                status, 400,
                "case {:?} max={max_results}: expected a page size outside the published range \
                 to be refused, got {status} {body}",
                row.case
            );
        }
    }
}
