// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for `UntagResource`, driven over the wire through the same
//! `MultiServiceRouter` the production router serves from, with every result read back
//! through `ListTagsForResource`.
//!
//! The RULES asserted here:
//!
//! 1. `UntagResource` deletes from the resource its `ResourceARN` names exactly the tags
//!    whose keys `ResourceTagKeys` lists, and leaves every other tag of that resource —
//!    value and position alike — as it was. Both a budget and a budget action are such a
//!    resource ("a budget or budget action resource").
//! 2. A key names at most one tag of a resource ("each key must be unique for the
//!    resource"), so naming a key is the whole statement of the deletion: a key the
//!    resource does not carry deletes nothing, a key named twice in one request states the
//!    same deletion twice, and repeating a whole call changes nothing the first did not.
//! 3. The ARN is the whole identity of the resource: a well-formed budgets ARN naming a
//!    budget or action this service does not hold locates nothing
//!    (`NotFoundException`), and a value that is not a budgets resource ARN is an invalid
//!    input value (`InvalidParameterException`). Neither deletes anything.
//! 4. The deletion reaches the one resource the ARN names: a sibling budget, the budget's
//!    own actions and an action's own budget keep their tags.
//!
//! Each table spans its class rather than one request: the rows differ along the number of
//! keys deleted (none, one, several, every tag the resource holds, the published maximum),
//! WHERE the deleted keys sit in the held set (first, middle, last, scattered), whether a
//! named key is held at all, the key length at each published `@length` bound of
//! `ResourceTagKey`, non-ascii keys, and the resource kind.
//!
//! The harness (`router`, `call`, `is_error`, `caller_account`) and the ARN, tag-list and
//! tag-count helpers are the sibling suites', so every suite in this target drives the
//! service identically.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The ARN namespace and the tag ceiling are the service's own constants, so a table built
/// from them cannot drift from the code under test.
use service_com_amazonaws_budgets_awsbudgetservicegateway::state::{
    ARN_SERVICE, MAX_TAGS_PER_RESOURCE,
};

use super::budget_action_execution::{seed_action, seed_budget};
use super::budget_lifecycle::{account, call, caller_account, is_error, router};
use super::resource_tags::{action_arn, budget_arn, list_tags, many_tags, pair, reported, tag_list};

/// One `TagResource` call, for the setup each row starts from.
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

/// Put `tags` on the resource `arn` names and fail the test if the setup call did not take.
async fn seed_tags(router: &MultiServiceRouter, arn: &str, tags: &[(String, String)]) {
    let (status, body) = tag(router, arn, tags).await;
    assert_eq!(
        status, 200,
        "setup: expected TagResource({arn}) to answer 200, got {status} {body}"
    );
}

/// The tags the resource `arn` names reports, as `(Key, Value)` pairs.
async fn held_tags(
    router: &MultiServiceRouter,
    arn: &str,
    case: &str,
) -> Vec<(String, String)> {
    let (status, body) = list_tags(router, arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListTagsForResource to answer 200, got {status} {body}"
    );
    reported(&body)
}

/// What the rule says the resource holds once `keys` are deleted from `held`: every tag
/// whose key is not named, in the order it was already in. Derived here rather than written
/// out per row, so each row states only its inputs.
fn remaining(held: &[(String, String)], keys: &[String]) -> Vec<(String, String)> {
    held.iter()
        .filter(|(key, _)| !keys.contains(key))
        .cloned()
        .collect()
}

/// Keys as a `Vec<String>`, so a row names them as the strings they are.
fn keys(named: &[&str]) -> Vec<String> {
    named.iter().map(|key| key.to_string()).collect()
}

/// One row of the deletion table: a case name, the tags the resource starts from, and the
/// keys one `UntagResource` call names.
struct Row {
    case: String,
    held: Vec<(String, String)>,
    deleted: Vec<String>,
}

/// The tag set the position rows start from: four tags, so a row can delete the first, a
/// middle one, the last, or a scattered pair and still have neighbours left to check the
/// order of.
fn four_tags() -> Vec<(String, String)> {
    vec![
        pair("owner", "platform"),
        pair("env", "prod"),
        pair("cost-center", "4815"),
        pair("team", "finance"),
    ]
}

