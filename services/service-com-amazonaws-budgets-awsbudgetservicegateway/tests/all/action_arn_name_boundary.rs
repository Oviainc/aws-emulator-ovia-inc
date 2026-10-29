// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the boundary between a budget's NAME and the `/action/`
//! separator inside a budget action's ARN, driven over the wire through the same
//! `MultiServiceRouter` the production router serves from.
//!
//! The RULE asserted here: a budget action's ARN is
//! `…:budget/{BudgetName}/action/{ActionId}`, so the three ARN-addressed tagging
//! operations must resolve the separator at the end of the WHOLE stored budget name.
//! `BudgetName` (`smithy_query(query="shape",shape="BudgetName")`) admits any name that
//! holds no `/action/` substring — a name may therefore END in `/action` and spell that
//! substring where it meets the separator's leading slash — while `ActionId` is a UUID
//! (`@length(36, 36)`, `^[a-fA-F0-9]{8}-…-[a-fA-F0-9]{12}$`) holding no slash of its own.
//! The separator is thus the LAST occurrence in the resource field, and every ARN the
//! service renders resolves back to the resource it was rendered for.
//!
//! The rows differ along the FORM of the budget name around that word: a name without it
//! at all, a name ending in it behind one slash and behind several, a name that is only
//! that word, a name beginning with the slashed word, the word twice, the word followed by
//! a slash, the word with a suffix that forms no separator, a non-ascii name ending in it,
//! and a name ending in it at `BudgetName`'s published `@length` maximum of 100.
//!
//! The harness (`router`, `call`, `is_error`, `caller_account`), the budget and action
//! setup, and the ARN and tag helpers are the sibling suites', so every suite in this
//! target drives the service identically.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The ARN resolution under test is the service's own, so a parse row cannot drift from
/// the code the wire rows drive.
use service_com_amazonaws_budgets_awsbudgetservicegateway::state::{
    parse_resource_arn, TaggedResource,
};

use super::budget_action_execution::{seed_action, seed_budget};
use super::budget_lifecycle::{call, caller_account, is_error, router};
use super::resource_tags::{action_arn, budget_arn, list_tags, pair, reported, tag_list};

/// `BudgetName`'s published `@length` maximum
/// (`smithy_query(query="shape",shape="BudgetName")`), so the longest row sits exactly on
/// it rather than past it.
const MAX_BUDGET_NAME: usize = 100;

/// The word the action separator is built from, without its slashes — the text a budget
/// name is allowed to carry and even to end in.
const ACTION_WORD: &str = "action";

/// One `TagResource` call.
async fn tag(
    router: &MultiServiceRouter,
    resource_arn: &str,
    tags: &[(String, String)],
) -> (u16, Value) {
    call(
        router,
        "TagResource",
        &json!({"ResourceARN": resource_arn, "ResourceTags": tag_list(tags)}),
    )
    .await
}

/// One `UntagResource` call.
async fn untag(router: &MultiServiceRouter, resource_arn: &str, keys: &[String]) -> (u16, Value) {
    call(
        router,
        "UntagResource",
        &json!({"ResourceARN": resource_arn, "ResourceTagKeys": keys}),
    )
    .await
}

/// Write `tags` through the ARN `arn` and fail the test if the call did not take.
async fn write_tags(router: &MultiServiceRouter, case: &str, arn: &str, tags: &[(String, String)]) {
    let (status, body) = tag(router, arn, tags).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected TagResource({arn}) to answer 200, got {status} {body}"
    );
}

/// The tags the resource `arn` names reports, as `(Key, Value)` pairs.
async fn held_tags(router: &MultiServiceRouter, case: &str, arn: &str) -> Vec<(String, String)> {
    let (status, body) = list_tags(router, arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListTagsForResource({arn}) to answer 200, got {status} {body}"
    );
    reported(&body)
}

