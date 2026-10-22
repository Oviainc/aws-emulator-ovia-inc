// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the Budget lifecycle: `CreateBudget`, `DescribeBudget`,
//! `DescribeBudgets`, `UpdateBudget`, `DescribeNotificationsForBudget` and
//! `DeleteBudget`, driven over the wire (`POST /` with
//! `X-Amz-Target: AWSBudgetServiceGateway.<Operation>`, awsJson1_1) through the same
//! `MultiServiceRouter` the production router serves from.
//!
//! Every test states a RULE and asserts it over a set of inputs; the request the
//! issue recorded is one row of the first table.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// The `TimeUnit` / `BudgetType` catalogs are not re-typed here: these are the
/// model-derived constants the scaffold generated into `src/ops/types/shared_types.rs`,
/// so a table built from them cannot drift from the model.
use service_com_amazonaws_budgets_awsbudgetservicegateway::ops::types::{
    Budget, BUDGET_TYPE_VALUES, TIME_UNIT_VALUES,
};
use service_com_amazonaws_budgets_awsbudgetservicegateway::state::{
    ComAmazonawsBudgetsAwsbudgetservicegatewayState, StoredBudget, MAX_BUDGETS_PER_ACCOUNT,
};

/// A router carrying only this service, over one shared engine — what
/// `core/router/src/lib.rs` builds, narrowed to the service under test. Request
/// authentication is off so the suite needs no signing keys; routing still goes
/// through the SigV4 credential scope on the `authorization` header below.
///
/// `pub(crate)` so the sibling suites in this test target drive the same harness
/// rather than copying it.
pub(crate) fn router() -> MultiServiceRouter {
    router_with_engine().0
}

/// As [`router`], plus the shared engine it was built from, so a test can seed the
/// store directly instead of driving tens of thousands of requests to reach a quota.
fn router_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false).register_service(
        service_com_amazonaws_budgets_awsbudgetservicegateway::EmulatedService::new(engine.clone()),
    );
    (router, engine)
}

/// Call one operation and return `(status, body)`. The body is `Value::Null` when the
/// response carried no bytes.
pub(crate) async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AWSBudgetServiceGateway.{operation}"),
        )
        // The routing key every front door reads: the 4th field of the SigV4
        // credential scope is this service's signing name, `budgets`.
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 \
             Credential=AKIDEXAMPLE/20260101/us-east-1/budgets/aws4_request, \
             SignedHeaders=host;x-amz-target, Signature=0000",
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Whether an error body names `exception`. The awsJson `__type` is either the bare
/// shape name or the namespaced shape id, so the suffix is what is matched.
pub(crate) fn is_error(body: &Value, exception: &str) -> bool {
    body.get("__type")
        .and_then(Value::as_str)
        .is_some_and(|found| found.ends_with(exception))
}

/// Structural JSON equality that compares numbers by value, so a timestamp sent as
/// `1704067200` still matches one rendered as `1704067200.0`.
fn same(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(a), Value::Number(b)) => a.as_f64() == b.as_f64(),
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| same(x, y))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len() && a.iter().all(|(k, v)| b.get(k).is_some_and(|w| same(v, w)))
        }
        _ => left == right,
    }
}

/// An account id: 12 digits, as `AccountId`'s `^\d{12}$` requires.
pub(crate) fn account(n: u64) -> String {
    format!("{n:012}")
}

/// One row of the lifecycle table: a case name and the `Budget` / notification
/// payloads to create it with.
struct Row {
    case: &'static str,
    budget: Value,
    notifications: Value,
    /// The `Budget` that `UpdateBudget` replaces it with.
    updated: Value,
}