/// The deletion table. Its rows differ along what the rule generalises over: how many keys
/// are named (none, one, several, every key held, the `ResourceTagKeyList` maximum), where
/// the named keys sit in the held set, whether a named key is held at all, the key length
/// at each published `@length` bound of `ResourceTagKey` (1 and 128), and non-ascii keys.
fn deletion_rows() -> Vec<Row> {
    let max_set = many_tags(MAX_TAGS_PER_RESOURCE);
    vec![
        Row {
            case: "no keys at all".to_string(),
            held: four_tags(),
            deleted: Vec::new(),
        },
        Row {
            // The case the issue recorded: one key, deleted from a resource that holds it.
            case: "the first key held".to_string(),
            held: four_tags(),
            deleted: keys(&["owner"]),
        },
        Row {
            case: "a key in the middle of the held set".to_string(),
            held: four_tags(),
            deleted: keys(&["env"]),
        },
        Row {
            case: "the last key held".to_string(),
            held: four_tags(),
            deleted: keys(&["team"]),
        },
        Row {
            case: "two keys scattered through the held set".to_string(),
            held: four_tags(),
            deleted: keys(&["owner", "cost-center"]),
        },
        Row {
            case: "every key the resource holds".to_string(),
            held: four_tags(),
            deleted: keys(&["owner", "env", "cost-center", "team"]),
        },
        Row {
            case: "a key the resource does not hold".to_string(),
            held: four_tags(),
            deleted: keys(&["never-written"]),
        },
        Row {
            case: "a held key together with one that is not held".to_string(),
            held: four_tags(),
            deleted: keys(&["env", "never-written"]),
        },
        Row {
            case: "a key at the published length minimum".to_string(),
            held: vec![pair("k", "v"), pair("env", "prod")],
            deleted: keys(&["k"]),
        },
        Row {
            case: "a key at the published length maximum".to_string(),
            held: vec![pair(&"k".repeat(128), "v"), pair("env", "prod")],
            deleted: vec!["k".repeat(128)],
        },
        Row {
            case: "a non-ascii key".to_string(),
            held: vec![pair("チーム", "予算-2026"), pair("env", "prod")],
            deleted: keys(&["チーム"]),
        },
        Row {
            case: "a key whose tag value is empty".to_string(),
            held: vec![pair("empty", ""), pair("env", "prod")],
            deleted: keys(&["empty"]),
        },
        Row {
            case: format!("the published maximum of {MAX_TAGS_PER_RESOURCE} keys in one call"),
            held: max_set.clone(),
            deleted: max_set.iter().map(|(key, _)| key.clone()).collect(),
        },
        Row {
            case: format!("{MAX_TAGS_PER_RESOURCE} keys of which only one is held"),
            held: vec![pair("key-7", "value-7"), pair("kept", "yes")],
            deleted: max_set.iter().map(|(key, _)| key.clone()).collect(),
        },
    ]
}

/// RULE 1: exactly the named keys go away, and every other tag keeps its value and its
/// place.
///
/// Every row of the deletion table is run against BOTH kinds of taggable resource — a
/// budget and a budget action of that budget — so the rule holds over the key set and over
/// the resource kind.
#[tokio::test]
async fn untagging_deletes_exactly_the_named_keys() {
    let router = router();
    let account_id = caller_account();

    for (index, row) in deletion_rows().into_iter().enumerate() {
        let budget_name = format!("deletion-{index}");
        seed_budget(&router, &account_id, &budget_name).await;
        let action_id = seed_action(&router, &account_id, &budget_name).await;

        for (kind, arn) in [
            ("budget", budget_arn(&account_id, &budget_name)),
            (
                "budget action",
                action_arn(&account_id, &budget_name, &action_id),
            ),
        ] {
            let case = format!("{} on a {kind}", row.case);
            seed_tags(&router, &arn, &row.held).await;

            let (status, body) = untag(&router, &arn, &row.deleted).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected UntagResource to answer 200, got {status} {body}"
            );

            let expected = remaining(&row.held, &row.deleted);
            let actual = held_tags(&router, &arn, &case).await;
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the named keys gone and the rest unchanged, got \
                 {actual:?}"
            );
        }
    }
}

