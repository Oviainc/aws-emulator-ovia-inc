// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the rules a supplied TAG MAP answers to, on every operation
//! that accepts one: `CreateDatastore`, `TagResource` and `UntagResource`.
//!
//! RULE: a tag key a request supplies is accepted only when it satisfies every
//! constraint the model puts on `com.amazonaws.medicalimaging#TagKey` — its `@length`
//! (1..128) and its `@pattern` `^(?!aws:)[a-zA-Z+-=._:/]+$`, whose leading negative
//! lookahead reserves the `aws:` namespace — and a request may not leave one resource
//! holding more tags than the per-resource limit; a request that breaks either is
//! refused with HTTP 400 `ValidationException` and persists nothing. The rule belongs to
//! the tag MAP, so it does not vary with the operation installing it: a map one write
//! path refuses cannot be installed through the other.
//!
//! Every suite is a TABLE whose rows differ along what the rule generalises over: which
//! clause of the key pattern a key breaks (reserved prefix, character class, each length
//! bound), which character class an admitted key uses, how many entries the map carries,
//! and which operation carries it. Each case drives real HTTP through the production
//! front door, so what is asserted is the wire behaviour a `medical-imaging` SDK client
//! sees.

use serde_json::{json, Value};

use service_com_amazonaws_medicalimaging_ahigatewayservice::state::{
    MAX_TAGS_PER_RESOURCE, TAG_KEY_MAX_LENGTH, TAG_KEY_MIN_LENGTH, TAG_KEY_PATTERN,
    TAG_KEY_RESERVED_PREFIX,
};

use crate::datastore_lifecycle::{Answer, Emulator, ACCOUNT};
use crate::resource_tagging::{list_tags, tag, tag_object, untag};

/// A tag map holding exactly `key`, with a value every row shares (the rule under test
/// is about the KEY; `TagValue` is the map's element, which the boundary validator
/// already enforces).
fn one_tag(key: &str) -> Value {
    json!({ key: "v" })
}

/// `n` distinct, legal tag keys — `key0`, `key1`, … — with a value each.
fn distinct_tags(n: usize) -> Vec<(String, String)> {
    (0..n)
        .map(|index| (format!("key{index}"), format!("value{index}")))
        .collect()
}

/// The ARN of the data store `datastore_id` names, as the data store itself reports it.
async fn datastore_arn(emulator: &Emulator, datastore_id: &str) -> String {
    let read = emulator.get_datastore(datastore_id).await;
    assert_eq!(
        read.status, 200,
        "reading back {datastore_id}: expected 200, got {read:?}"
    );
    let arn = read.string(&["datastoreProperties", "datastoreArn"]);
    assert_eq!(
        arn,
        format!("arn:aws:medical-imaging:us-east-1:{ACCOUNT}:datastore/{datastore_id}"),
        "reading back {datastore_id}: expected the canonical data store ARN, got {read:?}"
    );
    arn
}

/// How many data stores this emulator holds, read off `ListDatastores`. Used to assert
/// that a refused `CreateDatastore` minted nothing.
async fn datastore_count(emulator: &Emulator) -> usize {
    let listed = emulator.list_datastores("").await;
    assert_eq!(listed.status, 200, "ListDatastores: got {listed:?}");
    listed
        .field(&["datastoreSummaries"])
        .as_array()
        .map(Vec::len)
        .unwrap_or_default()
}

/// Assert `answer` is the modelled refusal of an illegal tag map: HTTP 400 with the
/// error code `ValidationException` and a message that says something.
fn assert_refused(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400, got {answer:?}"
    );
    assert_eq!(
        answer.error_code, "ValidationException",
        "case {case:?}: expected error code ValidationException, got {answer:?}"
    );
    assert!(
        !answer.string(&["message"]).is_empty(),
        "case {case:?}: expected a message stating the constraint, got {answer:?}"
    );
}

// ---------------------------------------------------------------------------
// The keys the model admits
// ---------------------------------------------------------------------------

