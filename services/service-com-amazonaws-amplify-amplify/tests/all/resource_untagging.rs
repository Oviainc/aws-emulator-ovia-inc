// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of Amplify `UntagResource`, proved through the reads a
//! removal has to show up in: `ListTagsForResource` and the resource's own record.
//!
//! Every case drives the real wire path (the shared harness in
//! `tests/all/app_branch_lifecycle.rs`), so what is asserted is the JSON an AWS SDK
//! client parses — including the `@httpQuery("tagKeys")` binding, which sends one
//! `tagKeys=` parameter per key.
//!
//! The rule under test: a tag removal deletes from the tag map persisted on the
//! resource its ARN names exactly the keys the request lists and nothing else, so a
//! key the resource does not carry leaves the map untouched (the request asks for an
//! absence that already holds) and the call still succeeds with an empty body. An ARN
//! naming no resource of a kind this service persists tags for is refused 404 and
//! removes nothing.
//!
//! The STATUS a served removal answers with — `204 No Content`, because the response
//! carries no document — is its own suite: `tests/all/empty_response_status.rs`.
//!
//! The tables span the class the rule generalises over — the two resource kinds the
//! model gives a tag map (an app and a branch, the branch also under a name carrying
//! the slash a Git branch name commonly has), how many keys one call names, whether
//! those keys are carried, absent, repeated or reordered, keys at `TagKey`'s modelled
//! length maximum and over its punctuation range, tags set at create time and tags set
//! by a later write, and the `tagKeys` count at each published bound and one past the
//! top one — rather than the one recorded request.

use serde_json::{json, Value};

use crate::app_branch_lifecycle::{Emulator, Reply, ACCOUNT_ID, REGION};
use crate::resource_tags::{
    encode_label, list_tags, record_tags, seed, tag_resource, Kind, Seeded, KINDS,
};

/// `UntagResource`: `DELETE /tags/{resourceArn}?tagKeys=…&tagKeys=…`.
///
/// `tagKeys` targets `TagKeyList` bound to `@httpQuery("tagKeys")`
/// (`smithy_query(query="operation",operation="UntagResource")` reports
/// `DELETE /tags/{resourceArn}`), so every key travels as its own query pair, and the
/// ARN travels as ONE path segment with its `:` and `/` percent-encoded.
pub(crate) async fn untag_resource(
    emulator: &Emulator,
    resource_arn: &str,
    tag_keys: &[String],
) -> Reply {
    let query = tag_keys
        .iter()
        .map(|key| format!("tagKeys={}", encode_label(key)))
        .collect::<Vec<_>>()
        .join("&");
    emulator
        .call(
            "DELETE",
            &format!("/tags/{}?{query}", encode_label(resource_arn)),
            None,
        )
        .await
}

/// One removal case: the tags the resource is created with, the tags a later write adds,
/// the `tagKeys` lists removed in order, and the map the resource must carry afterwards.
struct UntagCase {
    case: &'static str,
    created_with: Value,
    written_after: Option<Value>,
    removals: Vec<Vec<String>>,
    expected: Value,
}

/// `TagKey`'s modelled maximum is 128 characters
/// (`smithy_query(query="shape",shape="TagMap")`): a key at that length is still an
/// ordinary key to remove.
fn longest_key() -> String {
    "k".repeat(128)
}

/// `n` keys with their values, as the tag map a create or a write carries.
fn numbered_tags(count: usize) -> Value {
    Value::Object(
        (0..count)
            .map(|index| (format!("key.{index}"), json!(format!("value-{index}"))))
            .collect(),
    )
}

/// `n` key names, as a `tagKeys` list.
fn numbered_keys(count: usize) -> Vec<String> {
    (0..count).map(|index| format!("key.{index}")).collect()
}

fn keys(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_string()).collect()
}

