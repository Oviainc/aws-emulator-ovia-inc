// SPDX-License-Identifier: CC0-1.0

//! `TagDeliveryStream` and `UntagDeliveryStream` over the `awsJson1_1` wire, read back
//! through `ListTagsForDeliveryStream`.
//!
//! The rules under test:
//!
//! * an untag deletes EXACTLY the keys its `TagKeys` names and leaves every other tag of
//!   that stream in place, ignoring a named key the stream does not carry — so repeating
//!   an untag changes nothing ("Removes tags from the specified Firehose stream... If you
//!   specify a tag that doesn't exist, the operation ignores it");
//! * a tag adds the keys its `Tags` names and replaces the value of a key the stream
//!   already carries, leaving every other tag in place ("Adds or updates tags... If you
//!   specify a tag that already exists, the tag value is replaced"), up to the documented
//!   ceiling of fifty tags per stream;
//! * either call on a stream that does not exist is refused with
//!   `ResourceNotFoundException`.
//!
//! Each test spans a SET of tag sets and key sets; the recorded "untag one of two keys"
//! case is one row. The harness (`Emulator`, `error_code`) is the lifecycle suite's and
//! the tag helpers are the listing suite's, so every request here goes through the same
//! real router.

use serde_json::{json, Value};

use crate::delivery_stream_lifecycle::{error_code, Emulator, PAST_THE_TRANSITION};
use crate::delivery_stream_tags::{
    create_tagged, generated_tags, list_tags, listed, owned, MAX_TAGS,
};