/// One admitted key per clause of the character class `[a-zA-Z+-=._:/]` and per published
/// `@length` bound of `TagKey` (len 1..128), sourced from
/// `smithy_query(query="shape",shape="TagMap,TagKey")`. Inside a character class `+-=` is
/// the RANGE from `+` (0x2B) to `=` (0x3D), so the digits, `,`, `;`, `<` and `=` are
/// admitted too — rows for each, none of which the issue cited.
fn admitted_keys() -> Vec<(&'static str, String)> {
    vec![
        ("lowercase letters", "team".to_string()),
        ("uppercase letters", "TEAM".to_string()),
        ("digits, admitted by the +..= range", "key0".to_string()),
        ("underscore, listed in the class", "owning_team".to_string()),
        (
            "the punctuation the class lists one by one",
            "ns:sub/part.v-1".to_string(),
        ),
        ("plus, the low end of the range", "a+b".to_string()),
        ("comma, inside the range", "a,b".to_string()),
        ("semicolon, inside the range", "a;b".to_string()),
        ("less-than, inside the range", "a<b".to_string()),
        ("equals, the high end of the range", "a=b".to_string()),
        (
            "the 1-character lower length bound",
            "k".repeat(TAG_KEY_MIN_LENGTH),
        ),
        (
            "the 128-character upper length bound",
            "k".repeat(TAG_KEY_MAX_LENGTH),
        ),
        // The lookahead refuses the prefix `aws:`, not the letters before it: a key is
        // reserved only when the colon is there too.
        ("the reserved prefix without its colon", "aws".to_string()),
        ("a longer word starting with aws", "awsome".to_string()),
        (
            "the reserved prefix in another case, which the pattern spells in one case",
            "AWS:owner".to_string(),
        ),
    ]
}

/// RULE (the admitting side): a key the modelled `TagKey` admits is accepted by every
/// write path and reads back verbatim — so the check added for the illegal keys refuses
/// nothing the model allows.
#[tokio::test]
async fn every_tag_key_the_model_admits_is_accepted_on_every_write_path() {
    let emulator = Emulator::new();

    for (index, (case, key)) in admitted_keys().into_iter().enumerate() {
        // Through CreateDatastore, which installs the map as the store is minted.
        let created = emulator
            .create_datastore(json!({
                "datastoreName": format!("admit.create.{index}"),
                "clientToken": format!("admit-create-{index}"),
                "tags": one_tag(&key),
            }))
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?} via CreateDatastore: expected HTTP 200, got {created:?}"
        );
        let created_id = created.string(&["datastoreId"]);
        let created_arn = datastore_arn(&emulator, &created_id).await;
        let listed = list_tags(&emulator, &created_arn).await;
        assert_eq!(
            listed.field(&["tags"]),
            &one_tag(&key),
            "case {case:?} via CreateDatastore: expected the key to read back verbatim, got {listed:?}"
        );

        // And through TagResource, onto a store created with no tags at all.
        let bare = emulator
            .create_datastore(json!({
                "datastoreName": format!("admit.tag.{index}"),
                "clientToken": format!("admit-tag-{index}"),
            }))
            .await;
        assert_eq!(
            bare.status, 200,
            "case {case:?}: expected the untagged create to answer 200, got {bare:?}"
        );
        let bare_arn = datastore_arn(&emulator, &bare.string(&["datastoreId"])).await;
        let tagged = tag(&emulator, &bare_arn, one_tag(&key)).await;
        assert_eq!(
            tagged.status, 200,
            "case {case:?} via TagResource: expected HTTP 200, got {tagged:?}"
        );
        let read_back = list_tags(&emulator, &bare_arn).await;
        assert_eq!(
            read_back.field(&["tags"]),
            &one_tag(&key),
            "case {case:?} via TagResource: expected the key to read back verbatim, got {read_back:?}"
        );

        // And it can be removed again by name, so UntagResource admits it too.
        let removed = untag(&emulator, &bare_arn, &[key.as_str()]).await;
        assert_eq!(
            removed.status, 200,
            "case {case:?} via UntagResource: expected HTTP 200, got {removed:?}"
        );
        let after = list_tags(&emulator, &bare_arn).await;
        assert_eq!(
            after.field(&["tags"]),
            &json!({}),
            "case {case:?} via UntagResource: expected an empty map, got {after:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The keys the model refuses
// ---------------------------------------------------------------------------

/// One illegal key per clause of `TagKey` a key can break: the reserved prefix the
/// pattern's negative lookahead excludes, a character outside the class, and each
/// `@length` bound. Only the first row is the one the issue cited.
fn rejected_keys() -> Vec<(&'static str, String)> {
    vec![
        (
            "the reserved aws: prefix",
            format!("{TAG_KEY_RESERVED_PREFIX}owner"),
        ),
        (
            "the reserved prefix alone",
            TAG_KEY_RESERVED_PREFIX.to_string(),
        ),
        (
            "the reserved prefix followed by more colon-separated parts",
            format!("{TAG_KEY_RESERVED_PREFIX}cloudformation:stack-name"),
        ),
        ("a space, outside the class", "owning team".to_string()),
        ("an exclamation mark, below the range", "team!".to_string()),
        ("a hash, below the range", "team#1".to_string()),
        ("a dollar sign, below the range", "team$".to_string()),
        ("a percent sign, below the range", "team%".to_string()),
        ("an asterisk, below the range", "team*".to_string()),
        ("a parenthesis, below the range", "(team)".to_string()),
        ("a question mark, just past the range", "team?".to_string()),
        ("an at sign, just past the range", "team@acme".to_string()),
        ("a backslash, past the range", "team\\acme".to_string()),
        ("a brace, past the range", "{team}".to_string()),
        ("a tab, a control character", "team\tone".to_string()),
        ("a non-ASCII letter", "équipe".to_string()),
        (
            "one character past the 128-character upper length bound",
            "k".repeat(TAG_KEY_MAX_LENGTH + 1),
        ),
    ]
}

/// RULE: a tag key outside `TagKey`'s modelled constraints is refused with HTTP 400
/// `ValidationException` by `CreateDatastore` — and the refused call mints no data store
/// and consumes no `clientToken`, so the illegal key is observable nowhere.
#[tokio::test]
async fn an_illegal_tag_key_is_refused_by_create_datastore_and_creates_nothing() {
    let emulator = Emulator::new();

    for (index, (case, key)) in rejected_keys().into_iter().enumerate() {
        // Read the count fresh each row: the token check below creates one store per
        // row, so what the refusal must leave unchanged is the count as it stands now.
        let before = datastore_count(&emulator).await;
        let token = format!("reject-create-{index}");
        let refused = emulator
            .create_datastore(json!({
                "datastoreName": format!("reject.create.{index}"),
                "clientToken": token.clone(),
                "tags": one_tag(&key),
            }))
            .await;
        assert_refused(case, &refused);
        assert!(
            refused.field(&["datastoreId"]).is_null(),
            "case {case:?}: expected no datastoreId in a refusal, got {refused:?}"
        );
        assert_eq!(
            datastore_count(&emulator).await,
            before,
            "case {case:?}: expected the refusal to mint no data store"
        );

        // The refusal consumed nothing: the same token still creates a store when the
        // request it carries is legal.
        let accepted = emulator
            .create_datastore(json!({
                "datastoreName": format!("reject.create.{index}"),
                "clientToken": token,
                "tags": one_tag("team"),
            }))
            .await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: expected the same token to be free after the refusal, got {accepted:?}"
        );
        let arn = datastore_arn(&emulator, &accepted.string(&["datastoreId"])).await;
        let listed = list_tags(&emulator, &arn).await;
        assert_eq!(
            listed.field(&["tags"]),
            &one_tag("team"),
            "case {case:?}: expected only the legal tag, got {listed:?}"
        );
    }
}

