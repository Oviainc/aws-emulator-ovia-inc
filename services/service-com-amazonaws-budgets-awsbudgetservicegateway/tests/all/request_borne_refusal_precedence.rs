// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for WHICH refusal a request that is malformed in itself gets, driven
//! over the wire through the same `MultiServiceRouter` the production router serves from
//! (the harness is the lifecycle suite's, so every suite drives the service identically).
//!
//! The RULE these tables assert: a refusal that the request's own members decide — a
//! `TimePeriod` whose `Start` does not come before its `End`, a role in another account
//! than the one the request names, a subscriber address that is not what its own
//! `SubscriptionType` names, a subscriber set past its per-type split — is answered with
//! the client error that rule models, whether or not the resource the request names
//! exists; only a request that carries no such violation is answered about the resource
//! (`NotFoundException`, or the 200 the read itself).
//!
//! Each table spans that class rather than one request: its rows differ along the
//! operation, which member carries the violation and how it fails, and which part of the
//! named identity is absent (the action, the budget, or neither).

use serde_json::{json, Value};

use service_com_amazonaws_budgets_awsbudgetservicegateway::state::MAX_SNS_SUBSCRIBERS_PER_NOTIFICATION;

use super::budget_action_execution::{seed_action, seed_budget};
use super::budget_lifecycle::{call, caller_account, is_error, router};
use super::subscriber_address_validation::subscriber;

/// The client error every rule a request carries in itself is refused with on these
/// operations: "an error on the client occurred. Typically, the cause is an invalid input
/// value" (`InvalidParameterException`, HTTP 400).
const REQUEST_REFUSAL: &str = "InvalidParameterException";

/// The error an operation answers about a resource that is not there: "we can't locate
/// the resource that you specified" (`NotFoundException`).
const ABSENT_REFUSAL: &str = "NotFoundException";

/// An `ActionId` that satisfies the modelled
/// `^[a-fA-F0-9]{8}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{12}$` and was
/// minted for nothing, so it reaches the handler and resolves to no action.
const UNMINTED_ACTION_ID: &str = "00000000-0000-4000-8000-000000000000";

/// A budget name no `CreateBudget` in this suite established, satisfying `BudgetName`'s
/// `@length(1, 100)` and its pattern.
const UNKNOWN_BUDGET: &str = "no-such-budget";

/// An instant the seeded windows are built around, in epoch seconds.
const BASE: i64 = 1_700_000_000;

/// A second, a day and a year in seconds — the spans the malformed windows are inverted
/// over, so the table does not depend on one gap size.
const SECOND: i64 = 1;
const DAY: i64 = 86_400;
const YEAR: i64 = 365 * DAY;

/// The windows that break the one rule every `TimePeriod` carries: "the start date must
/// come before the end date" (`TimePeriod`, from
/// `smithy_query(query="doc",shape="TimePeriod")`). The rule is STRICT, so a window whose
/// bounds are the same instant breaks it too, and the rows span the inversion by a
/// second, a day and a year as well as sub-second ordering.
fn malformed_windows() -> Vec<(&'static str, Value)> {
    vec![
        (
            "End one second before Start",
            json!({"Start": BASE, "End": BASE - SECOND}),
        ),
        (
            "End a day before Start",
            json!({"Start": BASE, "End": BASE - DAY}),
        ),
        (
            "End a year before Start",
            json!({"Start": BASE, "End": BASE - YEAR}),
        ),
        (
            "End equal to Start",
            json!({"Start": BASE, "End": BASE}),
        ),
        (
            "End a fraction of a second before Start",
            json!({"Start": 0.5 + BASE as f64, "End": 0.25 + BASE as f64}),
        ),
    ]
}

/// The windows that satisfy the rule — including the two that name one bound only, and
/// the one that names neither: "there are no restrictions on the end date", so a side the
/// caller left out restricts nothing and cannot be the wrong way round.
fn well_formed_windows() -> Vec<(&'static str, Value)> {
    vec![
        (
            "Start before End",
            json!({"Start": BASE - DAY, "End": BASE + DAY}),
        ),
        ("Start only", json!({"Start": BASE - YEAR})),
        ("End only", json!({"End": BASE + YEAR})),
        ("neither bound", json!({})),
    ]
}

