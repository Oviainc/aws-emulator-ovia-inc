// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the `ResourceTags` a CREATE may establish on a resource —
//! `CreateBudget`'s and `CreateBudgetAction`'s own tag sets — driven over the wire
//! through the same `MultiServiceRouter` the production router serves from.
//!
//! The RULE asserted here: every write that establishes a resource's tag set is held to
//! "each tag consists of a key and a value, and each key must be unique for the resource"
//! (`ResourceTags`), so a list naming one key more than once states no value for that key
//! and is refused with `InvalidParameterException` naming the repeated key, creating
//! nothing; a list whose keys are all distinct is accepted and read back as supplied.
//! The sibling suite `resource_tags.rs` asserts the same rule for `TagResource`; this one
//! covers the two CREATE paths, so the rule holds on every path that writes a tag set.
//!
//! Each table spans its class rather than one request: the refused rows differ along where
//! the repeat sits (adjacent, separated by another key, at the end of a longer set), how
//! many entries one key gets (two, three), whether the repeated entries carry the same
//! value or different ones, and the form of the repeated key (short, at the published
//! length maximum, non-ascii); the accepted rows differ along the number of tags (one,
//! several, the published maximum), key case, and the value length at its published
//! minimum.
//!
//! The harness (`router`, `call`, `is_error`, `caller_account`), the create bodies and the
//! tag helpers are the sibling suites', so every suite in this target drives the service
//! identically.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The tag ceiling is the service's own constant, so a row built from it cannot drift
/// from the code under test.
use service_com_amazonaws_budgets_awsbudgetservicegateway::state::MAX_TAGS_PER_RESOURCE;

use super::budget_action_execution::{create_body, email_subscriber, iam_definition, seed_budget};
use super::budget_lifecycle::{caller_account, call, is_error, router};
use super::client_error_envelopes::message;
use super::resource_tags::{action_arn, budget_arn, list_tags, many_tags, pair, reported, tag_list};

/// A `CreateBudget` request whose budget satisfies every modelled constraint, carrying
/// `tags` as its `ResourceTags`, so a row varies only the tag set.
fn create_budget_body(account_id: &str, budget_name: &str, tags: &[(String, String)]) -> Value {
    json!({
        "AccountId": account_id,
        "Budget": {
            "BudgetName": budget_name,
            "TimeUnit": "MONTHLY",
            "BudgetType": "COST",
            "BudgetLimit": {"Amount": "100", "Unit": "USD"}
        },
        "ResourceTags": tag_list(tags)
    })
}

/// A `CreateBudgetAction` request carrying `tags` as its `ResourceTags`; every other
/// member is the simplest the model and the documented cross-member rules accept.
fn create_action_body(account_id: &str, budget_name: &str, tags: &[(String, String)]) -> Value {
    create_body(
        account_id,
        budget_name,
        "APPLY_IAM_POLICY",
        &iam_definition(),
        "AUTOMATIC",
        "ACTUAL",
        &json!({"ActionThresholdValue": 80.0, "ActionThresholdType": "PERCENTAGE"}),
        &json!([email_subscriber(0)]),
        Some(tag_list(tags)),
    )
}

/// One row of the refused table: the tags one create supplies and the key they name more
/// than once, which the refusal has to name for the caller to be able to fix the request.
struct Duplicate {
    case: &'static str,
    tags: Vec<(String, String)>,
    repeated: String,
}

