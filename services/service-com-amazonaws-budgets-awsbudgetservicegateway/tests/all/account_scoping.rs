// SPDX-License-Identifier: CC0-1.0

//! The account every budgets operation is scoped to.
//!
//! Every operation of this service takes an `AccountId` — "the account ID of the user.
//! It's a 12-digit number" — and the service answers for exactly ONE account, the one
//! its caller signs as (`engine::emulator_account_id`). So the RULE asserted here:
//!
//! > a request whose `AccountId` is not the caller's own account is refused with the
//! > modelled `AccessDeniedException` before it reads or writes anything, and
//! > the same request naming the caller's own account is served.
//!
//! The tables span the class the rule generalises over: EVERY implemented operation of
//! the service that takes an `AccountId` (reads, writes and the action state machine
//! alike) × a set of foreign account ids that each satisfy `AccountId`'s modelled
//! `@length(12, 12)` and `^\d{12}$`, so the refusal comes from the handler rather than
//! from request validation. The operations still scaffolded answer 501 and are covered
//! by `client_error_envelopes.rs`.
//!
//! The tag plane (`TagResource`, `UntagResource`, `ListTagsForResource`) carries no
//! `AccountId` member: it names the account inside its `ResourceARN`, whose account
//! field is the account half of the identity the store is keyed by. The same rule
//! therefore holds for it, over its own table below, and its account field is not a
//! lookup — an ARN of another account is refused before the store is consulted, not
//! answered "no such resource".

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

use service_com_amazonaws_budgets_awsbudgetservicegateway::state::ComAmazonawsBudgetsAwsbudgetservicegatewayState;

use super::budget_action_execution::{email_subscriber, pending_create_body, simple_create_body};
use super::budget_lifecycle::{account, caller_account, call, is_error, router_with_engine};
use super::resource_tags::{action_arn, budget_arn, reported, tag_list};

/// The status `AccessDeniedException` is answered with: HTTP 400, the status this service
/// answers EVERY refusal of a request at whatever 4xx the error shape's `@httpError`
/// declares (`client_error_envelopes.rs`), the authorization refusal included.
const REFUSAL_STATUS: u16 = 400;

/// The budget every row addresses, and the notification and action created on it.
const BUDGET: &str = "scoped";

/// An `ActionId` that is well-formed per the modelled `@pattern` and was minted for
/// nothing — used only where a row needs an id to BUILD the request table, never to
/// locate an action.
const UNMINTED_ACTION_ID: &str = "00000000-0000-4000-8000-000000000000";

/// Account ids that are not the caller's, each satisfying `AccountId`'s modelled
/// `@length(12, 12)` + `^\d{12}$` so the request reaches the handler.
///
/// The rows differ along how they sit beside the caller's own id: all nines, the
/// documentation's example account, an id differing from the caller's in its last digit
/// only, one differing in its first digit only, and one of all ones.
fn foreign_accounts() -> Vec<String> {
    let own = caller_account();
    let rows = vec![
        account(999_999_999_999),
        account(123_456_789_012),
        with_another_digit(&own, own.len() - 1),
        with_another_digit(&own, 0),
        account(111_111_111_111),
    ];
    for row in &rows {
        assert_eq!(
            row.len(),
            12,
            "case {row:?}: a foreign account id must still be 12 digits"
        );
        assert!(
            row.chars().all(|c| c.is_ascii_digit()),
            "case {row:?}: a foreign account id must still be all digits"
        );
        assert_ne!(
            *row, own,
            "case {row:?}: a foreign account id must not be the caller's own"
        );
    }
    rows
}

/// `digits` with the digit at `position` replaced by one that DIFFERS from it, so the
/// row it builds cannot collapse onto the caller's own id whatever that id happens to
/// be.
fn with_another_digit(digits: &str, position: usize) -> String {
    digits
        .chars()
        .enumerate()
        .map(|(index, digit)| match (index == position, digit) {
            (false, digit) => digit,
            (true, '0') => '1',
            (true, _) => '0',
        })
        .collect()
}