/// The identities a history read can name: the action that exists, and the two ways the
/// named resource is absent. Answers `(case, budget_name, action_id)`.
fn identities(budget_name: &str, action_id: &str) -> Vec<(&'static str, String, String)> {
    vec![
        (
            "the action that exists",
            budget_name.to_string(),
            action_id.to_string(),
        ),
        (
            "an action that was never minted",
            budget_name.to_string(),
            UNMINTED_ACTION_ID.to_string(),
        ),
        (
            "a budget that does not exist",
            UNKNOWN_BUDGET.to_string(),
            action_id.to_string(),
        ),
        (
            "neither the budget nor the action",
            UNKNOWN_BUDGET.to_string(),
            UNMINTED_ACTION_ID.to_string(),
        ),
    ]
}

/// RULE: `DescribeBudgetActionHistories` refuses a window that breaks the `TimePeriod`
/// ordering rule with the modelled `InvalidParameterException`, whichever action it
/// names — so one and the same malformed window is never answered two different errors
/// depending on something unrelated to it.
#[tokio::test]
async fn a_malformed_window_is_refused_the_same_way_whichever_action_a_history_read_names() {
    let router = router();
    let account_id = caller_account();
    let budget_name = "refusal-precedence-history";
    seed_budget(&router, &account_id, budget_name).await;
    let action_id = seed_action(&router, &account_id, budget_name).await;

    for (window_case, window) in malformed_windows() {
        for (identity_case, named_budget, named_action) in identities(budget_name, &action_id) {
            let case = format!("{window_case} + {identity_case}");
            let (status, body) = call(
                &router,
                "DescribeBudgetActionHistories",
                &json!({
                    "AccountId": &account_id,
                    "BudgetName": &named_budget,
                    "ActionId": &named_action,
                    "TimePeriod": &window,
                }),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected the refusal to answer 400, got {status} {body}"
            );
            assert!(
                is_error(&body, REQUEST_REFUSAL),
                "case {case:?}: expected {REQUEST_REFUSAL}, got {body}"
            );
        }
    }
}

/// RULE, the other side: a window that holds together carries no violation of its own, so
/// the read is answered about the resource — the records when it is there, and
/// `NotFoundException` when it is not.
#[tokio::test]
async fn a_well_formed_window_is_answered_about_the_resource() {
    let router = router();
    let account_id = caller_account();
    let budget_name = "refusal-precedence-well-formed";
    seed_budget(&router, &account_id, budget_name).await;
    let action_id = seed_action(&router, &account_id, budget_name).await;

    for (window_case, window) in well_formed_windows() {
        for (identity_case, named_budget, named_action) in identities(budget_name, &action_id) {
            let case = format!("{window_case} + {identity_case}");
            let exists = named_budget == budget_name && named_action == action_id;
            let (status, body) = call(
                &router,
                "DescribeBudgetActionHistories",
                &json!({
                    "AccountId": &account_id,
                    "BudgetName": &named_budget,
                    "ActionId": &named_action,
                    "TimePeriod": &window,
                }),
            )
            .await;
            if exists {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the read to answer 200, got {status} {body}"
                );
                assert!(
                    body.get("ActionHistories").is_some_and(Value::is_array),
                    "case {case:?}: expected an ActionHistories list, got {body}"
                );
            } else {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected the refusal to answer 400, got {status} {body}"
                );
                assert!(
                    is_error(&body, ABSENT_REFUSAL),
                    "case {case:?}: expected {ABSENT_REFUSAL}, got {body}"
                );
            }
        }
    }
}

/// RULE, on the sibling history read that shares the `TimePeriod` rule: the same
/// malformed window is refused the same way there too, whichever budget it names — the
/// two reads of this service answer one rule with one error.
#[tokio::test]
async fn both_history_reads_refuse_a_malformed_window_before_resolving_the_resource() {
    let router = router();
    let account_id = caller_account();
    let budget_name = "refusal-precedence-sibling";
    seed_budget(&router, &account_id, budget_name).await;
    let action_id = seed_action(&router, &account_id, budget_name).await;

    for (window_case, window) in malformed_windows() {
        for named_budget in [budget_name, UNKNOWN_BUDGET] {
            for operation in [
                "DescribeBudgetActionHistories",
                "DescribeBudgetPerformanceHistory",
            ] {
                let case = format!("{window_case} + {named_budget} + {operation}");
                let mut request = json!({
                    "AccountId": &account_id,
                    "BudgetName": named_budget,
                    "TimePeriod": &window,
                });
                if operation == "DescribeBudgetActionHistories" {
                    request["ActionId"] = json!(&action_id);
                }
                let (status, body) = call(&router, operation, &request).await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected the refusal to answer 400, got {status} {body}"
                );
                assert!(
                    is_error(&body, REQUEST_REFUSAL),
                    "case {case:?}: expected {REQUEST_REFUSAL}, got {body}"
                );
            }
        }
    }
}

