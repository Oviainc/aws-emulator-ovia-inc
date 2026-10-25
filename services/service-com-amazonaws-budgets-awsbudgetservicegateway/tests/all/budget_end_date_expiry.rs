// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the end of a budget's life.
//!
//! RULE: `Budget$TimePeriod` bounds a budget's life — "After the end date, Amazon Web
//! Services deletes the budget and all the associated notifications and subscribers" — so
//! a budget whose `TimePeriod$End` lies in the past is no longer part of the account's
//! state: it is absent from every listing, every operation that names it answers
//! `NotFoundException`, and it stops counting against the account's budget quota. A budget
//! whose end date is still ahead is held, whatever its granularity or type.
//!
//! Each test states that rule and asserts it over a table spanning what the rule
//! generalises over — how far the end date is from now (years, a day, a second, a second
//! ahead, the documented maximum), the budget's `TimeUnit` and `BudgetType`, and the
//! resources hanging off the budget. The request the issue recorded (CUSTOM/COST, start
//! 2020-01-01, end 2021-01-01) is one row of the first table.
//!
//! The harness (`router`, `call`, `is_error`, `account`, `civil_secs`) is the one
//! `tests/all/budget_lifecycle.rs` builds, so every suite drives the service identically.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use service_com_amazonaws_budgets_awsbudgetservicegateway::ops::types::{Budget, TimePeriod};
use service_com_amazonaws_budgets_awsbudgetservicegateway::state::{
    ComAmazonawsBudgetsAwsbudgetservicegatewayState, StoredBudget, ARN_SERVICE,
    MAX_BUDGETS_PER_ACCOUNT,
};

use super::budget_lifecycle::{caller_account, call, civil_secs, is_error, router, router_with_engine};

/// Seconds in a day, for a row that puts an end date a whole period either side of now.
const DAY_SECS: i64 = 86_400;

/// The status this service renders a client-side modelled failure at — the same status
/// `tests/all/budget_lifecycle.rs` asserts for `NotFoundException` after a delete.
const CLIENT_ERROR_STATUS: u16 = 400;

/// Now, in epoch seconds.
fn now_secs() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the test host's clock is after the epoch")
            .as_secs(),
    )
    .expect("an epoch that fits in i64")
}

/// `arn:{partition}:budgets::{account}:budget/{BudgetName}` — the ARN form the tagging
/// operations name a budget by (a global service, so the region field is empty).
fn budget_arn(account_id: &str, budget_name: &str) -> String {
    format!("arn:aws:{ARN_SERVICE}::{account_id}:budget/{budget_name}")
}

/// One row of the end-date table: a budget to create, and whether the clock has already
/// made its deletion due.
struct Row {
    case: &'static str,
    time_unit: &'static str,
    budget_type: &'static str,
    /// `TimePeriod$Start`, in epoch seconds.
    start: i64,
    /// `TimePeriod$End`, in epoch seconds.
    end: i64,
    /// Whether the budget is past its end date, so the account no longer holds it.
    expired: bool,
}

/// The table: the end date varies from years past to the documented maximum, and the
/// granularity and type vary with it, so the rule is asserted over the class rather than
/// over the recorded request.
///
/// `06/15/87 00:00 UTC` is the last end date `Budget$TimePeriod` allows ("The end date
/// must come before `06/15/87 00:00 UTC`"), and the default for a budget written without
/// one — the published bound, tested here as a row of its own.
fn rows() -> Vec<Row> {
    let now = now_secs();
    vec![
        // The recorded instance: start 2020-01-01, end 2021-01-01, both long past.
        Row {
            case: "ended-years-ago",
            time_unit: "CUSTOM",
            budget_type: "COST",
            start: civil_secs("2020-01-01T00:00:00Z"),
            end: civil_secs("2021-01-01T00:00:00Z"),
            expired: true,
        },
        Row {
            case: "ended-a-day-ago",
            time_unit: "DAILY",
            budget_type: "USAGE",
            start: now - 2 * DAY_SECS,
            end: now - DAY_SECS,
            expired: true,
        },
        Row {
            case: "ended-a-second-ago",
            time_unit: "MONTHLY",
            budget_type: "RI_UTILIZATION",
            start: now - DAY_SECS,
            end: now - 1,
            expired: true,
        },
        Row {
            case: "ends-tomorrow",
            time_unit: "DAILY",
            budget_type: "COST",
            start: now - DAY_SECS,
            end: now + DAY_SECS,
            expired: false,
        },
        Row {
            case: "ends-next-year",
            time_unit: "ANNUALLY",
            budget_type: "USAGE",
            start: now - DAY_SECS,
            end: now + 365 * DAY_SECS,
            expired: false,
        },
        Row {
            case: "ends-at-the-documented-maximum",
            time_unit: "QUARTERLY",
            budget_type: "COST",
            start: now - DAY_SECS,
            end: civil_secs("2087-06-15T00:00:00Z"),
            expired: false,
        },
    ]
}