/// The name rows: the forms a `BudgetName` can take around the separator's own word, each
/// admitted by the modelled `@pattern` (no `/action/` substring, no ':' and no '\').
fn name_rows() -> Vec<(String, String)> {
    vec![
        ("a name without that word at all".to_string(), "monthly-cost".to_string()),
        (
            "a name ending in the slashed word".to_string(),
            format!("team/{ACTION_WORD}"),
        ),
        (
            "a name that is only that word".to_string(),
            ACTION_WORD.to_string(),
        ),
        (
            "a name that is only the slashed word".to_string(),
            format!("/{ACTION_WORD}"),
        ),
        (
            "several slashes, the last segment that word".to_string(),
            format!("team/platform/{ACTION_WORD}"),
        ),
        (
            "that word twice".to_string(),
            format!("{ACTION_WORD}/{ACTION_WORD}"),
        ),
        (
            "that word followed by a slash".to_string(),
            format!("{ACTION_WORD}/"),
        ),
        (
            "that word with a suffix, which forms no separator".to_string(),
            format!("team/{ACTION_WORD}s"),
        ),
        (
            "a non-ascii name ending in the slashed word".to_string(),
            format!("予算/{ACTION_WORD}"),
        ),
        (
            "the published length maximum, ending in the slashed word".to_string(),
            format!(
                "{}/{ACTION_WORD}",
                "y".repeat(MAX_BUDGET_NAME - ACTION_WORD.len() - 1)
            ),
        ),
    ]
}

/// The rule at the resolution itself: the ARN the service renders for a resource parses
/// back to that resource, whatever the budget's name ends in.
///
/// An `ActionId` stands in for a minted one — the form `ActionId`'s modelled `@pattern`
/// requires (a UUID, so it carries no slash of its own).
#[test]
fn an_arn_parses_back_to_the_resource_it_was_rendered_for() {
    let account_id = caller_account();
    let action_id = "11111111-2222-3333-4444-555555555555";

    for (case, name) in name_rows() {
        assert!(
            name.chars().count() <= MAX_BUDGET_NAME && !name.contains("/action/"),
            "case {case:?}: the row itself must be a name the modelled pattern admits, got \
             {name:?}"
        );

        let rendered = action_arn(&account_id, &name, action_id);
        let actual = parse_resource_arn(&rendered);
        let expected = Ok(TaggedResource::Action {
            account_id: account_id.clone(),
            budget_name: name.clone(),
            action_id: action_id.to_string(),
        });
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {rendered} to resolve to that budget's action, got \
             {actual:?}"
        );

        let rendered = budget_arn(&account_id, &name);
        let actual = parse_resource_arn(&rendered);
        let expected = Ok(TaggedResource::Budget {
            account_id: account_id.clone(),
            budget_name: name.clone(),
        });
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {rendered} to resolve to the budget itself, got {actual:?}"
        );
    }
}

/// The rule over the wire: all three ARN-addressed tagging operations reach the action an
/// action ARN names, and the budget a budget ARN names, for every form of budget name.
#[tokio::test]
async fn the_tag_plane_reaches_an_action_of_a_budget_whose_name_ends_in_the_separators_word() {
    let router = router();
    let account_id = caller_account();

    for (case, name) in name_rows() {
        seed_budget(&router, &account_id, &name).await;
        let action_id = seed_action(&router, &account_id, &name).await;
        let budget = budget_arn(&account_id, &name);
        let action = action_arn(&account_id, &name, &action_id);

        // Distinct tag sets, so a call that reaches the other resource fails the row
        // rather than passing it.
        let on_budget = vec![pair("scope", "budget"), pair("name", &name)];
        let on_action = vec![pair("scope", "action"), pair("name", &name)];
        write_tags(&router, &case, &budget, &on_budget).await;
        write_tags(&router, &case, &action, &on_action).await;

        // ListTagsForResource: each ARN reports its own resource's tags.
        let actual = held_tags(&router, &case, &action).await;
        assert_eq!(
            actual, on_action,
            "case {case:?}: expected the action's own tags for its ARN, got {actual:?}"
        );
        let actual = held_tags(&router, &case, &budget).await;
        assert_eq!(
            actual, on_budget,
            "case {case:?}: expected the budget's own tags for its ARN, got {actual:?}"
        );

        // TagResource through the action's ARN re-values that action's tag, and only it.
        write_tags(&router, &case, &action, &[pair("scope", "re-valued")]).await;
        let expected = vec![pair("scope", "re-valued"), pair("name", &name)];
        let actual = held_tags(&router, &case, &action).await;
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the action's tag re-valued through its ARN, got {actual:?}"
        );

        // UntagResource through the same ARN deletes from the same action.
        let (status, body) = untag(&router, &action, &["scope".to_string()]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UntagResource({action}) to answer 200, got {status} {body}"
        );
        let actual = held_tags(&router, &case, &action).await;
        assert_eq!(
            actual,
            vec![pair("name", &name)],
            "case {case:?}: expected only the key no call named to be left, got {actual:?}"
        );

        // None of the three writes touched the budget the action hangs under.
        let actual = held_tags(&router, &case, &budget).await;
        assert_eq!(
            actual, on_budget,
            "case {case:?}: expected the budget's tags untouched by the action's calls, got \
             {actual:?}"
        );
    }
}