/// The lifecycle table. Rows differ along what the rule generalises over: the time
/// unit, the budget type, whether the limit is a single `BudgetLimit` or a
/// `PlannedBudgetLimits` map, the presence of `TimePeriod` / `CostTypes` /
/// `CostFilters` / `FilterExpression` / `Metrics`, and the number of notifications
/// (zero, one, several).
fn lifecycle_rows() -> Vec<Row> {
    vec![
        Row {
            // The case the issue recorded.
            case: "monthly cost budget with one notification",
            budget: json!({
                "BudgetName": "monthly-cost",
                "BudgetLimit": {"Amount": "100", "Unit": "USD"},
                "TimeUnit": "MONTHLY",
                "BudgetType": "COST"
            }),
            notifications: json!([{
                "Notification": {
                    "NotificationType": "ACTUAL",
                    "ComparisonOperator": "GREATER_THAN",
                    "Threshold": 80.0,
                    "ThresholdType": "PERCENTAGE"
                },
                "Subscribers": [{"SubscriptionType": "EMAIL", "Address": "ops@example.com"}]
            }]),
            updated: json!({
                "BudgetName": "monthly-cost",
                "BudgetLimit": {"Amount": "250", "Unit": "USD"},
                "TimeUnit": "MONTHLY",
                "BudgetType": "COST"
            }),
        },
        Row {
            case: "daily usage budget, no notifications, with a time period",
            budget: json!({
                "BudgetName": "daily usage/hours",
                "BudgetLimit": {"Amount": "3.5", "Unit": "GB"},
                "TimeUnit": "DAILY",
                "BudgetType": "USAGE",
                "TimePeriod": {"Start": 1_704_067_200, "End": 1_735_689_600},
                "CostTypes": {"IncludeTax": false, "UseAmortized": true}
            }),
            notifications: json!([]),
            updated: json!({
                "BudgetName": "daily usage/hours",
                "BudgetLimit": {"Amount": "7", "Unit": "GB"},
                "TimeUnit": "DAILY",
                "BudgetType": "USAGE",
                "TimePeriod": {"Start": 1_704_067_200, "End": 1_767_225_600}
            }),
        },
        Row {
            case: "quarterly planned limits with three notifications",
            budget: json!({
                "BudgetName": "planned-quarterly",
                "PlannedBudgetLimits": {
                    "1704067200": {"Amount": "10", "Unit": "USD"},
                    "1711929600": {"Amount": "20", "Unit": "USD"}
                },
                "TimeUnit": "QUARTERLY",
                "BudgetType": "COST",
                "CostFilters": {"Service": ["Amazon EC2", "Amazon Redshift"]}
            }),
            notifications: json!([
                {
                    "Notification": {
                        "NotificationType": "FORECASTED",
                        "ComparisonOperator": "LESS_THAN",
                        "Threshold": 1.0,
                        "ThresholdType": "ABSOLUTE_VALUE"
                    },
                    "Subscribers": [
                        {"SubscriptionType": "SNS", "Address": "arn:aws:sns::000000000000:budget"}
                    ]
                },
                {
                    "Notification": {
                        "NotificationType": "ACTUAL",
                        "ComparisonOperator": "EQUAL_TO",
                        "Threshold": 15_000_000_000_000.0,
                        "NotificationState": "ALARM"
                    },
                    "Subscribers": [
                        {"SubscriptionType": "EMAIL", "Address": "a@example.com"},
                        {"SubscriptionType": "EMAIL", "Address": "b@example.com"}
                    ]
                },
                {
                    "Notification": {
                        "NotificationType": "ACTUAL",
                        "ComparisonOperator": "GREATER_THAN",
                        "Threshold": 0.0
                    },
                    "Subscribers": [{"SubscriptionType": "EMAIL", "Address": "c@example.com"}]
                }
            ]),
            updated: json!({
                "BudgetName": "planned-quarterly",
                "PlannedBudgetLimits": {"1704067200": {"Amount": "11", "Unit": "USD"}},
                "TimeUnit": "QUARTERLY",
                "BudgetType": "COST"
            }),
        },
        Row {
            case: "annual savings-plans budget filtered by expression and metric",
            budget: json!({
                "BudgetName": "sp-coverage",
                "BudgetLimit": {"Amount": "100", "Unit": "PERCENT"},
                "TimeUnit": "ANNUALLY",
                "BudgetType": "SAVINGS_PLANS_COVERAGE",
                "FilterExpression": {
                    "Dimensions": {
                        "Key": "REGION",
                        "Values": ["us-east-1"],
                        "MatchOptions": ["EQUALS"]
                    }
                },
                "Metrics": ["UnblendedCost"]
            }),
            notifications: json!([{
                "Notification": {
                    "NotificationType": "ACTUAL",
                    "ComparisonOperator": "LESS_THAN",
                    "Threshold": 99.5,
                    "ThresholdType": "PERCENTAGE"
                },
                "Subscribers": [{"SubscriptionType": "EMAIL", "Address": "sp@example.com"}]
            }]),
            updated: json!({
                "BudgetName": "sp-coverage",
                "BudgetLimit": {"Amount": "90", "Unit": "PERCENT"},
                "TimeUnit": "ANNUALLY",
                "BudgetType": "SAVINGS_PLANS_COVERAGE",
                "FilterExpression": {"Not": {"Tags": {"Key": "team", "Values": ["platform"]}}},
                "Metrics": ["AmortizedCost"]
            }),
        },
        Row {
            case: "custom time unit, RI utilization",
            budget: json!({
                "BudgetName": "ri-util",
                "BudgetLimit": {"Amount": "100", "Unit": "PERCENT"},
                "TimeUnit": "CUSTOM",
                "BudgetType": "RI_UTILIZATION",
                "TimePeriod": {"Start": 1_704_067_200}
            }),
            notifications: json!([{
                "Notification": {
                    "NotificationType": "ACTUAL",
                    "ComparisonOperator": "LESS_THAN",
                    "Threshold": 80.0
                },
                "Subscribers": [{"SubscriptionType": "EMAIL", "Address": "ri@example.com"}]
            }]),
            updated: json!({
                "BudgetName": "ri-util",
                "BudgetLimit": {"Amount": "100", "Unit": "PERCENT"},
                "TimeUnit": "CUSTOM",
                "BudgetType": "RI_COVERAGE"
            }),
        },
    ]
}

/// Assert that `read` carries every member of `sent`, plus a `LastUpdatedTime` the
/// service stamped. `FilterExpression` is excluded: it is only returned when the
/// caller sets `ShowFilterExpression`, which its own test covers.
fn assert_read_back(case: &str, sent: &Value, read: &Value) {
    for (member, expected) in sent.as_object().expect("the sent budget is an object") {
        if member == "FilterExpression" {
            continue;
        }
        let actual = read.get(member).unwrap_or(&Value::Null);
        assert!(
            same(expected, actual),
            "case {case:?}: expected {member} = {expected}, got {actual}"
        );
    }
    let stamped = read.get("LastUpdatedTime").and_then(Value::as_f64);
    assert!(
        stamped.is_some_and(|seen| seen > 0.0),
        "case {case:?}: expected a LastUpdatedTime the service stamped, got {stamped:?}"
    );
    assert!(
        sent.get("LastUpdatedTime").is_none(),
        "case {case:?}: the table must not send LastUpdatedTime \
         (it is the service's value, not the caller's)"
    );
}