/// The refused table.
fn duplicate_rows() -> Vec<Duplicate> {
    // A key at `ResourceTagKey`'s published `@length` maximum
    // (`smithy_query(query="shape",shape="ResourceTag")`: Key len 1..128).
    let longest = "k".repeat(128);
    let mut trailing = many_tags(5);
    trailing.push(pair("key-0", "a-second-value"));
    vec![
        Duplicate {
            // The case the issue recorded.
            case: "the same key twice, adjacent, with different values",
            tags: vec![pair("env", "dev"), pair("env", "prod")],
            repeated: "env".to_string(),
        },
        Duplicate {
            case: "the same key twice, separated by another key",
            tags: vec![pair("env", "dev"), pair("team", "cost"), pair("env", "prod")],
            repeated: "env".to_string(),
        },
        Duplicate {
            case: "the same key twice with the SAME value",
            tags: vec![pair("env", "dev"), pair("env", "dev")],
            repeated: "env".to_string(),
        },
        Duplicate {
            case: "one key given three values",
            tags: vec![pair("env", "a"), pair("env", "b"), pair("env", "c")],
            repeated: "env".to_string(),
        },
        Duplicate {
            case: "the repeat at the end of a longer set of distinct keys",
            tags: trailing,
            repeated: "key-0".to_string(),
        },
        Duplicate {
            case: "a key at the published length maximum, repeated",
            tags: vec![pair(&longest, "a"), pair(&longest, "b")],
            repeated: longest.clone(),
        },
        Duplicate {
            case: "a non-ascii key, repeated",
            tags: vec![pair("チーム", "予算"), pair("チーム", "原価")],
            repeated: "チーム".to_string(),
        },
    ]
}

/// The accepted table: tag sets whose keys are all distinct, which the rule leaves alone.
fn distinct_rows() -> Vec<(String, Vec<(String, String)>)> {
    vec![
        ("one tag".to_string(), vec![pair("owner", "platform")]),
        (
            "several distinct keys carrying the same value".to_string(),
            vec![pair("owner", "shared"), pair("team", "shared")],
        ),
        (
            // AWS tag keys are case-sensitive, so keys differing only in case are two
            // keys and neither repeats the other.
            "two keys differing only in case".to_string(),
            vec![pair("Env", "dev"), pair("env", "prod")],
        ),
        (
            // `ResourceTagValue` is len 0..256, so the empty value is published.
            "a non-ascii key with a value at the published length minimum".to_string(),
            vec![pair("チーム", ""), pair("owner", "platform")],
        ),
        (
            format!("the published maximum of {MAX_TAGS_PER_RESOURCE} distinct tags"),
            many_tags(MAX_TAGS_PER_RESOURCE),
        ),
    ]
}

