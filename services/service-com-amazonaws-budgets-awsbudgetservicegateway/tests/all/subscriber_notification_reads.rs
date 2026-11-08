// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for `DescribeSubscribersForNotification` as a READ: which
//! subscribers it reports, and the page size and token rules it reads them through.
//! Driven over the wire through the same `MultiServiceRouter` the production router
//! serves from (the harness is the lifecycle suite's, so every suite in this target
//! drives the service identically).
//!
//! Each test states a RULE and asserts it over a table that spans the class the rule
//! generalises over, not one request:
//!
//! 1. The subscribers reported are exactly the list stored for the notification of the
//!    budget the request addresses — the account, the budget name and what the
//!    notification alerts on together — in stored order; a notification that budget does
//!    not hold holds no subscribers, which is the empty set the read reports by leaving
//!    `Subscribers` out, and a budget the account does not hold is `NotFoundException`.
//! 2. The page size and the continuation token are the modelled ones: a `MaxResults`
//!    inside the published `@range` caps a page and the pages concatenate to the stored
//!    list, one past either published bound is refused at request validation, a
//!    `BudgetName` outside its `@pattern` is the same class of refusal, and a token that
//!    addresses no page of THIS list is `InvalidNextTokenException`.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_budgets_awsbudgetservicegateway::state::MAX_EMAIL_SUBSCRIBERS_PER_NOTIFICATION;

use super::budget_lifecycle::{caller_account, call, is_error, router, same};
use super::client_error_envelopes::message;
use super::notification_lifecycle::seed_budget;

/// The notifications the tables below address.
///
/// The rows differ along every member a notification's identity is made of — the
/// notification type, the comparison operator, the threshold (each published
/// `@range` bound of `Notification$Threshold`, 0 and 15000000000000) and the threshold
/// type, absent on the first row (`smithy_query(query="shape",shape="Notification")`).
fn notification_catalog() -> Vec<Value> {
    vec![
        json!({
            "NotificationType": "ACTUAL",
            "ComparisonOperator": "GREATER_THAN",
            "Threshold": 80
        }),
        json!({
            "NotificationType": "FORECASTED",
            "ComparisonOperator": "EQUAL_TO",
            "Threshold": 0,
            "ThresholdType": "ABSOLUTE_VALUE"
        }),
        json!({
            "NotificationType": "ACTUAL",
            "ComparisonOperator": "LESS_THAN",
            "Threshold": 15_000_000_000_000.0_f64,
            "ThresholdType": "PERCENTAGE"
        }),
    ]
}

/// The subscribers stored for one cell of the scope × notification table.
///
/// Every cell holds a DIFFERENT set, so a read that reported another cell's subscribers
/// is visible, and the sets differ along the count and the subscription-type mix. The
/// first cell holds the modelled maximum: `Subscribers` is `@length(min: 1, max: 11)` —
/// "one SNS subscriber and up to 10 email subscribers, for a total of 11 subscribers".
fn subscriber_set(scope: usize, notification: usize) -> Value {
    let emails = if (scope, notification) == (0, 0) {
        MAX_EMAIL_SUBSCRIBERS_PER_NOTIFICATION
    } else {
        1 + (scope + notification) % 3
    };
    let mut list = Vec::new();
    if (scope + notification).is_multiple_of(2) {
        list.push(json!({
            "SubscriptionType": "SNS",
            "Address": format!(
                "arn:aws:sns:us-east-1:{}:budget-s{scope}-n{notification}",
                caller_account()
            )
        }));
    }
    list.extend((0..emails).map(|index| {
        json!({
            "SubscriptionType": "EMAIL",
            "Address": format!("s{scope}n{notification}-{index}@example.com")
        })
    }));
    Value::Array(list)
}