/// The removal table.
///
/// It varies the number of keys one call names (one, several, every key the resource
/// has, the `tagKeys` minimum of 1 and its maximum of 50 — `TagKeyList` is modelled
/// `length 1..50`, from `smithy_query(query="operation",operation="UntagResource")`),
/// whether those keys are carried by the resource, absent from it, repeated inside one
/// list or named in an order other than the one they were written in, how many calls do
/// the removing, where the tags came from (the create or a later `TagResource`), and the
/// shape of the key itself (`TagKey`'s length maximum, its punctuation range, a key
/// whose value is empty).
fn untag_cases() -> Vec<UntagCase> {
    vec![
        UntagCase {
            case: "one key of several removed, every other key left as it was",
            created_with: json!({ "env": "dev", "owner": "platform", "team": "web" }),
            written_after: None,
            removals: vec![keys(&["env"])],
            expected: json!({ "owner": "platform", "team": "web" }),
        },
        UntagCase {
            case: "several keys removed in one call",
            created_with: json!({ "a": "1", "b": "2", "c": "3", "d": "4" }),
            written_after: None,
            removals: vec![keys(&["a", "c"])],
            expected: json!({ "b": "2", "d": "4" }),
        },
        UntagCase {
            case: "every key removed leaves the empty map, not a missing one",
            created_with: json!({ "env": "dev", "owner": "platform" }),
            written_after: None,
            removals: vec![keys(&["env", "owner"])],
            expected: json!({}),
        },
        UntagCase {
            case: "a key the resource does not carry removes nothing",
            created_with: json!({ "env": "dev" }),
            written_after: None,
            removals: vec![keys(&["owner"])],
            expected: json!({ "env": "dev" }),
        },
        UntagCase {
            case: "a list mixing a carried key with an absent one removes only the carried one",
            created_with: json!({ "env": "dev", "owner": "platform" }),
            written_after: None,
            removals: vec![keys(&["env", "never-set"])],
            expected: json!({ "owner": "platform" }),
        },
        UntagCase {
            case: "the same key removed twice ends where one removal left it",
            created_with: json!({ "env": "dev", "keep": "me" }),
            written_after: None,
            removals: vec![keys(&["env"]), keys(&["env"])],
            expected: json!({ "keep": "me" }),
        },
        UntagCase {
            case: "a key repeated inside one list is removed once",
            created_with: json!({ "env": "dev", "keep": "me" }),
            written_after: None,
            removals: vec![keys(&["env", "env"])],
            expected: json!({ "keep": "me" }),
        },
        UntagCase {
            case: "the keys may be named in any order",
            created_with: json!({ "a": "1", "b": "2", "c": "3" }),
            written_after: None,
            removals: vec![keys(&["c", "a"])],
            expected: json!({ "b": "2" }),
        },
        UntagCase {
            case: "two calls of one key each remove both",
            created_with: json!({ "a": "1", "b": "2", "c": "3" }),
            written_after: None,
            removals: vec![keys(&["a"]), keys(&["b"])],
            expected: json!({ "c": "3" }),
        },
        UntagCase {
            case: "a key a later tag write added is removable like any other",
            created_with: json!({ "env": "dev" }),
            written_after: Some(json!({ "owner": "platform" })),
            removals: vec![keys(&["owner"])],
            expected: json!({ "env": "dev" }),
        },
        UntagCase {
            case: "a key a later tag write overwrote takes its key with it",
            created_with: json!({ "env": "dev", "keep": "me" }),
            written_after: Some(json!({ "env": "prod" })),
            removals: vec![keys(&["env"])],
            expected: json!({ "keep": "me" }),
        },
        UntagCase {
            case: "a key at TagKey's modelled length maximum",
            created_with: json!({ longest_key(): "v", "keep": "me" }),
            written_after: None,
            removals: vec![vec![longest_key()]],
            expected: json!({ "keep": "me" }),
        },
        UntagCase {
            case: "keys over the punctuation range TagKey admits",
            created_with: json!({
                "owner:group": "web",
                "cost.center": "4711",
                "team/name": "platform",
                "Zürich_plus-minus@1": "yes",
            }),
            written_after: None,
            removals: vec![keys(&["owner:group", "team/name", "Zürich_plus-minus@1"])],
            expected: json!({ "cost.center": "4711" }),
        },
        UntagCase {
            case: "an empty value does not shield its key from removal",
            created_with: json!({ "env": "", "keep": "me" }),
            written_after: None,
            removals: vec![keys(&["env"])],
            expected: json!({ "keep": "me" }),
        },
        UntagCase {
            case: "a resource carrying no tags is left carrying none",
            created_with: json!({}),
            written_after: None,
            removals: vec![keys(&["env"])],
            expected: json!({}),
        },
        UntagCase {
            case: "fifty keys, the tagKeys maximum, removed in one call",
            created_with: numbered_tags(50),
            written_after: None,
            removals: vec![numbered_keys(50)],
            expected: json!({}),
        },
        UntagCase {
            case: "forty-nine of fifty keys removed leaves exactly the one not named",
            created_with: numbered_tags(50),
            written_after: None,
            removals: vec![numbered_keys(49)],
            expected: json!({ "key.49": "value-49" }),
        },
    ]
}