/// Two budgets whose names overlap at the separator — `team` and `team/action` — each hold
/// their own action, and each of the four ARNs names exactly one of the four resources. An
/// action id paired with the other budget's name names no resource of this service.
#[tokio::test]
async fn overlapping_budget_names_do_not_share_an_action_arn() {
    let router = router();
    let account_id = caller_account();

    let shorter = "team".to_string();
    let longer = format!("team/{ACTION_WORD}");
    seed_budget(&router, &account_id, &shorter).await;
    seed_budget(&router, &account_id, &longer).await;
    let shorter_action = seed_action(&router, &account_id, &shorter).await;
    let longer_action = seed_action(&router, &account_id, &longer).await;

    let resources = [
        ("the shorter budget", budget_arn(&account_id, &shorter)),
        ("the longer budget", budget_arn(&account_id, &longer)),
        (
            "the shorter budget's action",
            action_arn(&account_id, &shorter, &shorter_action),
        ),
        (
            "the longer budget's action",
            action_arn(&account_id, &longer, &longer_action),
        ),
    ];
    for (case, arn) in &resources {
        write_tags(&router, case, arn, &[pair("holder", case)]).await;
    }
    for (case, arn) in &resources {
        let actual = held_tags(&router, case, arn).await;
        assert_eq!(
            actual,
            vec![pair("holder", case)],
            "case {case:?}: expected each ARN to hold only its own resource's tag, got {actual:?}"
        );
    }

    // Each action id under the OTHER budget's name: a well-formed ARN naming no resource.
    let crossed = [
        (
            "the shorter budget's action id under the longer name",
            action_arn(&account_id, &longer, &shorter_action),
        ),
        (
            "the longer budget's action id under the shorter name",
            action_arn(&account_id, &shorter, &longer_action),
        ),
    ];
    for (case, arn) in &crossed {
        for operation in ["ListTagsForResource", "TagResource", "UntagResource"] {
            let (status, body) = match operation {
                "ListTagsForResource" => list_tags(&router, arn).await,
                "TagResource" => tag(&router, arn, &[pair("holder", "crossed")]).await,
                _ => untag(&router, arn, &["holder".to_string()]).await,
            };
            assert_eq!(
                status, 400,
                "case {case:?} through {operation}: expected the refusal, got {status} {body}"
            );
            assert!(
                is_error(&body, "NotFoundException"),
                "case {case:?} through {operation}: expected NotFoundException, got {body}"
            );
        }
    }

    // The refusals wrote nothing: every resource still holds the one tag it was given.
    for (case, arn) in &resources {
        let actual = held_tags(&router, case, arn).await;
        assert_eq!(
            actual,
            vec![pair("holder", case)],
            "case {case:?}: expected the crossed refusals to change nothing, got {actual:?}"
        );
    }
}