/// The notification the seeded budget alerts on.
fn notification() -> Value {
    json!({
        "NotificationType": "ACTUAL",
        "ComparisonOperator": "GREATER_THAN",
        "Threshold": 80.0,
        "ThresholdType": "PERCENTAGE"
    })
}

/// A second notification, for the rows that CREATE one.
fn other_notification() -> Value {
    json!({
        "NotificationType": "FORECASTED",
        "ComparisonOperator": "LESS_THAN",
        "Threshold": 20.0,
        "ThresholdType": "PERCENTAGE"
    })
}

/// A budget that satisfies every modelled constraint and documented cross-member rule.
fn budget(name: &str) -> Value {
    json!({
        "BudgetName": name,
        "TimeUnit": "MONTHLY",
        "BudgetType": "COST",
        "BudgetLimit": {"Amount": "100", "Unit": "USD"}
    })
}

/// A service over a fresh store, seeded for the CALLER's account with the budget every
/// row addresses: one notification with one subscriber, and one budget action.
///
/// Answers the router, the engine behind it (so a test can read the store itself) and
/// the `ActionId` the seeded action was minted with.
async fn seeded() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>, String) {
    let (router, engine) = router_with_engine();
    let own = caller_account();
    let (status, body) = call(
        &router,
        "CreateBudget",
        &json!({
            "AccountId": &own,
            "Budget": budget(BUDGET),
            "NotificationsWithSubscribers": [{
                "Notification": notification(),
                "Subscribers": [email_subscriber(0)]
            }]
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: expected CreateBudget to answer 200, got {status} {body}"
    );
    // The action is created WAITING for its approval, which is the status
    // `ExecuteBudgetAction`'s `APPROVE_BUDGET_ACTION` is accepted from, so the row that
    // executes it under the caller's own account has work to do.
    let (status, body) = call(
        &router,
        "CreateBudgetAction",
        &pending_create_body(&own, BUDGET),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: expected CreateBudgetAction to answer 200, got {status} {body}"
    );
    let action_id = body["ActionId"]
        .as_str()
        .expect("the seeded action carries an ActionId")
        .to_string();
    (router, engine, action_id)
}

/// Every IMPLEMENTED operation of this service, each with a request that satisfies every
/// modelled constraint on its input (so it reaches the handler) and that would do real
/// work under the caller's own account: the reads address the seeded budget, the writes
/// create, update or delete something.
fn every_operation(account_id: &str, action_id: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "CreateBudget",
            json!({"AccountId": account_id, "Budget": budget("created-by-the-row")}),
        ),
        (
            "DescribeBudget",
            json!({"AccountId": account_id, "BudgetName": BUDGET}),
        ),
        ("DescribeBudgets", json!({"AccountId": account_id})),
        (
            "UpdateBudget",
            json!({"AccountId": account_id, "NewBudget": budget(BUDGET)}),
        ),
        (
            "DeleteBudget",
            json!({"AccountId": account_id, "BudgetName": BUDGET}),
        ),
        (
            "CreateNotification",
            json!({
                "AccountId": account_id,
                "BudgetName": BUDGET,
                "Notification": other_notification(),
                "Subscribers": [email_subscriber(1)]
            }),
        ),
        (
            "DeleteNotification",
            json!({
                "AccountId": account_id,
                "BudgetName": BUDGET,
                "Notification": notification()
            }),
        ),
        (
            "UpdateNotification",
            json!({
                "AccountId": account_id,
                "BudgetName": BUDGET,
                "OldNotification": notification(),
                "NewNotification": other_notification()
            }),
        ),
        (
            "DescribeNotificationsForBudget",
            json!({"AccountId": account_id, "BudgetName": BUDGET}),
        ),
        (
            "DescribeBudgetNotificationsForAccount",
            json!({"AccountId": account_id}),
        ),
        (
            "CreateSubscriber",
            json!({
                "AccountId": account_id,
                "BudgetName": BUDGET,
                "Notification": notification(),
                "Subscriber": email_subscriber(2)
            }),
        ),
        (
            "DescribeSubscribersForNotification",
            json!({
                "AccountId": account_id,
                "BudgetName": BUDGET,
                "Notification": notification()
            }),
        ),
        (
            "UpdateSubscriber",
            json!({
                "AccountId": account_id,
                "BudgetName": BUDGET,
                "Notification": notification(),
                "OldSubscriber": email_subscriber(0),
                "NewSubscriber": email_subscriber(3)
            }),
        ),
        ("CreateBudgetAction", simple_create_body(account_id, BUDGET)),
        (
            "DescribeBudgetAction",
            json!({
                "AccountId": account_id,
                "BudgetName": BUDGET,
                "ActionId": action_id
            }),
        ),
        (
            "DescribeBudgetActionsForBudget",
            json!({"AccountId": account_id, "BudgetName": BUDGET}),
        ),
        (
            "DescribeBudgetActionHistories",
            json!({
                "AccountId": account_id,
                "BudgetName": BUDGET,
                "ActionId": action_id
            }),
        ),
        (
            "UpdateBudgetAction",
            json!({
                "AccountId": account_id,
                "BudgetName": BUDGET,
                "ActionId": action_id,
                "ApprovalModel": "MANUAL"
            }),
        ),
        (
            "ExecuteBudgetAction",
            json!({
                "AccountId": account_id,
                "BudgetName": BUDGET,
                "ActionId": action_id,
                "ExecutionType": "APPROVE_BUDGET_ACTION"
            }),
        ),
        (
            "DeleteBudgetAction",
            json!({
                "AccountId": account_id,
                "BudgetName": BUDGET,
                "ActionId": action_id
            }),
        ),
    ]
}