/// One row of an untag table: the case name, the tags the stream holds, and the keys the
/// untag names.
type UntagCase = (&'static str, Vec<(String, String)>, Vec<String>);

/// One row of a tag table: the case name, the tags the stream holds, and the tags the
/// call adds or replaces.
type TagCase = (&'static str, Vec<(String, String)>, Vec<(String, String)>);

/// Remove `keys` from `name`'s tags.
async fn untag(emulator: &Emulator, name: &str, keys: &[String]) -> (u16, Value) {
    emulator
        .call(
            "UntagDeliveryStream",
            json!({ "DeliveryStreamName": name, "TagKeys": keys }),
        )
        .await
}

/// Add or replace `tags` on `name`.
async fn tag(emulator: &Emulator, name: &str, tags: &[(String, String)]) -> (u16, Value) {
    emulator
        .call(
            "TagDeliveryStream",
            json!({
                "DeliveryStreamName": name,
                "Tags": tags
                    .iter()
                    .map(|(key, value)| json!({ "Key": key, "Value": value }))
                    .collect::<Vec<Value>>(),
            }),
        )
        .await
}

/// Every tag `name` carries now, in key order, read back through the modelled reader.
async fn tags_of(emulator: &Emulator, name: &str) -> Vec<(String, String)> {
    let (status, response) = list_tags(emulator, name, None, None).await;
    assert_eq!(status, 200, "list {name}: got {response:?}");
    listed(&response)
}

fn keys(tags: &[(String, String)]) -> Vec<String> {
    tags.iter().map(|(key, _)| key.clone()).collect()
}

fn owned_keys(given: &[&str]) -> Vec<String> {
    given.iter().map(|key| (*key).to_string()).collect()
}

/// The tags left after removing `removed` from `held`: the set difference, in key order.
fn difference(held: &[(String, String)], removed: &[String]) -> Vec<(String, String)> {
    let mut folded: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    for (key, value) in held {
        folded.insert(key.clone(), value.clone());
    }
    for key in removed {
        folded.remove(key);
    }
    folded.into_iter().collect()
}

/// The tags left after adding `added` to `held`: a key already held takes the new value,
/// every other key is kept, in key order.
fn merged(held: &[(String, String)], added: &[(String, String)]) -> Vec<(String, String)> {
    let mut folded: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    for (key, value) in held.iter().chain(added.iter()) {
        folded.insert(key.clone(), value.clone());
    }
    folded.into_iter().collect()
}

/// Keys and values spanning the modelled `TagKey` / `TagValue` character classes
/// (`smithy_query(query="shape",shape="TagKey,TagValue")`:
/// `[\p{L}\p{Z}\p{N}_.:\/=+\-@%]`, and `TagValue` may be empty) — a set the issue's own
/// case does not cite.
fn patterned_tags() -> Vec<(String, String)> {
    owned(&[
        ("dotted.key", "1.0"),
        ("colon:key", "a:b"),
        ("slash/key", "a/b"),
        ("equals=key", "a=b"),
        ("plus+key", "a+b"),
        ("dash-key", "a-b"),
        ("at@key", "a@b"),
        ("percent%key", "50%"),
        ("spaced key", "a value"),
        ("ключ", "значение"),
        ("键", "值"),
        ("under_score", ""),
    ])
}

#[tokio::test]
async fn untag_removes_exactly_the_named_keys_and_keeps_every_other_tag() {
    let patterned = patterned_tags();
    let fifty = generated_tags(MAX_TAGS);

    // (case, the tags the stream holds, the keys the untag names).
    let cases: Vec<UntagCase> = vec![
        (
            "one of two keys removed, the other kept",
            owned(&[("keep", "1"), ("drop", "2")]),
            owned_keys(&["drop"]),
        ),
        (
            "one of several keys removed",
            owned(&[("env", "prod"), ("app", "api"), ("zone", "c")]),
            owned_keys(&["app"]),
        ),
        (
            "several of several keys removed",
            owned(&[("env", "prod"), ("app", "api"), ("zone", "c"), ("tier", "1")]),
            owned_keys(&["app", "tier"]),
        ),
        (
            "every key removed",
            owned(&[("env", "prod"), ("app", "api")]),
            owned_keys(&["app", "env"]),
        ),
        (
            "the stream's only key removed",
            owned(&[("env", "prod")]),
            owned_keys(&["env"]),
        ),
        (
            "only keys the stream never carried",
            owned(&[("env", "prod"), ("app", "api")]),
            owned_keys(&["absent", "missing"]),
        ),
        (
            "a mix of carried and absent keys",
            owned(&[("env", "prod"), ("app", "api")]),
            owned_keys(&["env", "absent"]),
        ),
        (
            "the same key named twice in one request",
            owned(&[("env", "prod"), ("app", "api")]),
            owned_keys(&["env", "env"]),
        ),
        (
            "keys named in descending order",
            owned(&[("a", "1"), ("b", "2"), ("c", "3"), ("d", "4")]),
            owned_keys(&["d", "b"]),
        ),
        (
            "a key whose value is the empty string",
            owned(&[("unset", ""), ("set", "1")]),
            owned_keys(&["unset"]),
        ),
        (
            "keys spanning the modelled character class",
            patterned.clone(),
            owned_keys(&["ключ", "percent%key", "spaced key", "under_score"]),
        ),
        (
            "every key of the modelled character class set",
            patterned.clone(),
            keys(&patterned),
        ),
        (
            "one key of the modelled maximum of fifty",
            fifty.clone(),
            owned_keys(&["key-07"]),
        ),
        ("all but one of fifty", fifty.clone(), keys(&fifty[1..])),
        ("all fifty", fifty.clone(), keys(&fifty)),
    ];

    for (index, (case, held, removed)) in cases.into_iter().enumerate() {
        let emulator = Emulator::new();
        let name = format!("untagged-stream-{index}");
        create_tagged(&emulator, &name, &held).await;

        let (status, response) = untag(&emulator, &name, &removed).await;
        assert_eq!(status, 200, "case {case:?}: got {response:?}");

        let expected = difference(&held, &removed);
        let actual = tags_of(&emulator, &name).await;
        assert_eq!(
            actual, expected,
            "case {case:?}: removing {removed:?} from {held:?} leaves {expected:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn untagging_the_same_keys_again_changes_nothing() {
    // (case, the tags the stream holds, the keys each repeated untag names).
    let cases: Vec<UntagCase> = vec![
        (
            "a key that was carried",
            owned(&[("keep", "1"), ("drop", "2")]),
            owned_keys(&["drop"]),
        ),
        (
            "a key that was never carried",
            owned(&[("keep", "1")]),
            owned_keys(&["never"]),
        ),
        (
            "several keys, one of them absent",
            owned(&[("keep", "1"), ("drop", "2"), ("also", "3")]),
            owned_keys(&["drop", "also", "absent"]),
        ),
        (
            "every key the stream holds",
            owned(&[("one", "1"), ("two", "2")]),
            owned_keys(&["one", "two"]),
        ),
    ];

    for (index, (case, held, removed)) in cases.into_iter().enumerate() {
        let emulator = Emulator::new();
        let name = format!("idempotent-stream-{index}");
        create_tagged(&emulator, &name, &held).await;
        let expected = difference(&held, &removed);

        // Three calls: the first removes what it names, the next two must be no-ops.
        for attempt in 1..=3 {
            let (status, response) = untag(&emulator, &name, &removed).await;
            assert_eq!(
                status, 200,
                "case {case:?} attempt {attempt}: got {response:?}"
            );
            let actual = tags_of(&emulator, &name).await;
            assert_eq!(
                actual, expected,
                "case {case:?} attempt {attempt}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}

/// The seed the generated key subsets below are drawn from. Fixed, so a failure is
/// reproducible: it is quoted in every assertion message.
const SUBSET_SEED: u64 = 0x5f1e_2d3c_4b5a_6978;

#[tokio::test]
async fn untag_leaves_the_set_difference_for_generated_key_subsets() {
    // A bounded, deterministic sweep: the invariant is a set difference, so the cases
    // that matter differ in WHICH keys (and how many) a request names. Nothing is
    // written to disk and the generator is a plain LCG, so no crate is needed for it.
    let mut lcg = SUBSET_SEED;
    let mut next = move || {
        lcg = lcg.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        lcg >> 33
    };

    for round in 0..24_u32 {
        let total = 1 + usize::try_from(next() % 12).unwrap_or(0);
        let held = generated_tags(total);
        // Each held key is named with probability 1/2, plus one key no tag carries, so
        // present and absent keys are mixed in one request.
        let mut named: Vec<String> = keys(&held)
            .into_iter()
            .filter(|_| next() % 2 == 0)
            .collect();
        named.push(format!("absent-{round}"));

        let emulator = Emulator::new();
        let name = format!("swept-stream-{round}");
        create_tagged(&emulator, &name, &held).await;

        let (status, response) = untag(&emulator, &name, &named).await;
        assert_eq!(
            status, 200,
            "seed {SUBSET_SEED:#x} round {round}: got {response:?}"
        );

        let expected = difference(&held, &named);
        let actual = tags_of(&emulator, &name).await;
        assert_eq!(
            actual, expected,
            "seed {SUBSET_SEED:#x} round {round}: removing {named:?} from {total} tag(s) \
             leaves {expected:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn tag_adds_keys_and_replaces_values_leaving_every_other_tag() {
    let patterned = patterned_tags();

    // (case, the tags the stream was created with, the tags the request adds).
    let cases: Vec<TagCase> = vec![
        ("a stream created with no tags", Vec::new(), owned(&[("env", "prod")])),
        (
            "a key the stream does not carry",
            owned(&[("env", "prod")]),
            owned(&[("app", "api")]),
        ),
        (
            "a key the stream already carries",
            owned(&[("env", "staging"), ("app", "api")]),
            owned(&[("env", "prod")]),
        ),
        (
            "a mix of a new and an existing key",
            owned(&[("env", "staging")]),
            owned(&[("env", "prod"), ("zone", "c")]),
        ),
        (
            "a value replaced by the empty string",
            owned(&[("env", "prod")]),
            owned(&[("env", "")]),
        ),
        (
            "the same key twice in one request",
            owned(&[("env", "prod")]),
            owned(&[("app", "first"), ("app", "last")]),
        ),
        (
            "keys spanning the modelled character class",
            owned(&[("env", "prod")]),
            patterned.clone(),
        ),
        (
            "every value of a patterned set replaced",
            patterned.clone(),
            patterned
                .iter()
                .map(|(key, _)| (key.clone(), "replaced".to_string()))
                .collect(),
        ),
        (
            "the modelled maximum of fifty tags at once",
            Vec::new(),
            generated_tags(MAX_TAGS),
        ),
    ];

    for (index, (case, held, added)) in cases.into_iter().enumerate() {
        let emulator = Emulator::new();
        let name = format!("tagged-updates-stream-{index}");
        create_tagged(&emulator, &name, &held).await;

        let (status, response) = tag(&emulator, &name, &added).await;
        assert_eq!(status, 200, "case {case:?}: got {response:?}");

        let expected = merged(&held, &added);
        let actual = tags_of(&emulator, &name).await;
        assert_eq!(
            actual, expected,
            "case {case:?}: adding {added:?} to {held:?} leaves {expected:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn tag_and_untag_round_trip_through_the_listing_reader() {
    let emulator = Emulator::new();
    // Tagging is accepted while the stream is still `CREATING`, so no wait is needed.
    create_tagged(&emulator, "round-trip-stream", &owned(&[("keep", "1")])).await;

    // Add two keys, then remove one of the three: the other two must survive.
    let (status, response) = tag(
        &emulator,
        "round-trip-stream",
        &owned(&[("drop", "2"), ("also", "3")]),
    )
    .await;
    assert_eq!(status, 200, "tag: got {response:?}");
    let actual = tags_of(&emulator, "round-trip-stream").await;
    assert_eq!(
        actual,
        owned(&[("also", "3"), ("drop", "2"), ("keep", "1")]),
        "after tagging: got {actual:?}"
    );

    let (status, response) = untag(&emulator, "round-trip-stream", &owned_keys(&["drop"])).await;
    assert_eq!(status, 200, "untag: got {response:?}");
    let actual = tags_of(&emulator, "round-trip-stream").await;
    assert_eq!(
        actual,
        owned(&[("also", "3"), ("keep", "1")]),
        "after untagging only 'drop': got {actual:?}"
    );

    // The same tags are still there once the stream has become `ACTIVE`, and a tag
    // written while it is `ACTIVE` lands the same way.
    tokio::time::sleep(PAST_THE_TRANSITION).await;
    let (status, response) = tag(&emulator, "round-trip-stream", &owned(&[("late", "4")])).await;
    assert_eq!(status, 200, "tag an ACTIVE stream: got {response:?}");
    let actual = tags_of(&emulator, "round-trip-stream").await;
    assert_eq!(
        actual,
        owned(&[("also", "3"), ("keep", "1"), ("late", "4")]),
        "after tagging an ACTIVE stream: got {actual:?}"
    );
}

#[tokio::test]
async fn a_stream_holds_at_most_the_documented_fifty_tags() {
    let emulator = Emulator::new();
    // "Each Firehose stream can have up to 50 tags" (`TagDeliveryStream`).
    create_tagged(&emulator, "full-stream", &generated_tags(MAX_TAGS)).await;
    let full = tags_of(&emulator, "full-stream").await;
    assert_eq!(full.len(), MAX_TAGS, "created with fifty tags: got {full:?}");

    // One more DISTINCT key would be the fifty-first.
    let (status, refused) = tag(&emulator, "full-stream", &owned(&[("one-too-many", "x")])).await;
    assert_eq!(status, 400, "a fifty-first tag: got {refused:?}");
    assert_eq!(
        error_code(&refused),
        "LimitExceededException",
        "a fifty-first tag: got {refused:?}"
    );
    let unchanged = tags_of(&emulator, "full-stream").await;
    assert_eq!(
        unchanged, full,
        "a refused tag writes nothing: got {unchanged:?}"
    );

    // Replacing a key the stream already carries adds nothing, so it is accepted at the
    // ceiling.
    let existing = full[0].0.clone();
    let (status, response) = tag(
        &emulator,
        "full-stream",
        &[(existing.clone(), "replaced".to_string())],
    )
    .await;
    assert_eq!(status, 200, "replacing a value at the ceiling: {response:?}");
    let replaced = tags_of(&emulator, "full-stream").await;
    assert_eq!(
        replaced.len(),
        MAX_TAGS,
        "replacing a value keeps the count: got {replaced:?}"
    );
    assert_eq!(
        replaced
            .iter()
            .find(|(key, _)| *key == existing)
            .map(|(_, value)| value.as_str()),
        Some("replaced"),
        "the replaced value: got {replaced:?}"
    );

    // Freeing a slot makes room for the new key again.
    let (status, response) = untag(&emulator, "full-stream", std::slice::from_ref(&existing)).await;
    assert_eq!(status, 200, "untag one: got {response:?}");
    let (status, response) = tag(&emulator, "full-stream", &owned(&[("one-too-many", "x")])).await;
    assert_eq!(status, 200, "a tag into the freed slot: got {response:?}");
    let after = tags_of(&emulator, "full-stream").await;
    assert_eq!(after.len(), MAX_TAGS, "back at the ceiling: got {after:?}");
    assert!(
        after.iter().any(|(key, _)| key == "one-too-many")
            && !after.iter().any(|(key, _)| *key == existing),
        "the freed key is gone and the new one is there: got {after:?}"
    );
}

#[tokio::test]
async fn tagging_or_untagging_a_stream_that_does_not_exist_is_refused() {
    let emulator = Emulator::new();
    create_tagged(&emulator, "present-stream", &owned(&[("env", "prod")])).await;

    // Never created, and a name that only resembles one that exists.
    for name in ["absent-stream", "present-stream-2"] {
        let (status, refused) = tag(&emulator, name, &owned(&[("env", "prod")])).await;
        assert_eq!(status, 400, "tag {name:?}: got {refused:?}");
        assert_eq!(
            error_code(&refused),
            "ResourceNotFoundException",
            "tag {name:?}: got {refused:?}"
        );

        let (status, refused) = untag(&emulator, name, &owned_keys(&["env"])).await;
        assert_eq!(status, 400, "untag {name:?}: got {refused:?}");
        assert_eq!(
            error_code(&refused),
            "ResourceNotFoundException",
            "untag {name:?}: got {refused:?}"
        );
    }

    // A stream whose removal the clock has made due carries no tags to change either.
    tokio::time::sleep(PAST_THE_TRANSITION).await;
    let (status, deleted) = emulator
        .call(
            "DeleteDeliveryStream",
            json!({ "DeliveryStreamName": "present-stream" }),
        )
        .await;
    assert_eq!(status, 200, "delete: got {deleted:?}");
    tokio::time::sleep(PAST_THE_TRANSITION).await;

    for (operation, (status, refused)) in [
        (
            "TagDeliveryStream",
            tag(&emulator, "present-stream", &owned(&[("env", "prod")])).await,
        ),
        (
            "UntagDeliveryStream",
            untag(&emulator, "present-stream", &owned_keys(&["env"])).await,
        ),
    ] {
        assert_eq!(status, 400, "{operation} on a removed stream: {refused:?}");
        assert_eq!(
            error_code(&refused),
            "ResourceNotFoundException",
            "{operation} on a removed stream: {refused:?}"
        );
    }
}