#[tokio::test]
async fn a_tag_removal_deletes_exactly_the_keys_it_names() {
    for case in untag_cases() {
        for kind in KINDS {
            let emulator = Emulator::new();
            let seeded = seed(&emulator, kind, Some(&case.created_with)).await;

            if let Some(write) = &case.written_after {
                let written = tag_resource(&emulator, &seeded.arn, write).await;
                assert_eq!(
                    written.status, 200,
                    "case {:?} ({kind:?}): TagResource {write} expected 200, got {written:?}",
                    case.case
                );
            }

            for removal in &case.removals {
                let removed = untag_resource(&emulator, &seeded.arn, removal).await;
                assert_eq!(
                    removed.status, 204,
                    "case {:?} ({kind:?}): UntagResource {removal:?} expected 204, got {removed:?}",
                    case.case
                );
                // `UntagResourceResponse` declares no members
                // (`smithy_query(query="operation",operation="UntagResource")`).
                assert!(
                    removed.body == Value::Null || removed.body == json!({}),
                    "case {:?} ({kind:?}): UntagResource answers an empty body, got {:?}",
                    case.case,
                    removed.body
                );
            }

            let listed = list_tags(&emulator, &seeded.arn).await;
            assert_eq!(
                listed.status, 200,
                "case {:?} ({kind:?}): ListTagsForResource expected 200, got {listed:?}",
                case.case
            );
            assert_eq!(
                listed.member("tags"),
                case.expected,
                "case {:?} ({kind:?}): expected the remaining map {}, got {:?}",
                case.case,
                case.expected,
                listed.member("tags")
            );

            // The map has ONE home: the resource's own record reports the same thing.
            let on_the_record = record_tags(&emulator, &seeded).await;
            assert_eq!(
                on_the_record, case.expected,
                "case {:?} ({kind:?}): the resource record reports {on_the_record:?}, not the \
                 remaining map {}",
                case.case, case.expected
            );
        }
    }
}