/// Attach `notification` to `budget_name` with `subscribers`, and fail the test if the
/// setup call did not take.
async fn seed_notification(
    router: &MultiServiceRouter,
    account_id: &str,
    budget_name: &str,
    notification: &Value,
    subscribers: &Value,
) {
    let (status, body) = call(
        router,
        "CreateNotification",
        &json!({
            "AccountId": account_id,
            "BudgetName": budget_name,
            "Notification": notification,
            "Subscribers": subscribers
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: expected CreateNotification on {budget_name:?} to answer 200, \
         got {status} {body}"
    );
}

/// One `DescribeSubscribersForNotification` call, with the optional paging members set
/// only when the row supplies them.
async fn read_subscribers(
    router: &MultiServiceRouter,
    account_id: &str,
    budget_name: &str,
    notification: &Value,
    max_results: Option<i64>,
    next_token: Option<&str>,
) -> (u16, Value) {
    let mut request = json!({
        "AccountId": account_id,
        "BudgetName": budget_name,
        "Notification": notification
    });
    if let Some(max) = max_results {
        request["MaxResults"] = json!(max);
    }
    if let Some(token) = next_token {
        request["NextToken"] = json!(token);
    }
    call(router, "DescribeSubscribersForNotification", &request).await
}

/// One scope the subscribers are stored under: an account, a budget it holds, and the
/// notifications of [`notification_catalog`] that budget carries.
struct Scope {
    case: &'static str,
    account_id: String,
    budget_name: &'static str,
    /// Indices into [`notification_catalog`].
    held: Vec<usize>,
}

/// RULE 1: the subscribers reported are exactly those stored for the notification of the
/// budget the request addresses, in stored order — a notification the budget does not hold
/// reports no subscribers rather than another notification's, and a budget the account
/// does not hold is `NotFoundException`.
///
/// The rows span what the addressing generalises over: one account holding two budgets,
/// each budget holding a different subset of the notification catalog, every cell with
/// its own subscriber set.
#[tokio::test]
async fn subscribers_are_reported_for_the_addressed_notification_only() {
    let router = router();
    let catalog = notification_catalog();
    let only = caller_account();
    let scopes = [
        Scope {
            case: "the account's first budget",
            account_id: only.clone(),
            budget_name: "first-name",
            held: vec![0, 1],
        },
        Scope {
            case: "a second budget of the same account",
            account_id: only.clone(),
            budget_name: "second-name",
            held: vec![0, 2],
        },
        Scope {
            case: "a third budget of the same account",
            account_id: only.clone(),
            budget_name: "other-name",
            held: vec![2],
        },
    ];

    for (index, scope) in scopes.iter().enumerate() {
        seed_budget(&router, &scope.account_id, scope.budget_name).await;
        for held in &scope.held {
            seed_notification(
                &router,
                &scope.account_id,
                scope.budget_name,
                &catalog[*held],
                &subscriber_set(index, *held),
            )
            .await;
        }
    }

    for (index, scope) in scopes.iter().enumerate() {
        for (position, notification) in catalog.iter().enumerate() {
            let case = format!("{} / notification {position}", scope.case);
            let (status, body) = read_subscribers(
                &router,
                &scope.account_id,
                scope.budget_name,
                notification,
                None,
                None,
            )
            .await;
            if scope.held.contains(&position) {
                let expected = subscriber_set(index, position);
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200, got {status} {body}"
                );
                assert!(
                    same(&body["Subscribers"], &expected),
                    "case {case:?}: expected {expected}, got {body}"
                );
                assert!(
                    body.get("NextToken").is_none_or(Value::is_null),
                    "case {case:?}: a whole list leaves nothing to continue, got {body}"
                );
            } else {
                // A notification this budget does not hold holds no subscribers, which is
                // the EMPTY set this read reports by leaving `Subscribers` out — the list
                // is `@length(min: 1)`, so no empty list can be serialized for it.
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200 for a notification the budget does not \
                     hold, got {status} {body}"
                );
                assert!(
                    body.get("Subscribers").is_none_or(Value::is_null),
                    "case {case:?}: expected no Subscribers member, got {body}"
                );
            }
        }
    }

    // The budget itself has to be one this account holds.
    for (case, budget_name) in [
        ("a budget name the account holds nothing under", "absent-name"),
        ("a stored name in another case", "FIRST-NAME"),
    ] {
        let (status, body) =
            read_subscribers(&router, &only, budget_name, &catalog[0], None, None).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the refusal at 400, got {status} {body}"
        );
        assert!(
            is_error(&body, "NotFoundException"),
            "case {case:?}: expected NotFoundException, got {body}"
        );
    }
}