/// Every budget key the store holds, as `(AccountId, BudgetName)` — read from the state
/// struct directly, so what a refused request did to the store is asserted independently
/// of any read operation.
fn stored_keys(engine: &Arc<engine::InMemoryEngine>) -> Vec<(String, String)> {
    let state = engine.state::<ComAmazonawsBudgetsAwsbudgetservicegatewayState>();
    let store = state.lock();
    let mut keys: Vec<(String, String)> = store.budgets.keys().cloned().collect();
    keys.sort();
    keys
}

/// RULE: a request whose `AccountId` is not the caller's own is refused with
/// `AccessDeniedException` at [`REFUSAL_STATUS`] — on every operation, for every foreign
/// account id — and it neither reports nor changes anything.
#[tokio::test]
async fn every_operation_refuses_an_account_that_is_not_the_callers() {
    let (router, engine, action_id) = seeded().await;
    let own = caller_account();
    let before = stored_keys(&engine);
    assert_eq!(
        before,
        vec![(own.clone(), BUDGET.to_string())],
        "setup: expected the store to hold only the caller's seeded budget"
    );

    for foreign in foreign_accounts() {
        for (operation, request) in every_operation(&foreign, &action_id) {
            let case = format!("{operation} as {foreign}");
            let (status, body) = call(&router, operation, &request).await;
            assert_eq!(
                status, REFUSAL_STATUS,
                "case {case:?}: expected the refusal at {REFUSAL_STATUS}, got {status} {body}"
            );
            assert!(
                is_error(&body, "AccessDeniedException"),
                "case {case:?}: expected AccessDeniedException, got {body}"
            );
            let message = body
                .get("Message")
                .or_else(|| body.get("message"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            assert!(
                !message.is_empty(),
                "case {case:?}: expected a message a caller can read, got {body}"
            );
            // Nothing of the caller's is reported to a foreign account, whatever the
            // operation would have returned.
            for reported in ["Budgets", "Notifications", "Subscribers", "Actions", "Action"] {
                assert!(
                    body.get(reported).is_none(),
                    "case {case:?}: expected no {reported} in a refusal, got {body}"
                );
            }

            // And the refusal wrote nothing: no budget of the foreign account came into
            // being, and the caller's own is untouched.
            let after = stored_keys(&engine);
            assert_eq!(
                after, before,
                "case {case:?}: expected the refusal to leave the store at {before:?}, \
                 got {after:?}"
            );
        }
    }

    // The caller's budget still reads back whole after every one of those refusals.
    let (status, body) = call(
        &router,
        "DescribeBudget",
        &json!({"AccountId": &own, "BudgetName": BUDGET}),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the caller's own budget to still read back, got {status} {body}"
    );
    assert_eq!(
        body["Budget"]["BudgetName"], BUDGET,
        "expected the caller's own budget to be unchanged, got {body}"
    );
}

/// RULE, the other side: the gate refuses ONLY a foreign account. Every one of those
/// same requests, sent for the caller's own account against a store seeded the same way,
/// is served — so none of them is refused with `AccessDeniedException` and none answers
/// [`REFUSAL_STATUS`].
///
/// Each row gets a store of its own, because the operations are destructive of each
/// other (a delete removes what the next row's read addresses). The table is rebuilt per
/// row, since the `ActionId` it addresses belongs to that row's own store, and the row's
/// POSITION is what selects it.
#[tokio::test]
async fn the_same_request_for_the_callers_own_account_is_served() {
    let own = caller_account();
    let row_count = every_operation(&own, UNMINTED_ACTION_ID).len();
    for row in 0..row_count {
        let (router, _engine, action_id) = seeded().await;
        let (operation, request) = every_operation(&own, &action_id).remove(row);
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            status, 200,
            "case {operation:?}: expected the caller's own account to be served, \
             got {status} {body}"
        );
        assert!(
            !is_error(&body, "AccessDeniedException"),
            "case {operation:?}: expected no AccessDeniedException for the caller's own \
             account, got {body}"
        );
    }
}

/// The tag the ARN-addressed rows below write, read and delete.
const TAG_KEY: &str = "team";

/// Every ARN-ADDRESSED operation of this service: the tag plane, which names the account
/// it acts in inside its `ResourceARN` rather than as an `AccountId` member. Each request
/// satisfies every modelled constraint on its input, and each would do real work under
/// the caller's own account.
fn every_arn_addressed_operation(resource_arn: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "TagResource",
            json!({
                "ResourceARN": resource_arn,
                "ResourceTags": tag_list(&[(TAG_KEY.to_string(), "cost".to_string())])
            }),
        ),
        (
            "UntagResource",
            json!({"ResourceARN": resource_arn, "ResourceTagKeys": [TAG_KEY]}),
        ),
        (
            "ListTagsForResource",
            json!({"ResourceARN": resource_arn}),
        ),
    ]
}