/// RULE 2: naming a key is the whole statement of the deletion, so the operation is
/// idempotent in every direction a request can repeat itself.
///
/// The steps run over one resource: a key named twice in ONE request, the same call issued
/// again after it already took, a call naming only keys that are gone, and an empty key
/// list. Run over a budget and over an action of it.
#[tokio::test]
async fn naming_a_key_that_is_already_gone_deletes_nothing_and_is_served() {
    let router = router();
    let account_id = caller_account();
    let budget_name = "idempotent";
    seed_budget(&router, &account_id, budget_name).await;
    let action_id = seed_action(&router, &account_id, budget_name).await;

    for (kind, arn) in [
        ("budget", budget_arn(&account_id, budget_name)),
        (
            "budget action",
            action_arn(&account_id, budget_name, &action_id),
        ),
    ] {
        let held = four_tags();
        seed_tags(&router, &arn, &held).await;

        let mut expected = held.clone();
        for (step, named) in [
            ("one key named twice in one request", keys(&["env", "env"])),
            ("that same request again", keys(&["env", "env"])),
            ("a key that is already gone", keys(&["env"])),
            ("an empty key list", Vec::new()),
            (
                "a key held, named beside one already gone",
                keys(&["env", "team"]),
            ),
            ("that last request again", keys(&["env", "team"])),
        ] {
            let case = format!("{step} on a {kind}");
            let (status, body) = untag(&router, &arn, &named).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected UntagResource to answer 200, got {status} {body}"
            );

            expected = remaining(&expected, &named);
            let actual = held_tags(&router, &arn, &case).await;
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the resource to hold the tags no request named, \
                 got {actual:?}"
            );
        }
    }
}

/// RULE 2, the write-back half: a key deleted can be written again, and it comes back as a
/// NEW key — at the end of the tag set, since the tag it had is gone rather than hidden.
#[tokio::test]
async fn a_deleted_key_written_again_is_a_new_tag() {
    let router = router();
    let account_id = caller_account();
    let budget_name = "rewritten";
    seed_budget(&router, &account_id, budget_name).await;
    let arn = budget_arn(&account_id, budget_name);

    let held = four_tags();
    seed_tags(&router, &arn, &held).await;

    let (status, body) = untag(&router, &arn, &keys(&["owner"])).await;
    assert_eq!(
        status, 200,
        "case \"delete the first key\": expected UntagResource to answer 200, got {status} {body}"
    );

    seed_tags(&router, &arn, &[pair("owner", "fin-ops")]).await;
    let actual = held_tags(&router, &arn, "a deleted key written again").await;
    let expected = vec![
        pair("env", "prod"),
        pair("cost-center", "4815"),
        pair("team", "finance"),
        pair("owner", "fin-ops"),
    ];
    assert_eq!(
        actual, expected,
        "case \"a deleted key written again\": expected the key back with its new value at \
         the end of the set, got {actual:?}"
    );
}