/// RULE 2: the page size and the continuation token are the modelled ones.
///
/// `MaxResults` is `@range(min: 1, max: 100)`
/// (`smithy_query(query="shape",shape="MaxResults")`), so each published bound is
/// accepted and caps a page while the pages concatenate to the stored list, and one past
/// either bound is refused at request validation — as is a `BudgetName` outside its
/// `@pattern`. A token addresses a page of the list it was minted for, so one naming an
/// offset THIS list never had a page at is `InvalidNextTokenException`.
#[tokio::test]
async fn the_page_size_bounds_and_the_token_scope_are_the_modelled_ones() {
    let router = router();
    let account_id = caller_account();
    let name = "paged-bounds";
    seed_budget(&router, &account_id, name).await;

    let catalog = notification_catalog();
    let long = &catalog[0];
    let short = &catalog[1];
    // The modelled maximum (11) beside a notification holding a single subscriber, which
    // is the modelled minimum (`Subscribers` is `@length(min: 1, max: 11)`).
    let long_set = subscriber_set(0, 0);
    let short_set = json!([{"SubscriptionType": "EMAIL", "Address": "single@example.com"}]);
    seed_notification(&router, &account_id, name, long, &long_set).await;
    seed_notification(&router, &account_id, name, short, &short_set).await;
    let stored = long_set.as_array().expect("a list of subscribers").clone();

    // ACCEPTED: each published bound of the range, and no page size at all — the sizes
    // BETWEEN the bounds are the lifecycle suite's walk, not restated here.
    for max_results in [Some(1), Some(100), None] {
        let case = format!("MaxResults {max_results:?}");
        let mut collected: Vec<Value> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let (status, body) = read_subscribers(
                &router,
                &account_id,
                name,
                long,
                max_results,
                token.as_deref(),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} {body}"
            );
            let page = body["Subscribers"].as_array().cloned().unwrap_or_default();
            let cap = max_results.unwrap_or(i64::MAX);
            let got = i64::try_from(page.len()).unwrap_or(i64::MAX);
            assert!(
                got >= 1 && got <= cap,
                "case {case:?}: expected a page of 1..={cap} subscribers, got {got}"
            );
            // No page size at all is "every remaining item", so the first page is the
            // whole list and there is nothing to continue.
            if max_results.is_none() {
                assert!(
                    body.get("NextToken").is_none_or(Value::is_null),
                    "case {case:?}: expected the whole list in one page, got {body}"
                );
            }
            collected.extend(page);
            assert!(
                collected.len() <= stored.len(),
                "case {case:?}: expected the walk to stop at {} subscribers, got {}",
                stored.len(),
                collected.len()
            );
            token = body["NextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert!(
            same(&Value::Array(collected.clone()), &long_set),
            "case {case:?}: expected the pages to concatenate to the stored list, \
             got {collected:?}"
        );
    }

    // REFUSED at request validation: one past each published bound of `MaxResults`. The
    // message names the failing member path and the constraint it violated.
    for (case, max_results, constraint) in [
        (
            "a page size below the published minimum",
            0_i64,
            "Member must have value greater than or equal to 1",
        ),
        (
            "a negative page size",
            -1_i64,
            "Member must have value greater than or equal to 1",
        ),
        (
            "a page size above the published maximum",
            101_i64,
            "Member must have value less than or equal to 100",
        ),
    ] {
        let (status, body) =
            read_subscribers(&router, &account_id, name, long, Some(max_results), None).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the refusal at 400, got {status} {body}"
        );
        assert!(
            is_error(&body, "ValidationException"),
            "case {case:?}: expected ValidationException, got {body}"
        );
        let text = message(&body);
        assert!(
            text.contains("maxResults") && text.contains(constraint),
            "case {case:?}: expected a message naming maxResults and {constraint:?}, \
             got {text:?}"
        );
    }

    // The same class of refusal for a `BudgetName` outside its `@pattern`
    // (`^(?![^:\\]*/action/|(?i).*<script>.*</script>.*)[^:\\]+$` —
    // `smithy_query(query="shape",shape="BudgetName")`), including a form the suites
    // that cover the create path do not drive through this read.
    for (case, budget_name) in [
        ("a colon", "a:b"),
        ("a backslash", "a\\b"),
        ("the /action/ substring", "a/action/b"),
        ("a script element", "a<SCRIPT>alert(1)</script>b"),
    ] {
        let (status, body) =
            read_subscribers(&router, &account_id, budget_name, long, None, None).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the refusal at 400, got {status} {body}"
        );
        assert!(
            is_error(&body, "ValidationException"),
            "case {case:?}: expected ValidationException, got {body}"
        );
        let text = message(&body);
        assert!(
            text.contains("budgetName")
                && text.contains("Member must satisfy regular expression pattern"),
            "case {case:?}: expected a message naming budgetName and the pattern \
             constraint, got {text:?}"
        );
    }

    // A token addresses a page of the list it was minted for: the one minted walking the
    // eleven-subscriber notification names an offset the single-subscriber notification
    // never had a page at, so it is invalid there — and still continues exactly where
    // that page stopped on the list it came from.
    let (status, body) = read_subscribers(&router, &account_id, name, long, Some(1), None).await;
    assert_eq!(
        status, 200,
        "expected the first page of one to answer 200, got {status} {body}"
    );
    let token = body["NextToken"]
        .as_str()
        .expect("a first page of 1 of 11 must carry a continuation token")
        .to_string();

    let (status, body) =
        read_subscribers(&router, &account_id, name, short, None, Some(&token)).await;
    assert_eq!(
        status, 400,
        "expected a token minted for another list to be refused at 400, got {status} {body}"
    );
    assert!(
        is_error(&body, "InvalidNextTokenException"),
        "expected InvalidNextTokenException, got {body}"
    );

    let (status, body) =
        read_subscribers(&router, &account_id, name, long, None, Some(&token)).await;
    let rest = Value::Array(stored[1..].to_vec());
    assert_eq!(
        status, 200,
        "expected the token to continue its own list, got {status} {body}"
    );
    assert!(
        same(&body["Subscribers"], &rest),
        "expected the subscribers after the first, {rest}, got {body}"
    );

    // Nothing the refusals touched: both lists still read back whole.
    for (case, notification, expected) in [
        ("the eleven-subscriber notification", long, &long_set),
        ("the single-subscriber notification", short, &short_set),
    ] {
        let (status, body) =
            read_subscribers(&router, &account_id, name, notification, None, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body}"
        );
        assert!(
            same(&body["Subscribers"], expected),
            "case {case:?}: expected {expected}, got {body}"
        );
    }
}