/// RULE (the same rule, the other write path): `TagResource` refuses exactly the keys
/// `CreateDatastore` refuses, and the resource's tag map is unchanged by the refusal.
/// `UntagResource` refuses them too: a key it is asked to remove is a `TagKey` as well.
#[tokio::test]
async fn an_illegal_tag_key_is_refused_by_the_tagging_routes_and_changes_nothing() {
    let emulator = Emulator::new();
    let datastore_id = emulator
        .active_datastore("reject.tag", "reject-tag-token")
        .await;
    let arn = datastore_arn(&emulator, &datastore_id).await;

    let held = vec![("team".to_string(), "radiology".to_string())];
    let seeded = tag(&emulator, &arn, tag_object(&held)).await;
    assert_eq!(seeded.status, 200, "seeding one legal tag: got {seeded:?}");

    for (case, key) in rejected_keys() {
        let refused = tag(&emulator, &arn, one_tag(&key)).await;
        assert_refused(case, &refused);

        // An illegal key does not become legal by travelling beside a legal one.
        let mut mixed = serde_json::Map::new();
        mixed.insert("legal".to_string(), json!("v"));
        mixed.insert(key.clone(), json!("v"));
        let mixed_refused = tag(&emulator, &arn, Value::Object(mixed)).await;
        assert_refused(case, &mixed_refused);

        let removed = untag(&emulator, &arn, &[key.as_str()]).await;
        assert_refused(case, &removed);

        let listed = list_tags(&emulator, &arn).await;
        assert_eq!(
            listed.field(&["tags"]),
            &tag_object(&held),
            "case {case:?}: expected the refusals to leave the tag map untouched, got {listed:?}"
        );
    }
}

/// RULE (the lower length bound of the same rule): the empty string is not a `TagKey` —
/// `@length`'s minimum is 1, which the pattern's `+` quantifier says too — so a map
/// carrying it is refused on both write paths.
///
/// Its own case because an empty key cannot be carried on `UntagResource`'s
/// `tagKeys` QUERY parameter at all: an empty query value names no key, so that route
/// has no row to drive here.
#[tokio::test]
async fn the_empty_string_is_not_a_tag_key() {
    let emulator = Emulator::new();
    let before = datastore_count(&emulator).await;

    let refused = emulator
        .create_datastore(json!({
            "datastoreName": "empty.key",
            "clientToken": "empty-key-token",
            "tags": one_tag(""),
        }))
        .await;
    assert_refused("the empty key, one below the length minimum", &refused);
    assert_eq!(
        datastore_count(&emulator).await,
        before,
        "the empty key: expected the refusal to mint no data store"
    );

    let datastore_id = emulator
        .active_datastore("empty.key.tag", "empty-key-tag-token")
        .await;
    let arn = datastore_arn(&emulator, &datastore_id).await;
    let on_tag_path = tag(&emulator, &arn, one_tag("")).await;
    assert_refused("the empty key on the tag path", &on_tag_path);
    let listed = list_tags(&emulator, &arn).await;
    assert_eq!(
        listed.field(&["tags"]),
        &json!({}),
        "the empty key: expected the refusal to leave an empty map, got {listed:?}"
    );
}