/// RULE: a budget created for an account is persisted under its name and read back
/// unchanged by `DescribeBudget` and `DescribeBudgets`; `UpdateBudget` replaces it in
/// place; `DeleteBudget` removes it and everything stored with it, after which both
/// the delete and the read answer `NotFoundException`.
#[tokio::test]
async fn a_budget_round_trips_through_its_whole_lifecycle() {
    let router = router();
    for (index, row) in lifecycle_rows().into_iter().enumerate() {
        let case = row.case;
        // One account per row, so rows cannot see each other's budgets.
        let account_id = account(100_000_000 + index as u64);
        let name = row.budget["BudgetName"]
            .as_str()
            .expect("every row names its budget")
            .to_string();

        let create = json!({
            "AccountId": &account_id,
            "Budget": row.budget.clone(),
            "NotificationsWithSubscribers": row.notifications.clone(),
            "ResourceTags": [{"Key": "owner", "Value": "platform"}]
        });
        let (status, body) = call(&router, "CreateBudget", &create).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateBudget to answer 200, got {status} {body}"
        );

        // A second create for the same (AccountId, BudgetName) is a duplicate. The
        // status is 400, not the `@httpError` 409 the shape declares — see
        // `client_error_envelopes.rs` for the rule.
        let (status, body) = call(&router, "CreateBudget", &create).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a duplicate CreateBudget to answer 400, got {status} {body}"
        );
        assert!(
            is_error(&body, "DuplicateRecordException"),
            "case {case:?}: expected DuplicateRecordException, got {body}"
        );

        // DescribeBudget reads back what was created.
        let describe = json!({"AccountId": &account_id, "BudgetName": &name});
        let (status, body) = call(&router, "DescribeBudget", &describe).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeBudget to answer 200, got {status} {body}"
        );
        assert_read_back(case, &row.budget, &body["Budget"]);

        // DescribeBudgets lists exactly that one budget for the account.
        let list = json!({"AccountId": &account_id});
        let (status, body) = call(&router, "DescribeBudgets", &list).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeBudgets to answer 200, got {status} {body}"
        );
        let listed = body["Budgets"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            listed.len(),
            1,
            "case {case:?}: expected exactly the one stored budget, got {body}"
        );
        assert_read_back(case, &row.budget, &listed[0]);
        assert!(
            body.get("NextToken").is_none_or(Value::is_null),
            "case {case:?}: expected no NextToken when nothing remains, got {body}"
        );

        // The notifications created with the budget are the ones reported for it.
        let (status, body) = call(&router, "DescribeNotificationsForBudget", &describe).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeNotificationsForBudget 200, got {status} {body}"
        );
        let expected: Vec<Value> = row
            .notifications
            .as_array()
            .expect("every row lists its notifications")
            .iter()
            .map(|supplied| supplied["Notification"].clone())
            .collect();
        let actual = body["Notifications"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            actual.len(),
            expected.len(),
            "case {case:?}: expected {} notifications, got {body}",
            expected.len()
        );
        for (position, want) in expected.iter().enumerate() {
            let got = &actual[position];
            for (member, value) in want.as_object().expect("a notification object") {
                let seen = got.get(member).unwrap_or(&Value::Null);
                assert!(
                    same(value, seen),
                    "case {case:?}: notification {position} expected {member} = {value}, \
                     got {seen}"
                );
            }
        }

        // UpdateBudget replaces the stored budget in place.
        let (status, body) = call(
            &router,
            "UpdateBudget",
            &json!({"AccountId": &account_id, "NewBudget": row.updated.clone()}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateBudget to answer 200, got {status} {body}"
        );
        let (status, body) = call(&router, "DescribeBudget", &describe).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeBudget after update 200, got {status} {body}"
        );
        assert_read_back(case, &row.updated, &body["Budget"]);
        // A member the update dropped is gone, not merged from the previous version.
        for member in [
            "BudgetLimit",
            "PlannedBudgetLimits",
            "TimePeriod",
            "CostFilters",
        ] {
            if row.updated.get(member).is_none() {
                let leftover = body["Budget"].get(member);
                assert!(
                    leftover.is_none_or(Value::is_null),
                    "case {case:?}: expected UpdateBudget to replace the budget, \
                     but {member} survived as {leftover:?}"
                );
            }
        }
        // The notifications are the budget's own resources; the update left them alone.
        let (_, after) = call(&router, "DescribeNotificationsForBudget", &describe).await;
        assert_eq!(
            after["Notifications"].as_array().map(Vec::len),
            Some(expected.len()),
            "case {case:?}: expected UpdateBudget to leave the notifications alone, got {after}"
        );

        // DeleteBudget removes it; the read and a second delete both answer
        // `NotFoundException` at HTTP 400 (the status this service sends for it).
        let (status, body) = call(&router, "DeleteBudget", &describe).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DeleteBudget to answer 200, got {status} {body}"
        );
        for operation in [
            "DescribeBudget",
            "DeleteBudget",
            "DescribeNotificationsForBudget",
        ] {
            let (status, body) = call(&router, operation, &describe).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected {operation} after delete to answer 400, \
                 got {status} {body}"
            );
            assert!(
                is_error(&body, "NotFoundException"),
                "case {case:?}: expected NotFoundException from {operation}, got {body}"
            );
        }
        let (status, body) = call(&router, "DescribeBudgets", &list).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeBudgets after delete 200, got {status} {body}"
        );
        assert_eq!(
            body["Budgets"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: expected no budgets left for the account, got {body}"
        );
    }
}

/// RULE: every value the model enumerates for a budget's `TimeUnit` and `BudgetType`
/// is accepted and read back as sent — the catalogs come from the generated
/// `TIME_UNIT_VALUES` / `BUDGET_TYPE_VALUES` constants, so this covers entries the
/// issue never cited.
#[tokio::test]
async fn every_modelled_time_unit_and_budget_type_round_trips() {
    let router = router();
    let account_id = account(222_222_222);
    for (index, time_unit) in TIME_UNIT_VALUES.iter().enumerate() {
        for budget_type in BUDGET_TYPE_VALUES {
            let case = format!("{time_unit}/{budget_type}");
            let name = format!("b-{index}-{budget_type}");
            let budget = json!({
                "BudgetName": &name,
                "BudgetLimit": {"Amount": "1", "Unit": "USD"},
                "TimeUnit": time_unit,
                "BudgetType": budget_type
            });
            let (status, body) = call(
                &router,
                "CreateBudget",
                &json!({"AccountId": &account_id, "Budget": budget.clone()}),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected CreateBudget to answer 200, got {status} {body}"
            );
            let (status, body) = call(
                &router,
                "DescribeBudget",
                &json!({"AccountId": &account_id, "BudgetName": &name}),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected DescribeBudget to answer 200, got {status} {body}"
            );
            assert_read_back(&case, &budget, &body["Budget"]);
        }
    }
    // Every budget created above is in the account's list, exactly once.
    let (_, body) = call(
        &router,
        "DescribeBudgets",
        &json!({"AccountId": &account_id}),
    )
    .await;
    let expected = TIME_UNIT_VALUES.len() * BUDGET_TYPE_VALUES.len();
    assert_eq!(
        body["Budgets"].as_array().map(Vec::len),
        Some(expected),
        "expected {expected} budgets in the account, got {body}"
    );
}

/// Walk every page of `DescribeBudgets` for `account_id` with the given page size and
/// return the budget names in the order they were served.
async fn walk_budget_pages(
    router: &MultiServiceRouter,
    account_id: &str,
    max_results: Option<i32>,
    total: usize,
) -> Vec<String> {
    let mut names = Vec::new();
    let mut next: Option<String> = None;
    // One more request than there can be pages, so a token that never clears is a
    // failed assertion rather than a hang.
    for _ in 0..=total + 1 {
        let mut request = json!({"AccountId": account_id});
        if let Some(max) = max_results {
            request["MaxResults"] = json!(max);
        }
        if let Some(token) = &next {
            request["NextToken"] = json!(token);
        }
        let (status, body) = call(router, "DescribeBudgets", &request).await;
        assert_eq!(
            status, 200,
            "case total={total} max={max_results:?}: expected 200, got {status} {body}"
        );
        let page = body["Budgets"].as_array().cloned().unwrap_or_default();
        if let Some(max) = max_results {
            assert!(
                i32::try_from(page.len()).is_ok_and(|held| held <= max),
                "case total={total} max={max_results:?}: a page held {} items, got {body}",
                page.len()
            );
        }
        names.extend(
            page.iter()
                .map(|budget| budget["BudgetName"].as_str().unwrap_or_default().to_string()),
        );
        next = body
            .get("NextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        if next.is_none() {
            return names;
        }
        assert!(
            !page.is_empty(),
            "case total={total} max={max_results:?}: a NextToken was returned after an \
             empty page, got {body}"
        );
    }
    panic!("case total={total} max={max_results:?}: pagination never finished");
}