#[tokio::test]
async fn a_removed_key_can_be_tagged_again_with_a_new_value() {
    for kind in KINDS {
        let emulator = Emulator::new();
        let seeded = seed(&emulator, kind, Some(&json!({ "env": "dev", "keep": "me" }))).await;

        let removed = untag_resource(&emulator, &seeded.arn, &keys(&["env"])).await;
        assert_eq!(
            removed.status, 204,
            "case {kind:?}: UntagResource expected 204, got {removed:?}"
        );
        assert_eq!(
            list_tags(&emulator, &seeded.arn).await.member("tags"),
            json!({ "keep": "me" }),
            "case {kind:?}: the removal must leave only the key it did not name"
        );

        // Removing a key is not a tombstone: tagging it again stores the new value, and
        // the key that was never removed is still there.
        let written = tag_resource(&emulator, &seeded.arn, &json!({ "env": "prod" })).await;
        assert_eq!(
            written.status, 200,
            "case {kind:?}: TagResource after the removal expected 200, got {written:?}"
        );
        assert_eq!(
            list_tags(&emulator, &seeded.arn).await.member("tags"),
            json!({ "env": "prod", "keep": "me" }),
            "case {kind:?}: re-tagging a removed key must store the new value"
        );
    }
}

#[tokio::test]
async fn a_removal_reaches_only_the_resource_its_arn_names() {
    let emulator = Emulator::new();

    // An app and a branch of that app, plus a second app, each carrying the SAME keys:
    // each resource's tag map is its own, so a removal addressed at one of them must
    // leave the others' maps whole.
    let shared = json!({ "env": "dev", "owner": "platform" });
    let app = seed(&emulator, Kind::App, Some(&shared)).await;
    let other_app = seed(&emulator, Kind::App, Some(&shared)).await;
    let branch_reply = emulator
        .create_branch(
            &app.app_id,
            &json!({ "branchName": "main", "tags": shared.clone() }),
        )
        .await;
    assert_eq!(
        branch_reply.status, 200,
        "CreateBranch must succeed, got {branch_reply:?}"
    );
    let branch = Seeded {
        kind: Kind::Branch("main"),
        app_id: app.app_id.clone(),
        arn: crate::app_branch_lifecycle::string_member(&branch_reply.member("branch"), "branchArn"),
    };

    let removed = untag_resource(&emulator, &branch.arn, &keys(&["env"])).await;
    assert_eq!(
        removed.status, 204,
        "UntagResource on the branch expected 204, got {removed:?}"
    );

    for (label, target, expected) in [
        ("the branch the removal named", &branch, json!({ "owner": "platform" })),
        ("the app that holds the branch", &app, shared.clone()),
        ("an unrelated app", &other_app, shared.clone()),
    ] {
        let listed = list_tags(&emulator, &target.arn).await;
        assert_eq!(
            listed.member("tags"),
            expected,
            "case {label:?}: expected {expected}, got {:?}",
            listed.member("tags")
        );
    }
}