/// The `Action`s `DescribeBudgetActionsForBudget` reports for `budget_name`.
async fn listed_action_ids(
    router: &MultiServiceRouter,
    account_id: &str,
    budget_name: &str,
) -> Vec<String> {
    let (status, body) = call(
        router,
        "DescribeBudgetActionsForBudget",
        &json!({"AccountId": account_id, "BudgetName": budget_name}),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected DescribeBudgetActionsForBudget({budget_name}) to answer 200, got \
         {status} {body}"
    );
    body["Actions"]
        .as_array()
        .map(|actions| {
            actions
                .iter()
                .map(|action| action["ActionId"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// THE RULE, refusal half, on `CreateBudget`: a `ResourceTags` list naming one key more
/// than once is refused with `InvalidParameterException` naming that key, and the budget
/// is not created — the name it asked for is still free afterwards.
#[tokio::test]
async fn create_budget_refuses_resource_tags_that_name_one_key_twice() {
    let router = router();
    let account_id = caller_account();

    for (index, row) in duplicate_rows().into_iter().enumerate() {
        let case = row.case;
        let budget_name = format!("tagdup-budget-{index}");
        let (status, body) = call(
            &router,
            "CreateBudget",
            &create_budget_body(&account_id, &budget_name, &row.tags),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the repeated tag key to be refused, got {status} {body}"
        );
        assert!(
            is_error(&body, "InvalidParameterException"),
            "case {case:?}: expected InvalidParameterException, got {body}"
        );
        let reported_message = message(&body);
        assert!(
            reported_message.contains(&row.repeated),
            "case {case:?}: expected the refusal to name the repeated key {:?}, got \
             {reported_message:?}",
            row.repeated
        );

        // Nothing was created: the budget the refused call named does not exist.
        let (status, body) = call(
            &router,
            "DescribeBudget",
            &json!({"AccountId": &account_id, "BudgetName": &budget_name}),
        )
        .await;
        assert!(
            is_error(&body, "NotFoundException"),
            "case {case:?}: expected the refused create to store no budget, got \
             {status} {body}"
        );

        // And the name is still free: the same create with the repeat dropped is served,
        // so the refusal consumed nothing.
        let mut distinct: Vec<(String, String)> = Vec::new();
        for (key, value) in row.tags {
            if !distinct.iter().any(|(held, _)| held == &key) {
                distinct.push((key, value));
            }
        }
        let (status, body) = call(
            &router,
            "CreateBudget",
            &create_budget_body(&account_id, &budget_name, &distinct),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the same create with distinct keys to be served, got \
             {status} {body}"
        );
        let (_, body) = list_tags(&router, &budget_arn(&account_id, &budget_name)).await;
        assert_eq!(
            reported(&body),
            distinct,
            "case {case:?}: expected the served create to hold one tag per key"
        );
    }
}

/// THE RULE, refusal half, on `CreateBudgetAction`: the same refusal, and no action is
/// hung off the budget the request named.
#[tokio::test]
async fn create_budget_action_refuses_resource_tags_that_name_one_key_twice() {
    let router = router();
    let account_id = caller_account();
    let budget_name = "tagdup-action-host";
    seed_budget(&router, &account_id, budget_name).await;

    for row in duplicate_rows() {
        let case = row.case;
        let (status, body) = call(
            &router,
            "CreateBudgetAction",
            &create_action_body(&account_id, budget_name, &row.tags),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the repeated tag key to be refused, got {status} {body}"
        );
        assert!(
            is_error(&body, "InvalidParameterException"),
            "case {case:?}: expected InvalidParameterException, got {body}"
        );
        let reported_message = message(&body);
        assert!(
            reported_message.contains(&row.repeated),
            "case {case:?}: expected the refusal to name the repeated key {:?}, got \
             {reported_message:?}",
            row.repeated
        );

        let actions = listed_action_ids(&router, &account_id, budget_name).await;
        assert!(
            actions.is_empty(),
            "case {case:?}: expected the refused create to hang no action off the budget, \
             got {actions:?}"
        );
    }
}

/// THE RULE, served half: a tag set whose keys are all distinct is created as supplied and
/// read back tag for tag, on both kinds of taggable resource ("a budget or budget action
/// resource").
#[tokio::test]
async fn a_create_whose_tag_keys_are_distinct_is_served_and_read_back() {
    let router = router();
    let account_id = caller_account();

    for (index, (case, tags)) in distinct_rows().into_iter().enumerate() {
        let budget_name = format!("tagok-{index}");
        let (status, body) = call(
            &router,
            "CreateBudget",
            &create_budget_body(&account_id, &budget_name, &tags),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateBudget to answer 200, got {status} {body}"
        );
        let (status, body) = list_tags(&router, &budget_arn(&account_id, &budget_name)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the budget's tags to be readable, got {status} {body}"
        );
        assert_eq!(
            reported(&body),
            tags,
            "case {case:?}: expected the budget to hold the tags it was created with"
        );

        let (status, body) = call(
            &router,
            "CreateBudgetAction",
            &create_action_body(&account_id, &budget_name, &tags),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateBudgetAction to answer 200, got {status} {body}"
        );
        let action_id = body["ActionId"].as_str().unwrap_or_default().to_string();
        let (status, body) =
            list_tags(&router, &action_arn(&account_id, &budget_name, &action_id)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the action's tags to be readable, got {status} {body}"
        );
        assert_eq!(
            reported(&body),
            tags,
            "case {case:?}: expected the action to hold the tags it was created with"
        );
    }
}