/// RULE: a paginated Describe serves the account's budgets ordered by `BudgetName`,
/// split into pages of at most `MaxResults`, each page continuing exactly where the
/// previous `NextToken` left off, and returns a token only while items remain — so
/// walking the pages yields every budget exactly once, whatever the page size.
#[tokio::test]
async fn pagination_partitions_the_account_in_budget_name_order() {
    // `MaxResults` on DescribeBudgets is modelled @range(min: 1, max: 1000): both
    // published bounds are exercised here, and the values past them are refused in
    // `a_request_outside_a_modelled_constraint_is_refused`.
    let page_sizes = [None, Some(1), Some(2), Some(3), Some(7), Some(1000)];
    for total in [0_usize, 1, 2, 5, 13] {
        let router = router();
        let account_id = account(300_000_000 + total as u64);
        // Created in an order unrelated to the order they must be served in.
        let mut expected: Vec<String> = (0..total)
            .map(|i| format!("budget-{:02}", (i * 7) % 100))
            .collect();
        for name in &expected {
            let (status, body) = call(
                &router,
                "CreateBudget",
                &json!({
                    "AccountId": &account_id,
                    "Budget": {
                        "BudgetName": name,
                        "TimeUnit": "MONTHLY",
                        "BudgetType": "COST",
                        "BudgetLimit": {"Amount": "1", "Unit": "USD"}
                    }
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case total={total}: expected CreateBudget {name} to answer 200, \
                 got {status} {body}"
            );
        }
        expected.sort();

        for max_results in page_sizes {
            let served = walk_budget_pages(&router, &account_id, max_results, total).await;
            assert_eq!(
                served, expected,
                "case total={total} max={max_results:?}: expected the account's budgets \
                 in BudgetName order exactly once, got {served:?}"
            );
        }
    }
}

/// RULE: the notifications of one budget paginate by the same rule, bounded by that
/// operation's own modelled `MaxResults` @range(min: 1, max: 100).
#[tokio::test]
async fn notifications_paginate_by_max_results() {
    let router = router();
    let account_id = account(444_444_444);
    // `NotificationWithSubscribersList` is @length(max: 10), so ten is the largest set
    // a single CreateBudget can establish.
    let notifications: Vec<Value> = (0..10)
        .map(|i| {
            json!({
                "Notification": {
                    "NotificationType": if i % 2 == 0 { "ACTUAL" } else { "FORECASTED" },
                    "ComparisonOperator": "GREATER_THAN",
                    "Threshold": f64::from(i) + 0.5,
                    "ThresholdType": "PERCENTAGE"
                },
                "Subscribers": [
                    {"SubscriptionType": "EMAIL", "Address": format!("n{i}@example.com")}
                ]
            })
        })
        .collect();
    let (status, body) = call(
        &router,
        "CreateBudget",
        &json!({
            "AccountId": &account_id,
            "Budget": {
                "BudgetName": "with-notifications",
                "TimeUnit": "MONTHLY",
                "BudgetType": "COST",
                "BudgetLimit": {"Amount": "1", "Unit": "USD"}
            },
            "NotificationsWithSubscribers": notifications
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected CreateBudget with ten notifications to answer 200, got {status} {body}"
    );

    let expected: Vec<f64> = (0..10).map(|i| f64::from(i) + 0.5).collect();
    for max_results in [None, Some(1), Some(3), Some(10), Some(100)] {
        let mut thresholds: Vec<f64> = Vec::new();
        let mut next: Option<String> = None;
        loop {
            let mut request =
                json!({"AccountId": &account_id, "BudgetName": "with-notifications"});
            if let Some(max) = max_results {
                request["MaxResults"] = json!(max);
            }
            if let Some(token) = &next {
                request["NextToken"] = json!(token);
            }
            let (status, body) = call(&router, "DescribeNotificationsForBudget", &request).await;
            assert_eq!(
                status, 200,
                "case max={max_results:?}: expected 200, got {status} {body}"
            );
            let page = body["Notifications"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            if let Some(max) = max_results {
                assert!(
                    i32::try_from(page.len()).is_ok_and(|held| held <= max),
                    "case max={max_results:?}: a page held {} notifications, got {body}",
                    page.len()
                );
            }
            thresholds.extend(page.iter().filter_map(|n| n["Threshold"].as_f64()));
            next = body
                .get("NextToken")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            if next.is_none() {
                break;
            }
        }
        assert_eq!(
            thresholds, expected,
            "case max={max_results:?}: expected every notification once, in creation \
             order, got {thresholds:?}"
        );
    }
}

/// RULE: a `NextToken` this service did not mint does not decode, and a paginated
/// Describe refuses it with `InvalidNextTokenException` (HTTP 400) instead of
/// silently serving the first page.
#[tokio::test]
async fn an_undecodable_next_token_is_refused() {
    let router = router();
    let account_id = account(555_555_555);
    let (status, body) = call(
        &router,
        "CreateBudget",
        &json!({
            "AccountId": &account_id,
            "Budget": {
                "BudgetName": "tokens",
                "TimeUnit": "MONTHLY",
                "BudgetType": "COST",
                "BudgetLimit": {"Amount": "1", "Unit": "USD"}
            },
            "NotificationsWithSubscribers": [{
                "Notification": {
                    "NotificationType": "ACTUAL",
                    "ComparisonOperator": "GREATER_THAN",
                    "Threshold": 1.0
                },
                "Subscribers": [{"SubscriptionType": "EMAIL", "Address": "t@example.com"}]
            }]
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the fixture budget to be created, got {status} {body}"
    );

    // One budget and MaxResults=1 leaves nothing to continue, so no token is due.
    let (_, body) = call(
        &router,
        "DescribeBudgets",
        &json!({"AccountId": &account_id, "MaxResults": 1}),
    )
    .await;
    assert!(
        body.get("NextToken").is_none_or(Value::is_null),
        "expected no NextToken when the page exhausted the account, got {body}"
    );

    // Tokens differing along form: empty, not hex, odd-length hex, hex of a payload
    // this service does not mint, a real token with a nibble removed, and a decodable
    // token whose offset is past the end of the list (stale or tampered with).
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
        for (operation, request) in [
            (
                "DescribeBudgets",
                json!({"AccountId": &account_id, "NextToken": token}),
            ),
            (
                "DescribeNotificationsForBudget",
                json!({
                    "AccountId": &account_id,
                    "BudgetName": "tokens",
                    "NextToken": token
                }),
            ),
        ] {
            let (status, body) = call(&router, operation, &request).await;
            assert_eq!(
                status, 400,
                "case {operation} token={token:?}: expected 400, got {status} {body}"
            );
            assert!(
                is_error(&body, "InvalidNextTokenException"),
                "case {operation} token={token:?}: expected InvalidNextTokenException, \
                 got {body}"
            );
        }
    }
}

/// RULE: a budget is scoped to its account, so a name stored for one account is not
/// visible to another — every read and write keyed on it answers `NotFoundException`
/// (HTTP 400, the status this service sends for it).
#[tokio::test]
async fn budgets_are_scoped_to_their_account() {
    let router = router();
    let owner = account(666_666_666);
    let other = account(777_777_777);
    let budget = json!({
        "BudgetName": "shared-name",
        "TimeUnit": "MONTHLY",
        "BudgetType": "COST",
        "BudgetLimit": {"Amount": "1", "Unit": "USD"}
    });
    let (status, body) = call(
        &router,
        "CreateBudget",
        &json!({"AccountId": &owner, "Budget": budget.clone()}),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the owner's budget to be created, got {status} {body}"
    );

    for (operation, request) in [
        (
            "DescribeBudget",
            json!({"AccountId": &other, "BudgetName": "shared-name"}),
        ),
        (
            "DeleteBudget",
            json!({"AccountId": &other, "BudgetName": "shared-name"}),
        ),
        (
            "DescribeNotificationsForBudget",
            json!({"AccountId": &other, "BudgetName": "shared-name"}),
        ),
        (
            "UpdateBudget",
            json!({"AccountId": &other, "NewBudget": budget.clone()}),
        ),
        (
            "DescribeBudget",
            json!({"AccountId": &owner, "BudgetName": "never-created"}),
        ),
    ] {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            status, 400,
            "case {operation} {request}: expected 400, got {status} {body}"
        );
        assert!(
            is_error(&body, "NotFoundException"),
            "case {operation} {request}: expected NotFoundException, got {body}"
        );
    }

    // The other account creating the same name is not a duplicate: it is its own
    // budget, and the owner's is untouched.
    let (status, body) = call(
        &router,
        "CreateBudget",
        &json!({
            "AccountId": &other,
            "Budget": {
                "BudgetName": "shared-name",
                "TimeUnit": "DAILY",
                "BudgetType": "USAGE",
                "BudgetLimit": {"Amount": "2", "Unit": "GB"}
            }
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the same name in another account to be accepted, got {status} {body}"
    );
    let (_, body) = call(
        &router,
        "DescribeBudget",
        &json!({"AccountId": &owner, "BudgetName": "shared-name"}),
    )
    .await;
    assert_eq!(
        body["Budget"]["TimeUnit"], "MONTHLY",
        "expected the owner's budget to be untouched, got {body}"
    );
}

/// RULE: a request outside a modelled constraint is refused before any state changes,
/// at request validation — HTTP 400 `ValidationException`, whose message names the
/// member path and the constraint it violated. `client_error_envelopes.rs` asserts the
/// message; this table asserts the status and the code over the whole constraint
/// surface.
#[tokio::test]
async fn a_request_outside_a_modelled_constraint_is_refused() {
    let router = router();
    let valid = json!({
        "BudgetName": "ok",
        "TimeUnit": "MONTHLY",
        "BudgetType": "COST",
        "BudgetLimit": {"Amount": "1", "Unit": "USD"}
    });
    let subscriber = json!([{"SubscriptionType": "EMAIL", "Address": "a@example.com"}]);
    let empty = account(1);
    let cases: Vec<(&str, &str, Value)> = vec![
        // AccountId is @length(12, 12) + @pattern(^\d{12}$).
        (
            "account id too short",
            "CreateBudget",
            json!({"AccountId": "12345", "Budget": valid.clone()}),
        ),
        (
            "account id not digits",
            "CreateBudget",
            json!({"AccountId": "abcdefghijkl", "Budget": valid.clone()}),
        ),
        (
            "account id too long",
            "CreateBudget",
            json!({"AccountId": "1234567890123", "Budget": valid.clone()}),
        ),
        (
            "account id absent",
            "CreateBudget",
            json!({"Budget": valid.clone()}),
        ),
        // Budget is @required, and so are its BudgetName / TimeUnit / BudgetType.
        ("budget absent", "CreateBudget", json!({"AccountId": &empty})),
        (
            "budget name absent",
            "CreateBudget",
            json!({
                "AccountId": &empty,
                "Budget": {"TimeUnit": "MONTHLY", "BudgetType": "COST"}
            }),
        ),
        (
            "time unit absent",
            "CreateBudget",
            json!({
                "AccountId": &empty,
                "Budget": {"BudgetName": "x", "BudgetType": "COST"}
            }),
        ),
        // BudgetName is @length(1, 100) and forbids ':', '\' and "/action/".
        (
            "budget name empty",
            "CreateBudget",
            json!({
                "AccountId": &empty,
                "Budget": {"BudgetName": "", "TimeUnit": "MONTHLY", "BudgetType": "COST"}
            }),
        ),
        (
            "budget name over 100 characters",
            "CreateBudget",
            json!({
                "AccountId": &empty,
                "Budget": {
                    "BudgetName": "n".repeat(101),
                    "TimeUnit": "MONTHLY",
                    "BudgetType": "COST"
                }
            }),
        ),
        (
            "budget name with a colon",
            "CreateBudget",
            json!({
                "AccountId": &empty,
                "Budget": {"BudgetName": "a:b", "TimeUnit": "MONTHLY", "BudgetType": "COST"}
            }),
        ),
        (
            "budget name containing /action/",
            "CreateBudget",
            json!({
                "AccountId": &empty,
                "Budget": {
                    "BudgetName": "a/action/b",
                    "TimeUnit": "MONTHLY",
                    "BudgetType": "COST"
                }
            }),
        ),
        // Enum members accept only the values the model lists.
        (
            "time unit outside the enum",
            "CreateBudget",
            json!({
                "AccountId": &empty,
                "Budget": {"BudgetName": "x", "TimeUnit": "WEEKLY", "BudgetType": "COST"}
            }),
        ),
        (
            "budget type outside the enum",
            "CreateBudget",
            json!({
                "AccountId": &empty,
                "Budget": {"BudgetName": "x", "TimeUnit": "MONTHLY", "BudgetType": "SPEND"}
            }),
        ),
        // Spend$Amount is @pattern(^([0-9]*\.)?[0-9]+$).
        (
            "limit amount not numeric",
            "CreateBudget",
            json!({
                "AccountId": &empty,
                "Budget": {
                    "BudgetName": "x",
                    "TimeUnit": "MONTHLY",
                    "BudgetType": "COST",
                    "BudgetLimit": {"Amount": "ten", "Unit": "USD"}
                }
            }),
        ),
        // Notification$Threshold is @range(min: 0, max: 15000000000000).
        (
            "threshold below the published minimum",
            "CreateBudget",
            json!({
                "AccountId": &empty,
                "Budget": valid.clone(),
                "NotificationsWithSubscribers": [{
                    "Notification": {
                        "NotificationType": "ACTUAL",
                        "ComparisonOperator": "GREATER_THAN",
                        "Threshold": -1.0
                    },
                    "Subscribers": subscriber.clone()
                }]
            }),
        ),
        (
            "threshold above the published maximum",
            "CreateBudget",
            json!({
                "AccountId": &empty,
                "Budget": valid.clone(),
                "NotificationsWithSubscribers": [{
                    "Notification": {
                        "NotificationType": "ACTUAL",
                        "ComparisonOperator": "GREATER_THAN",
                        "Threshold": 15_000_000_000_001.0
                    },
                    "Subscribers": subscriber.clone()
                }]
            }),
        ),
        // A notification must carry at least one subscriber (@length(min: 1)).
        (
            "notification with no subscribers",
            "CreateBudget",
            json!({
                "AccountId": &empty,
                "Budget": valid.clone(),
                "NotificationsWithSubscribers": [{
                    "Notification": {
                        "NotificationType": "ACTUAL",
                        "ComparisonOperator": "GREATER_THAN",
                        "Threshold": 1.0
                    },
                    "Subscribers": []
                }]
            }),
        ),
        (
            "budget name with a backslash",
            "CreateBudget",
            json!({
                "AccountId": &empty,
                "Budget": {"BudgetName": "a\\b", "TimeUnit": "MONTHLY", "BudgetType": "COST"}
            }),
        ),
        (
            "budget name carrying a script tag",
            "CreateBudget",
            json!({
                "AccountId": &empty,
                "Budget": {
                    "BudgetName": "x<SCRIPT>alert(1)</script>y",
                    "TimeUnit": "MONTHLY",
                    "BudgetType": "COST"
                }
            }),
        ),
        (
            "budget name with a colon on UpdateBudget",
            "UpdateBudget",
            json!({
                "AccountId": &empty,
                "NewBudget": {"BudgetName": "a:b", "TimeUnit": "MONTHLY", "BudgetType": "COST"}
            }),
        ),
        // The same name rule on the operations that take `BudgetName` directly: it is
        // refused as a constraint violation, not reported as a budget that is missing.
        (
            "budget name with a colon on DescribeBudget",
            "DescribeBudget",
            json!({"AccountId": &empty, "BudgetName": "a:b"}),
        ),
        (
            "budget name with a colon on DeleteBudget",
            "DeleteBudget",
            json!({"AccountId": &empty, "BudgetName": "a:b"}),
        ),
        (
            "budget name containing /action/ on DescribeNotificationsForBudget",
            "DescribeNotificationsForBudget",
            json!({"AccountId": &empty, "BudgetName": "a/action/b"}),
        ),
        // DescribeBudgets' MaxResults is @range(min: 1, max: 1000): one past each bound.
        (
            "max results below the published minimum",
            "DescribeBudgets",
            json!({"AccountId": &empty, "MaxResults": 0}),
        ),
        (
            "max results above the published maximum",
            "DescribeBudgets",
            json!({"AccountId": &empty, "MaxResults": 1001}),
        ),
        // DescribeNotificationsForBudget's MaxResults is @range(min: 1, max: 100).
        (
            "notification max results above the published maximum",
            "DescribeNotificationsForBudget",
            json!({"AccountId": &empty, "BudgetName": "ok", "MaxResults": 101}),
        ),
    ];

    for (case, operation, request) in cases {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected {operation} to answer 400, got {status} {body}"
        );
        assert!(
            is_error(&body, "ValidationException"),
            "case {case:?}: expected ValidationException, got {body}"
        );
    }

    // Nothing was stored by any refused request.
    let (_, body) = call(&router, "DescribeBudgets", &json!({"AccountId": &empty})).await;
    assert_eq!(
        body["Budgets"].as_array().map(Vec::len),
        Some(0),
        "expected a refused request to store nothing, got {body}"
    );
    // The published bounds themselves are accepted.
    for max_results in [1, 1000] {
        let (status, body) = call(
            &router,
            "DescribeBudgets",
            &json!({"AccountId": &empty, "MaxResults": max_results}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case MaxResults={max_results}: a published bound must be accepted, \
             got {status} {body}"
        );
    }
}

/// RULE: the cross-member combinations `CreateBudget` / `UpdateBudget` document as
/// mutually exclusive are refused with `InvalidParameterException` (HTTP 400), and
/// each half on its own is accepted.
#[tokio::test]
async fn mutually_exclusive_budget_members_are_refused() {
    let router = router();
    let account_id = account(888_888_888);
    let limit = json!({"Amount": "1", "Unit": "USD"});
    let refused: Vec<(&str, Value)> = vec![
        (
            // "You can't use BudgetLimit with PlannedBudgetLimits."
            "BudgetLimit together with PlannedBudgetLimits",
            json!({
                "BudgetName": "exclusive-1",
                "TimeUnit": "MONTHLY",
                "BudgetType": "COST",
                "BudgetLimit": limit.clone(),
                "PlannedBudgetLimits": {"1704067200": limit.clone()}
            }),
        ),
        (
            // "Either FilterExpression and Metrics or CostFilters and CostTypes, not
            // both or a different combination."
            "FilterExpression together with CostFilters",
            json!({
                "BudgetName": "exclusive-2",
                "TimeUnit": "MONTHLY",
                "BudgetType": "COST",
                "BudgetLimit": limit.clone(),
                "CostFilters": {"Service": ["Amazon EC2"]},
                "FilterExpression": {"Dimensions": {"Key": "REGION", "Values": ["us-east-1"]}}
            }),
        ),
        (
            "Metrics together with CostTypes",
            json!({
                "BudgetName": "exclusive-3",
                "TimeUnit": "MONTHLY",
                "BudgetType": "COST",
                "BudgetLimit": limit.clone(),
                "CostTypes": {"IncludeTax": true},
                "Metrics": ["UnblendedCost"]
            }),
        ),
        (
            // "The start date must come before the end date."
            "TimePeriod whose start is after its end",
            json!({
                "BudgetName": "exclusive-4",
                "TimeUnit": "CUSTOM",
                "BudgetType": "COST",
                "BudgetLimit": limit.clone(),
                "TimePeriod": {"Start": 1_735_689_600, "End": 1_704_067_200}
            }),
        ),
        (
            "TimePeriod whose start equals its end",
            json!({
                "BudgetName": "exclusive-5",
                "TimeUnit": "CUSTOM",
                "BudgetType": "COST",
                "BudgetLimit": limit.clone(),
                "TimePeriod": {"Start": 1_704_067_200, "End": 1_704_067_200}
            }),
        ),
    ];

    for (case, budget) in &refused {
        for (operation, request) in [
            (
                "CreateBudget",
                json!({"AccountId": &account_id, "Budget": budget.clone()}),
            ),
            (
                "UpdateBudget",
                json!({"AccountId": &account_id, "NewBudget": budget.clone()}),
            ),
        ] {
            let (status, body) = call(&router, operation, &request).await;
            assert_eq!(
                status, 400,
                "case {case:?} via {operation}: expected 400, got {status} {body}"
            );
            assert!(
                is_error(&body, "InvalidParameterException"),
                "case {case:?} via {operation}: expected InvalidParameterException, got {body}"
            );
        }
    }

    // Each half on its own is a valid budget.
    let accepted: Vec<(&str, Value)> = vec![
        (
            "BudgetLimit alone",
            json!({
                "BudgetName": "ok-1",
                "TimeUnit": "MONTHLY",
                "BudgetType": "COST",
                "BudgetLimit": limit.clone()
            }),
        ),
        (
            "PlannedBudgetLimits alone",
            json!({
                "BudgetName": "ok-2",
                "TimeUnit": "MONTHLY",
                "BudgetType": "COST",
                "PlannedBudgetLimits": {"1704067200": limit.clone()}
            }),
        ),
        (
            "CostFilters and CostTypes alone",
            json!({
                "BudgetName": "ok-3",
                "TimeUnit": "MONTHLY",
                "BudgetType": "COST",
                "BudgetLimit": limit.clone(),
                "CostFilters": {"Service": ["Amazon EC2"]},
                "CostTypes": {"IncludeTax": true}
            }),
        ),
        (
            "FilterExpression and Metrics alone",
            json!({
                "BudgetName": "ok-4",
                "TimeUnit": "MONTHLY",
                "BudgetType": "COST",
                "BudgetLimit": limit.clone(),
                "FilterExpression": {"Dimensions": {"Key": "SERVICE", "Values": ["AmazonEC2"]}},
                "Metrics": ["UsageQuantity"]
            }),
        ),
        (
            "TimePeriod whose start comes before its end",
            json!({
                "BudgetName": "ok-5",
                "TimeUnit": "CUSTOM",
                "BudgetType": "COST",
                "BudgetLimit": limit.clone(),
                "TimePeriod": {"Start": 1_704_067_200, "End": 1_735_689_600}
            }),
        ),
    ];
    for (case, budget) in &accepted {
        let (status, body) = call(
            &router,
            "CreateBudget",
            &json!({"AccountId": &account_id, "Budget": budget.clone()}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateBudget to answer 200, got {status} {body}"
        );
    }
    // And none of the refused ones left a budget behind.
    let (_, body) = call(
        &router,
        "DescribeBudgets",
        &json!({"AccountId": &account_id}),
    )
    .await;
    assert_eq!(
        body["Budgets"].as_array().map(Vec::len),
        Some(accepted.len()),
        "expected only the accepted budgets to be stored, got {body}"
    );
}

/// RULE: `LastUpdatedTime` is the service's own value ("The last time that you updated
/// this budget"), so a value the caller sends for it is replaced, not stored — on both
/// the create and the update path.
#[tokio::test]
async fn a_caller_supplied_last_updated_time_is_replaced() {
    let router = router();
    let account_id = account(131_313_131);
    // Values differing along form: the epoch, a past date, and a date in the far future.
    for (case, sent) in [
        ("epoch", 0.0_f64),
        ("a past date", 1_704_067_200.0),
        ("a far-future date", 4_102_444_800.0),
    ] {
        let name = format!("stamped-{case}");
        let (status, body) = call(
            &router,
            "CreateBudget",
            &json!({
                "AccountId": &account_id,
                "Budget": {
                    "BudgetName": &name,
                    "TimeUnit": "MONTHLY",
                    "BudgetType": "COST",
                    "BudgetLimit": {"Amount": "1", "Unit": "USD"},
                    "LastUpdatedTime": sent
                }
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateBudget to answer 200, got {status} {body}"
        );
        let (_, body) = call(
            &router,
            "DescribeBudget",
            &json!({"AccountId": &account_id, "BudgetName": &name}),
        )
        .await;
        let stamped = body["Budget"]["LastUpdatedTime"].as_f64();
        assert!(
            stamped.is_some_and(|seen| (seen - sent).abs() > f64::EPSILON && seen > 0.0),
            "case {case:?}: expected the service's own LastUpdatedTime, not the \
             caller's {sent}, got {stamped:?}"
        );

        let (status, body) = call(
            &router,
            "UpdateBudget",
            &json!({
                "AccountId": &account_id,
                "NewBudget": {
                    "BudgetName": &name,
                    "TimeUnit": "MONTHLY",
                    "BudgetType": "COST",
                    "BudgetLimit": {"Amount": "2", "Unit": "USD"},
                    "LastUpdatedTime": sent
                }
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateBudget to answer 200, got {status} {body}"
        );
        let (_, body) = call(
            &router,
            "DescribeBudget",
            &json!({"AccountId": &account_id, "BudgetName": &name}),
        )
        .await;
        let stamped = body["Budget"]["LastUpdatedTime"].as_f64();
        assert!(
            stamped.is_some_and(|seen| (seen - sent).abs() > f64::EPSILON && seen > 0.0),
            "case {case:?}: expected UpdateBudget to restamp LastUpdatedTime rather than \
             store the caller's {sent}, got {stamped:?}"
        );
    }
}

/// RULE: the store is bounded — an account cannot hold more budgets than its service
/// quota, and the request is refused with the modelled
/// `ServiceQuotaExceededException` (HTTP 402) rather than growing the process.
///
/// The quota is reached by seeding the shared store directly (the state struct is this
/// crate's own) rather than by driving 20,000 requests.
#[tokio::test]
async fn an_account_cannot_exceed_its_budget_quota() {
    let (router, engine) = router_with_engine();
    let full = account(141_414_141);
    let other = account(151_515_151);

    {
        let state = engine.state::<ComAmazonawsBudgetsAwsbudgetservicegatewayState>();
        let mut store = state.lock();
        for index in 0..MAX_BUDGETS_PER_ACCOUNT {
            let budget_name = format!("seeded-{index:06}");
            let budget = Budget {
                budget_name: budget_name.clone(),
                time_unit: "MONTHLY".to_string(),
                budget_type: "COST".to_string(),
                ..Budget::default()
            };
            store.budgets.insert(
                (full.clone(), budget_name),
                StoredBudget {
                    budget,
                    notifications: Vec::new(),
                    tags: Vec::new(),
                },
            );
        }
        assert_eq!(
            store.budgets.len(),
            MAX_BUDGETS_PER_ACCOUNT,
            "the seeded account must sit exactly at its quota"
        );
    }

    let new_budget = json!({
        "BudgetName": "one-too-many",
        "TimeUnit": "MONTHLY",
        "BudgetType": "COST",
        "BudgetLimit": {"Amount": "1", "Unit": "USD"}
    });
    let (status, body) = call(
        &router,
        "CreateBudget",
        &json!({"AccountId": &full, "Budget": new_budget.clone()}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case at-quota: expected CreateBudget to answer 400, got {status} {body}"
    );
    assert!(
        is_error(&body, "ServiceQuotaExceededException"),
        "case at-quota: expected ServiceQuotaExceededException, got {body}"
    );
    let (_, body) = call(
        &router,
        "DescribeBudget",
        &json!({"AccountId": &full, "BudgetName": "one-too-many"}),
    )
    .await;
    assert!(
        is_error(&body, "NotFoundException"),
        "case at-quota: the refused budget must not have been stored, got {body}"
    );

    // The quota is per account: another account is unaffected.
    let (status, body) = call(
        &router,
        "CreateBudget",
        &json!({"AccountId": &other, "Budget": new_budget}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case other-account: the quota is per account, so this must be accepted, \
         got {status} {body}"
    );
}

/// RULE: a budget's `FilterExpression` is returned only when the caller asks for it
/// with `ShowFilterExpression`; every other member is returned either way.
#[tokio::test]
async fn the_filter_expression_is_returned_only_when_asked_for() {
    let router = router();
    let account_id = account(999_999_999);
    let expression = json!({
        "And": [
            {
                "Dimensions": {
                    "Key": "REGION",
                    "Values": ["us-east-1", "eu-west-1"],
                    "MatchOptions": ["EQUALS"]
                }
            },
            {"Not": {"CostCategories": {"Key": "team", "Values": ["platform"]}}}
        ]
    });
    let (status, body) = call(
        &router,
        "CreateBudget",
        &json!({
            "AccountId": &account_id,
            "Budget": {
                "BudgetName": "filtered",
                "TimeUnit": "MONTHLY",
                "BudgetType": "COST",
                "BudgetLimit": {"Amount": "5", "Unit": "USD"},
                "FilterExpression": expression.clone(),
                "Metrics": ["NetUnblendedCost"]
            }
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected CreateBudget with a FilterExpression to answer 200, got {status} {body}"
    );

    for show in [None, Some(false), Some(true)] {
        let expected_present = show == Some(true);
        for operation in ["DescribeBudget", "DescribeBudgets"] {
            let mut request = json!({"AccountId": &account_id});
            if operation == "DescribeBudget" {
                request["BudgetName"] = json!("filtered");
            }
            if let Some(flag) = show {
                request["ShowFilterExpression"] = json!(flag);
            }
            let (status, body) = call(&router, operation, &request).await;
            assert_eq!(
                status, 200,
                "case {operation} show={show:?}: expected 200, got {status} {body}"
            );
            let read = if operation == "DescribeBudget" {
                body["Budget"].clone()
            } else {
                body["Budgets"][0].clone()
            };
            let seen = read.get("FilterExpression").cloned();
            if expected_present {
                let seen = seen.unwrap_or(Value::Null);
                assert!(
                    same(&expression, &seen),
                    "case {operation} show={show:?}: expected the stored FilterExpression \
                     {expression}, got {seen}"
                );
            } else {
                assert!(
                    seen.as_ref().is_none_or(Value::is_null),
                    "case {operation} show={show:?}: expected no FilterExpression, got {seen:?}"
                );
            }
            // The rest of the budget is reported either way.
            assert_eq!(
                read["Metrics"],
                json!(["NetUnblendedCost"]),
                "case {operation} show={show:?}: expected Metrics to be reported, got {read}"
            );
        }
    }
}

/// RULE: a reporting operation changes nothing a later caller sees — repeating any of
/// the reads leaves the store exactly as it was.
#[tokio::test]
async fn reads_do_not_change_what_a_later_caller_sees() {
    let router = router();
    let account_id = account(121_212_121);
    let (status, body) = call(
        &router,
        "CreateBudget",
        &json!({
            "AccountId": &account_id,
            "Budget": {
                "BudgetName": "stable",
                "TimeUnit": "MONTHLY",
                "BudgetType": "COST",
                "BudgetLimit": {"Amount": "1", "Unit": "USD"}
            },
            "NotificationsWithSubscribers": [{
                "Notification": {
                    "NotificationType": "ACTUAL",
                    "ComparisonOperator": "GREATER_THAN",
                    "Threshold": 50.0,
                    "ThresholdType": "PERCENTAGE"
                },
                "Subscribers": [{"SubscriptionType": "EMAIL", "Address": "r@example.com"}]
            }]
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the fixture budget to be created, got {status} {body}"
    );

    let reads: Vec<(&str, Value)> = vec![
        (
            "DescribeBudget",
            json!({"AccountId": &account_id, "BudgetName": "stable"}),
        ),
        ("DescribeBudgets", json!({"AccountId": &account_id})),
        (
            "DescribeNotificationsForBudget",
            json!({"AccountId": &account_id, "BudgetName": "stable"}),
        ),
    ];
    for (operation, request) in &reads {
        let (first_status, first) = call(&router, operation, request).await;
        // Other reads in between, so a read that mutated anything would show up.
        for (other, other_request) in &reads {
            let (status, body) = call(&router, other, other_request).await;
            assert_eq!(
                status, 200,
                "case {operation}/{other}: expected the interleaved read to answer 200, \
                 got {status} {body}"
            );
        }
        let (second_status, second) = call(&router, operation, request).await;
        assert_eq!(
            (first_status, &first),
            (second_status, &second),
            "case {operation}: a read changed what the next caller sees"
        );
    }
}