#[tokio::test]
async fn an_arn_naming_no_taggable_resource_is_refused_and_removes_nothing() {
    let emulator = Emulator::new();

    // A tagged app and a webhook on it: the webhook exists, and `Webhook` models no tag
    // map (`smithy_query(query="shape",shape="Webhook")`), so its ARN names no resource
    // tagging can address.
    let seeded = seed(&emulator, Kind::App, Some(&json!({ "env": "dev" }))).await;
    let webhook = emulator
        .create_webhook(
            &seeded.app_id,
            &json!({ "branchName": "main", "description": "untagging" }),
        )
        .await;
    assert_eq!(webhook.status, 200, "CreateWebhook must succeed, got {webhook:?}");
    let webhook_arn =
        crate::app_branch_lifecycle::string_member(&webhook.member("webhook"), "webhookArn");

    // An `appId` no app in this store holds: it satisfies `AppId`'s modelled
    // `pattern ^d[a-z0-9]+$` (`smithy_query(query="shape",shape="AppId")`) and names
    // nothing.
    let absent_app_arn = format!("arn:aws:amplify:{REGION}:{ACCOUNT_ID}:apps/dabsent00000");

    for (label, arn, message) in [
        (
            "an app the store does not hold",
            absent_app_arn,
            "App dabsent00000 not found.".to_string(),
        ),
        (
            "a branch the addressed app does not hold",
            format!("{}/branches/nothing-here", seeded.arn),
            "Branch nothing-here not found.".to_string(),
        ),
        (
            "a webhook, a resource kind this service models no tag map on",
            webhook_arn.clone(),
            format!("Resource {webhook_arn} not found."),
        ),
        (
            "the apps collection with no app id after it",
            format!("arn:aws:amplify:{REGION}:{ACCOUNT_ID}:apps/"),
            format!("Resource arn:aws:amplify:{REGION}:{ACCOUNT_ID}:apps/ not found."),
        ),
        (
            "an ARN of this service with an empty resource part",
            format!("arn:aws:amplify:{REGION}:{ACCOUNT_ID}:"),
            format!("Resource arn:aws:amplify:{REGION}:{ACCOUNT_ID}: not found."),
        ),
    ] {
        let refused = untag_resource(&emulator, &arn, &keys(&["env"])).await;
        assert_eq!(
            refused.status, 404,
            "case {label:?}: UntagResource expected HTTP 404, got {refused:?}"
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {label:?}: expected ResourceNotFoundException, got {:?}",
            refused.error_code
        );
        assert_eq!(
            refused.message(),
            Some(message.clone()),
            "case {label:?}: expected the message {message:?}, got {:?}",
            refused.message()
        );
        // `ResourceNotFoundException$code` is @required
        // (`smithy_query(query="shape",shape="ResourceNotFoundException")`).
        assert_eq!(
            refused.body.get("code").and_then(Value::as_str),
            Some("ResourceNotFoundException"),
            "case {label:?}: the refusal must carry the error's code, got {:?}",
            refused.body
        );
    }

    // None of the refusals reached the resource that DOES exist.
    assert_eq!(
        list_tags(&emulator, &seeded.arn).await.member("tags"),
        json!({ "env": "dev" }),
        "the refused removals must leave the tagged app exactly as it was"
    );
}

#[tokio::test]
async fn a_tag_keys_list_outside_the_modelled_bound_is_not_served() {
    // `tagKeys` targets `TagKeyList`, modelled `length 1..50`
    // (`smithy_query(query="operation",operation="UntagResource")`), so a list of 51
    // keys and a request binding no `tagKeys` pair at all are both outside the bound.
    for kind in KINDS {
        let emulator = Emulator::new();
        let fifty = numbered_tags(50);
        let seeded = seed(&emulator, kind, Some(&fifty)).await;

        let mut past_the_bound = numbered_keys(50);
        past_the_bound.push("one.too.many".to_string());
        let refused = untag_resource(&emulator, &seeded.arn, &past_the_bound).await;
        assert!(
            refused.status >= 400,
            "case {kind:?}: 51 tagKeys is past the modelled maximum, got {refused:?}"
        );

        let no_keys = untag_resource(&emulator, &seeded.arn, &[]).await;
        assert!(
            no_keys.status >= 400,
            "case {kind:?}: a request binding no tagKeys is past the modelled minimum, got \
             {no_keys:?}"
        );

        // Neither refusal removed anything.
        assert_eq!(
            list_tags(&emulator, &seeded.arn).await.member("tags"),
            fifty,
            "case {kind:?}: a refused removal must leave the tag map whole"
        );

        // The bound itself is served: exactly 50 keys, and exactly one.
        let at_the_minimum = untag_resource(&emulator, &seeded.arn, &keys(&["key.0"])).await;
        assert_eq!(
            at_the_minimum.status, 204,
            "case {kind:?}: one tagKey is the modelled minimum, got {at_the_minimum:?}"
        );
        let at_the_maximum = untag_resource(&emulator, &seeded.arn, &numbered_keys(50)).await;
        assert_eq!(
            at_the_maximum.status, 204,
            "case {kind:?}: 50 tagKeys is the modelled maximum, got {at_the_maximum:?}"
        );
        assert_eq!(
            list_tags(&emulator, &seeded.arn).await.member("tags"),
            json!({}),
            "case {kind:?}: the two served removals must leave the map empty"
        );
    }
}