/// The `ResourceARN`s an ARN-addressed row is run against, under `account_id`: both kinds
/// of taggable resource, each named once as a resource the caller's own account really
/// holds (`held` true) and once as one nobody created — so the answer cannot depend on
/// whether the store happens to hold the resource under the caller's account.
fn arn_rows(account_id: &str, action_id: &str) -> Vec<(&'static str, String, bool)> {
    vec![
        ("the budget", budget_arn(account_id, BUDGET), true),
        (
            "a budget nobody created",
            budget_arn(account_id, "never-created"),
            false,
        ),
        (
            "the budget's action",
            action_arn(account_id, BUDGET, action_id),
            true,
        ),
        (
            "an action nobody created",
            action_arn(account_id, BUDGET, UNMINTED_ACTION_ID),
            false,
        ),
    ]
}

/// RULE, over the tag plane: the account a request names is refused the same way whether
/// it arrives as an `AccountId` member or as the account field of a `ResourceARN`. An ARN
/// of an account that is not the caller's is answered `AccessDeniedException` at
/// [`REFUSAL_STATUS`] — not the `NotFoundException` a lookup under that account would
/// produce — and it
/// neither reports nor changes anything.
#[tokio::test]
async fn the_tag_plane_refuses_an_arn_whose_account_is_not_the_callers() {
    let (router, engine, action_id) = seeded().await;
    let own = caller_account();
    let before = stored_keys(&engine);

    // The caller's own resources carry a tag, so a refusal that wrote or deleted
    // something on the key it named would show.
    let held = [(TAG_KEY.to_string(), "platform".to_string())];
    for (case, arn) in [
        ("the budget", budget_arn(&own, BUDGET)),
        (
            "the budget's action",
            action_arn(&own, BUDGET, &action_id),
        ),
    ] {
        let (status, body) = call(
            &router,
            "TagResource",
            &json!({"ResourceARN": &arn, "ResourceTags": tag_list(&held)}),
        )
        .await;
        assert_eq!(
            status, 200,
            "setup: expected {case} to be tagged, got {status} {body}"
        );
    }

    for foreign in foreign_accounts() {
        for (resource, arn, _held_by_the_caller) in arn_rows(&foreign, &action_id) {
            for (operation, request) in every_arn_addressed_operation(&arn) {
                let case = format!("{operation} on {resource} of {foreign}");
                let (status, body) = call(&router, operation, &request).await;
                assert_eq!(
                    status, REFUSAL_STATUS,
                    "case {case:?}: expected the refusal at {REFUSAL_STATUS}, got {status} {body}"
                );
                assert!(
                    is_error(&body, "AccessDeniedException"),
                    "case {case:?}: expected AccessDeniedException, got {body}"
                );
                assert!(
                    !is_error(&body, "NotFoundException"),
                    "case {case:?}: expected the account gate rather than a lookup, got {body}"
                );
                let message = body
                    .get("Message")
                    .or_else(|| body.get("message"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                assert!(
                    !message.is_empty(),
                    "case {case:?}: expected a message a caller can read, got {body}"
                );
                assert!(
                    body.get("ResourceTags").is_none(),
                    "case {case:?}: expected no ResourceTags in a refusal, got {body}"
                );

                let after = stored_keys(&engine);
                assert_eq!(
                    after, before,
                    "case {case:?}: expected the refusal to leave the store at {before:?}, \
                     got {after:?}"
                );
            }
        }
    }

    // Not one of those refusals touched the tags of the caller's own resources.
    for (case, arn) in [
        ("the budget", budget_arn(&own, BUDGET)),
        (
            "the budget's action",
            action_arn(&own, BUDGET, &action_id),
        ),
    ] {
        let (status, body) = call(
            &router,
            "ListTagsForResource",
            &json!({"ResourceARN": &arn}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the caller's own resource to stay readable, \
             got {status} {body}"
        );
        assert_eq!(
            reported(&body),
            held.to_vec(),
            "case {case:?}: expected the refusals to leave this resource's tags alone"
        );
    }
}

/// RULE, the other side over the tag plane: the gate refuses ONLY a foreign account. The
/// same requests, with the ARN carrying the caller's OWN account and naming a resource
/// the store holds, are served.
#[tokio::test]
async fn the_tag_plane_serves_an_arn_of_the_callers_own_account() {
    let (router, _engine, action_id) = seeded().await;
    let own = caller_account();

    for (resource, arn, held_by_the_caller) in arn_rows(&own, &action_id) {
        // Only the rows naming a resource that EXISTS can be served; the rest are the
        // not-found rows the tag suites own.
        if !held_by_the_caller {
            continue;
        }
        for (operation, request) in every_arn_addressed_operation(&arn) {
            let case = format!("{operation} on {resource}");
            let (status, body) = call(&router, operation, &request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the caller's own account to be served, \
                 got {status} {body}"
            );
            assert!(
                !is_error(&body, "AccessDeniedException"),
                "case {case:?}: expected no AccessDeniedException for the caller's own \
                 account, got {body}"
            );
        }
    }
}