/// The `CreateBudget$Budget` a row describes. A `COST` or `USAGE` budget carries the
/// `BudgetLimit` it requires; a utilization budget's limit is the one the service defaults
/// ("RI and Savings Plans utilization or coverage budgets default to `100`"), so the row
/// sends none.
fn budget_json(row: &Row) -> Value {
    let mut budget = json!({
        "BudgetName": row.case,
        "TimeUnit": row.time_unit,
        "BudgetType": row.budget_type,
        "TimePeriod": {"Start": row.start, "End": row.end}
    });
    if matches!(row.budget_type, "COST" | "USAGE") {
        budget["BudgetLimit"] = json!({"Amount": "17", "Unit": "USD"});
    }
    budget
}

/// Every `BudgetName` `DescribeBudgets` reports for `account_id`, failing the test on
/// anything but a 200.
async fn listed_names(router: &MultiServiceRouter, account_id: &str) -> Vec<String> {
    let (status, body) = call(
        router,
        "DescribeBudgets",
        &json!({"AccountId": account_id, "MaxResults": 1000}),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected DescribeBudgets to answer 200, got {status} {body}"
    );
    body["Budgets"]
        .as_array()
        .map(|budgets| {
            budgets
                .iter()
                .filter_map(|budget| budget["BudgetName"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// RULE: a budget is part of the account's state only while its `TimePeriod$End` is still
/// ahead; past that instant the account holds it no longer, so it is absent from
/// `DescribeBudgets` and every operation naming it answers `NotFoundException`.
#[tokio::test]
async fn a_budget_is_held_only_until_its_end_date_has_passed() {
    let router = router();
    let account_id = caller_account();
    let table = rows();

    for row in &table {
        let (status, body) = call(
            &router,
            "CreateBudget",
            &json!({"AccountId": &account_id, "Budget": budget_json(row)}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {}: expected CreateBudget to answer 200, got {status} {body}",
            row.case
        );
    }

    let listed = listed_names(&router, &account_id).await;
    let mut expected: Vec<&str> = table
        .iter()
        .filter(|row| !row.expired)
        .map(|row| row.case)
        .collect();
    expected.sort_unstable();
    assert_eq!(
        listed,
        expected,
        "case listing: expected DescribeBudgets to report only the budgets whose end date \
         is still ahead ({expected:?}), got {listed:?}"
    );

    for row in &table {
        let named = json!({"AccountId": &account_id, "BudgetName": row.case});
        let (status, body) = call(&router, "DescribeBudget", &named).await;
        if row.expired {
            assert_eq!(
                status, CLIENT_ERROR_STATUS,
                "case {}: expected DescribeBudget on a budget past its end date to be \
                 refused, got {status} {body}",
                row.case
            );
            assert!(
                is_error(&body, "NotFoundException"),
                "case {}: expected NotFoundException, got {body}",
                row.case
            );
        } else {
            assert_eq!(
                status, 200,
                "case {}: expected DescribeBudget to answer 200, got {status} {body}",
                row.case
            );
            let reported = body["Budget"]["TimePeriod"]["End"].as_f64();
            assert!(
                reported.is_some_and(|end| (end - row.end as f64).abs() < 1.0),
                "case {}: expected the stored end date {}, got {reported:?}",
                row.case,
                row.end
            );
        }
    }

    // Every other operation that resolves a budget by name treats the expired one as the
    // resource it is: gone.
    for row in table.iter().filter(|row| row.expired) {
        let named = json!({"AccountId": &account_id, "BudgetName": row.case});
        for operation in [
            "DeleteBudget",
            "DescribeNotificationsForBudget",
            "DescribeBudgetActionsForBudget",
        ] {
            let (status, body) = call(&router, operation, &named).await;
            assert_eq!(
                status, CLIENT_ERROR_STATUS,
                "case {} {operation}: expected a refusal for a budget past its end date, \
                 got {status} {body}",
                row.case
            );
            assert!(
                is_error(&body, "NotFoundException"),
                "case {} {operation}: expected NotFoundException, got {body}",
                row.case
            );
        }
    }
}

/// RULE: the budget's own resources go with it — the notifications, their subscribers and
/// its tags are held inside the budget, so once its end date has passed every read and
/// write of them answers `NotFoundException` too, while an otherwise identical budget
/// whose end date is ahead keeps serving all of them.
#[tokio::test]
async fn an_expired_budget_takes_its_notifications_subscribers_and_tags_with_it() {
    let router = router();
    let account_id = caller_account();
    let now = now_secs();
    let notification = json!({
        "NotificationType": "ACTUAL",
        "ComparisonOperator": "GREATER_THAN",
        "Threshold": 80
    });
    let subscriber = json!({"SubscriptionType": "EMAIL", "Address": "ops@example.com"});

    // The two rows differ only in where the end date sits relative to now.
    for (case, end, expired) in [
        ("ended", now - DAY_SECS, true),
        ("running", now + 365 * DAY_SECS, false),
    ] {
        let (status, body) = call(
            &router,
            "CreateBudget",
            &json!({
                "AccountId": &account_id,
                "Budget": {
                    "BudgetName": case,
                    "TimeUnit": "MONTHLY",
                    "BudgetType": "COST",
                    "BudgetLimit": {"Amount": "5", "Unit": "USD"},
                    "TimePeriod": {"Start": now - 2 * DAY_SECS, "End": end}
                },
                "NotificationsWithSubscribers": [
                    {"Notification": notification.clone(), "Subscribers": [subscriber.clone()]}
                ],
                "ResourceTags": [{"Key": "owner", "Value": "finance"}]
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: expected CreateBudget to answer 200, got {status} {body}"
        );

        let named = json!({"AccountId": &account_id, "BudgetName": case});
        let requests: Vec<(&str, Value)> = vec![
            ("DescribeNotificationsForBudget", named.clone()),
            (
                "DescribeSubscribersForNotification",
                json!({
                    "AccountId": &account_id,
                    "BudgetName": case,
                    "Notification": notification.clone()
                }),
            ),
            (
                "ListTagsForResource",
                json!({"ResourceARN": budget_arn(&account_id, case)}),
            ),
            (
                "TagResource",
                json!({
                    "ResourceARN": budget_arn(&account_id, case),
                    "ResourceTags": [{"Key": "team", "Value": "billing"}]
                }),
            ),
            (
                "CreateNotification",
                json!({
                    "AccountId": &account_id,
                    "BudgetName": case,
                    "Notification": {
                        "NotificationType": "FORECASTED",
                        "ComparisonOperator": "GREATER_THAN",
                        "Threshold": 90
                    },
                    "Subscribers": [subscriber.clone()]
                }),
            ),
            (
                "UpdateNotification",
                json!({
                    "AccountId": &account_id,
                    "BudgetName": case,
                    "OldNotification": notification.clone(),
                    "NewNotification": {
                        "NotificationType": "ACTUAL",
                        "ComparisonOperator": "GREATER_THAN",
                        "Threshold": 95
                    }
                }),
            ),
        ];
        for (operation, request) in &requests {
            let (status, body) = call(&router, operation, request).await;
            if expired {
                assert_eq!(
                    status, CLIENT_ERROR_STATUS,
                    "case {case} {operation}: expected a refusal once the end date has \
                     passed, got {status} {body}"
                );
                assert!(
                    is_error(&body, "NotFoundException"),
                    "case {case} {operation}: expected NotFoundException, got {body}"
                );
            } else {
                assert_eq!(
                    status, 200,
                    "case {case} {operation}: expected 200 while the end date is ahead, \
                     got {status} {body}"
                );
            }
        }

        // The account-wide notification listing reports the live budget only.
        let (status, body) = call(
            &router,
            "DescribeBudgetNotificationsForAccount",
            &json!({"AccountId": &account_id}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: expected DescribeBudgetNotificationsForAccount 200, \
             got {status} {body}"
        );
        let names: Vec<&str> = body["BudgetNotificationsForAccount"]
            .as_array()
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|entry| entry["BudgetName"].as_str())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            names.contains(&case),
            !expired,
            "case {case}: expected the account notification listing to report it only \
             while its end date is ahead, got {names:?}"
        );
    }
}

/// RULE: the deletion follows the CLOCK, not the call that created the budget — a budget
/// created with an end date a moment ahead is held and readable until that instant, and
/// gone once it has passed, with no further call needed to make it so.
#[tokio::test]
async fn the_deletion_falls_due_when_the_end_date_passes() {
    let router = router();
    let account_id = caller_account();
    let name = "ends-shortly";
    // A window several seconds wide, so the budget is still live for the reads below even
    // on a loaded host, and the wait afterwards is only as long as the clock still owes.
    let ends_at = now_secs() + 5;
    let named = json!({"AccountId": &account_id, "BudgetName": name});

    let (status, body) = call(
        &router,
        "CreateBudget",
        &json!({
            "AccountId": &account_id,
            "Budget": {
                "BudgetName": name,
                "TimeUnit": "CUSTOM",
                "BudgetType": "COST",
                "BudgetLimit": {"Amount": "1", "Unit": "USD"},
                "TimePeriod": {"Start": ends_at - DAY_SECS, "End": ends_at}
            }
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case before-the-end: expected CreateBudget to answer 200, got {status} {body}"
    );

    // Round trip: the created budget reads back while its end date is still ahead.
    let (status, body) = call(&router, "DescribeBudget", &named).await;
    assert_eq!(
        status, 200,
        "case before-the-end: expected DescribeBudget to answer 200, got {status} {body}"
    );
    assert_eq!(
        body["Budget"]["BudgetName"].as_str(),
        Some(name),
        "case before-the-end: expected the created budget back, got {body}"
    );
    let listed = listed_names(&router, &account_id).await;
    assert_eq!(
        listed,
        vec![name.to_string()],
        "case before-the-end: expected the budget to be listed, got {listed:?}"
    );

    // Wait out exactly what is left of the window — the deletion falls due at the end
    // date, with no further call needed to make it so.
    let remaining = u64::try_from((ends_at - now_secs()).max(0)).expect("a non-negative wait");
    tokio::time::sleep(Duration::from_secs(remaining) + Duration::from_millis(250)).await;

    let (status, body) = call(&router, "DescribeBudget", &named).await;
    assert_eq!(
        status, CLIENT_ERROR_STATUS,
        "case after-the-end: expected DescribeBudget to be refused once the end date has \
         passed, got {status} {body}"
    );
    assert!(
        is_error(&body, "NotFoundException"),
        "case after-the-end: expected NotFoundException, got {body}"
    );
    let listed = listed_names(&router, &account_id).await;
    assert!(
        listed.is_empty(),
        "case after-the-end: expected no budgets left for the account, got {listed:?}"
    );
}

/// RULE: a budget the account no longer holds does not count against the account's budget
/// quota either — the quota is checked against the budgets that are still live.
///
/// The quota is reached by seeding the shared store directly (the state struct is this
/// crate's own) rather than by driving twenty thousand requests, exactly as
/// `tests/all/budget_lifecycle.rs` reaches it.
#[tokio::test]
async fn an_expired_budget_no_longer_counts_against_the_accounts_quota() {
    let (router, engine) = router_with_engine();
    let account_id = caller_account();
    let ended = ::aws_smithy_types::DateTime::from_secs(civil_secs("2021-01-01T00:00:00Z"));
    let started = ::aws_smithy_types::DateTime::from_secs(civil_secs("2020-01-01T00:00:00Z"));

    {
        let state = engine.state::<ComAmazonawsBudgetsAwsbudgetservicegatewayState>();
        let mut store = state.lock();
        for index in 0..MAX_BUDGETS_PER_ACCOUNT {
            let budget_name = format!("ended-{index:06}");
            store.budgets.insert(
                (account_id.clone(), budget_name.clone()),
                StoredBudget {
                    budget: Budget {
                        budget_name,
                        time_unit: "MONTHLY".to_string(),
                        budget_type: "COST".to_string(),
                        time_period: Some(TimePeriod {
                            start: Some(started),
                            end: Some(ended),
                        }),
                        ..Budget::default()
                    },
                    notifications: Vec::new(),
                    actions: Vec::new(),
                    tags: Vec::new(),
                },
            );
        }
        assert_eq!(
            store.budgets.len(),
            MAX_BUDGETS_PER_ACCOUNT,
            "case seeded: the account must sit exactly at its quota before the sweep"
        );
    }

    let (status, body) = call(
        &router,
        "CreateBudget",
        &json!({
            "AccountId": &account_id,
            "Budget": {
                "BudgetName": "fresh",
                "TimeUnit": "MONTHLY",
                "BudgetType": "COST",
                "BudgetLimit": {"Amount": "1", "Unit": "USD"}
            }
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case quota-of-expired-budgets: expected CreateBudget to answer 200 once the \
         expired budgets are gone, got {status} {body}"
    );

    let listed = listed_names(&router, &account_id).await;
    assert_eq!(
        listed,
        vec!["fresh".to_string()],
        "case quota-of-expired-budgets: expected only the new budget to be held, \
         got {listed:?}"
    );
}