/// RULE: the refusal states the constraint the key failed, in this service's one
/// validation envelope, with every `@pattern` quoted WITHOUT the anchors the model
/// spells it with (`crate::state`'s wording rule 3).
#[tokio::test]
async fn the_refusal_quotes_the_modelled_tag_key_pattern() {
    let emulator = Emulator::new();
    let refused = emulator
        .create_datastore(json!({
            "datastoreName": "pattern.wording",
            "clientToken": "pattern-wording",
            "tags": one_tag(&format!("{TAG_KEY_RESERVED_PREFIX}owner")),
        }))
        .await;
    assert_refused("the reserved aws: prefix", &refused);

    let message = refused.string(&["message"]);
    let unanchored = TAG_KEY_PATTERN
        .trim_start_matches('^')
        .trim_end_matches('$');
    assert!(
        message.contains(unanchored),
        "expected the message to quote the modelled pattern {unanchored:?}, got {message:?}"
    );
    assert!(
        !message.contains(TAG_KEY_PATTERN),
        "expected the pattern quoted without its anchors, got {message:?}"
    );
    assert!(
        message.contains("1 validation error detected"),
        "expected this service's validation envelope, got {message:?}"
    );
}

// ---------------------------------------------------------------------------
// How many tags one call may install
// ---------------------------------------------------------------------------

/// RULE: no request may leave one resource holding more than `MAX_TAGS_PER_RESOURCE`
/// tags — the bound is the RESOURCE's, so the create path enforces the same ceiling the
/// tag path does. Rows: the empty map, one tag, the bound itself, one past it, and far
/// past it.
#[tokio::test]
async fn no_write_path_installs_more_tags_than_a_resource_holds() {
    let emulator = Emulator::new();

    let rows: Vec<(String, usize, bool)> = vec![
        ("the empty map".to_string(), 0, true),
        ("one tag".to_string(), 1, true),
        (
            format!("{MAX_TAGS_PER_RESOURCE} tags, the bound itself"),
            MAX_TAGS_PER_RESOURCE,
            true,
        ),
        (
            format!("{} tags, one past the bound", MAX_TAGS_PER_RESOURCE + 1),
            MAX_TAGS_PER_RESOURCE + 1,
            false,
        ),
        (
            format!("{} tags, far past the bound", MAX_TAGS_PER_RESOURCE * 4),
            MAX_TAGS_PER_RESOURCE * 4,
            false,
        ),
    ];

    for (index, (case, count, admitted)) in rows.into_iter().enumerate() {
        let tags = distinct_tags(count);
        let before = datastore_count(&emulator).await;
        let created = emulator
            .create_datastore(json!({
                "datastoreName": format!("count.{index}"),
                "clientToken": format!("count-token-{index}"),
                "tags": tag_object(&tags),
            }))
            .await;

        if admitted {
            assert_eq!(
                created.status, 200,
                "case {case:?}: expected HTTP 200, got {created:?}"
            );
            let arn = datastore_arn(&emulator, &created.string(&["datastoreId"])).await;
            let listed = list_tags(&emulator, &arn).await;
            assert_eq!(
                listed.field(&["tags"]),
                &tag_object(&tags),
                "case {case:?}: expected every tag the create carried, got {listed:?}"
            );
        } else {
            assert_refused(&case, &created);
            assert_eq!(
                datastore_count(&emulator).await,
                before,
                "case {case:?}: expected the refusal to mint no data store"
            );

            // The same oversized map is refused on the tag path too, which is the
            // ceiling being the resource's rather than the operation's.
            let bare = emulator
                .active_datastore(&format!("count.bare.{index}"), &format!("count-bare-{index}"))
                .await;
            let bare_arn = datastore_arn(&emulator, &bare).await;
            let over = tag(&emulator, &bare_arn, tag_object(&tags)).await;
            assert_refused(&case, &over);
            let listed = list_tags(&emulator, &bare_arn).await;
            assert_eq!(
                listed.field(&["tags"]),
                &json!({}),
                "case {case:?}: expected the refused tag call to leave an empty map, got {listed:?}"
            );
        }
    }
}