/// One row of the update table: the members a request carries that violate a rule of its
/// own, and how that violation fails.
fn malformed_update_patches() -> Vec<(&'static str, Value)> {
    // `MAX_SNS_SUBSCRIBERS_PER_NOTIFICATION` distinct SNS subscribers are the whole of a
    // set's SNS half, so one more than that is a set past the split — "one SNS subscriber
    // and up to 10 email subscribers" (`Notification`).
    let sns_past_the_split: Vec<Value> = (0..=MAX_SNS_SUBSCRIBERS_PER_NOTIFICATION)
        .map(|n| {
            subscriber(
                "SNS",
                &format!("arn:aws:sns:us-east-1:{}:budget-alerts-{n}", caller_account()),
            )
        })
        .collect();
    vec![
        (
            "ExecutionRoleArn naming a role in another account",
            json!({"ExecutionRoleArn": "arn:aws:iam::210987654321:role/budget-action-role"}),
        ),
        (
            "an EMAIL subscriber whose address is not an email address",
            json!({"Subscribers": [subscriber("EMAIL", "not-an-email")]}),
        ),
        (
            "an SNS subscriber whose address names no topic",
            json!({"Subscribers": [subscriber("SNS", "budget-alerts")]}),
        ),
        (
            "a subscriber set past its SNS split",
            json!({"Subscribers": sns_past_the_split}),
        ),
    ]
}

/// RULE, on the write path that carries three such rules: `UpdateBudgetAction` refuses a
/// request whose own members break a rule with the modelled `InvalidParameterException`,
/// whichever action it names — and a request that breaks none is answered about the
/// resource.
#[tokio::test]
async fn a_malformed_update_is_refused_the_same_way_whichever_action_it_names() {
    let router = router();
    let account_id = caller_account();
    let budget_name = "refusal-precedence-update";
    seed_budget(&router, &account_id, budget_name).await;
    let action_id = seed_action(&router, &account_id, budget_name).await;

    for (patch_case, patch) in malformed_update_patches() {
        for (identity_case, named_budget, named_action) in identities(budget_name, &action_id) {
            let case = format!("{patch_case} + {identity_case}");
            let mut request = json!({
                "AccountId": &account_id,
                "BudgetName": &named_budget,
                "ActionId": &named_action,
            });
            for (member, value) in patch.as_object().cloned().unwrap_or_default() {
                request[member] = value;
            }
            let (status, body) = call(&router, "UpdateBudgetAction", &request).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected the refusal to answer 400, got {status} {body}"
            );
            assert!(
                is_error(&body, REQUEST_REFUSAL),
                "case {case:?}: expected {REQUEST_REFUSAL}, got {body}"
            );
        }
    }

    // The control: the same operation, carrying a replacement that breaks no rule of its
    // own, is answered about the resource it names.
    for (identity_case, named_budget, named_action) in identities(budget_name, &action_id) {
        let case = format!("well-formed replacement + {identity_case}");
        let exists = named_budget == budget_name && named_action == action_id;
        let (status, body) = call(
            &router,
            "UpdateBudgetAction",
            &json!({
                "AccountId": &account_id,
                "BudgetName": &named_budget,
                "ActionId": &named_action,
                "ExecutionRoleArn": format!("arn:aws:iam::{account_id}:role/budget-action-other"),
            }),
        )
        .await;
        if exists {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the update to answer 200, got {status} {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected the refusal to answer 400, got {status} {body}"
            );
            assert!(
                is_error(&body, ABSENT_REFUSAL),
                "case {case:?}: expected {ABSENT_REFUSAL}, got {body}"
            );
        }
    }
}