/// RULE 3: the ARN is the whole identity of the resource, so an ARN naming no resource of
/// this service is refused and deletes nothing.
///
/// The not-found rows differ along which part of the identity is wrong (the account, the
/// budget name, its case, the action id, the budget an action id belongs to); the invalid
/// rows along how the value departs from the ARN form.
#[tokio::test]
async fn an_arn_that_names_no_resource_of_this_service_deletes_nothing() {
    let router = router();
    let owner = caller_account();

    seed_budget(&router, &owner, "owning").await;
    seed_budget(&router, &owner, "sibling").await;
    let owned_action = seed_action(&router, &owner, "owning").await;
    let sibling_action = seed_action(&router, &owner, "sibling").await;

    let held = four_tags();
    let live = [
        budget_arn(&owner, "owning"),
        budget_arn(&owner, "sibling"),
        action_arn(&owner, "owning", &owned_action),
        action_arn(&owner, "sibling", &sibling_action),
    ];
    for arn in &live {
        seed_tags(&router, arn, &held).await;
    }

    let absent_action = "11111111-2222-3333-4444-555555555555";
    let not_found = [
        (
            "an account that holds no budget",
            budget_arn(&account(868_686_868), "owning"),
        ),
        (
            "a budget name nobody created",
            budget_arn(&owner, "never-created"),
        ),
        (
            "a stored budget name in another case",
            budget_arn(&owner, "OWNING"),
        ),
        (
            "an action id no action carries",
            action_arn(&owner, "owning", absent_action),
        ),
        (
            "an action id that belongs to another budget",
            action_arn(&owner, "owning", &sibling_action),
        ),
        (
            "an action under a budget nobody created",
            action_arn(&owner, "never-created", &owned_action),
        ),
    ];
    for (case, arn) in &not_found {
        let (status, body) = untag(&router, arn, &keys(&["owner"])).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the call to be refused, got {status} {body}"
        );
        assert!(
            is_error(&body, "NotFoundException"),
            "case {case:?}: expected NotFoundException, got {body}"
        );
    }

    let invalid = [
        ("a value that is not an ARN", "not-an-arn".to_string()),
        (
            "another service's ARN",
            format!("arn:aws:sns::{owner}:budget/owning"),
        ),
        (
            "a region, where a global service's ARN has none",
            format!("arn:aws:{ARN_SERVICE}:us-east-1:{owner}:budget/owning"),
        ),
        (
            "an account that is not 12 digits",
            format!("arn:aws:{ARN_SERVICE}::12345:budget/owning"),
        ),
        (
            "no partition",
            format!("arn::{ARN_SERVICE}::{owner}:budget/owning"),
        ),
        (
            "a collection this service does not name",
            format!("arn:aws:{ARN_SERVICE}::{owner}:billing-view/owning"),
        ),
        (
            "an empty budget name",
            format!("arn:aws:{ARN_SERVICE}::{owner}:budget/"),
        ),
        (
            "an empty action id",
            format!("arn:aws:{ARN_SERVICE}::{owner}:budget/owning/action/"),
        ),
        (
            "too few ARN fields",
            format!("arn:aws:{ARN_SERVICE}:{owner}:budget/owning"),
        ),
    ];
    for (case, arn) in &invalid {
        let (status, body) = untag(&router, arn, &keys(&["owner"])).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the call to be refused, got {status} {body}"
        );
        assert!(
            is_error(&body, "InvalidParameterException"),
            "case {case:?}: expected InvalidParameterException, got {body}"
        );
    }

    for arn in &live {
        let actual = held_tags(&router, arn, arn).await;
        assert_eq!(
            actual, held,
            "case {arn:?}: expected the refusals to delete nothing, got {actual:?}"
        );
    }
}

/// RULE 4: the deletion reaches the one resource the ARN names. Untagging each resource in
/// turn leaves the others holding every tag they were given.
#[tokio::test]
async fn untagging_reaches_only_the_resource_the_arn_names() {
    let router = router();
    let owner = caller_account();

    seed_budget(&router, &owner, "owning").await;
    seed_budget(&router, &owner, "sibling").await;
    let first = seed_action(&router, &owner, "owning").await;
    let second = seed_action(&router, &owner, "owning").await;

    let targets = [
        ("the budget", budget_arn(&owner, "owning")),
        ("its sibling budget", budget_arn(&owner, "sibling")),
        ("its first action", action_arn(&owner, "owning", &first)),
        ("its second action", action_arn(&owner, "owning", &second)),
    ];

    let held = four_tags();
    for (_, arn) in &targets {
        seed_tags(&router, arn, &held).await;
    }

    // One target at a time loses one key; every other target must still hold all four.
    for (index, (case, arn)) in targets.iter().enumerate() {
        let doomed = keys(&[held[index].0.as_str()]);
        let (status, body) = untag(&router, arn, &doomed).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UntagResource to answer 200, got {status} {body}"
        );

        for (other_index, (other_case, other_arn)) in targets.iter().enumerate() {
            let expected = if other_index <= index {
                remaining(&held, &keys(&[held[other_index].0.as_str()]))
            } else {
                held.clone()
            };
            let actual = held_tags(&router, other_arn, other_case).await;
            assert_eq!(
                actual, expected,
                "case {case:?} seen from {other_case:?}: expected each resource to lose only \
                 the keys its own call named, got {actual:?}"
            );
        }
    }
}
