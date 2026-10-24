// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the tag-key namespace AWS keeps for itself, driven over the
//! wire through the shared [`crate::emulator`] harness.
//!
//! RULE under test: a caller may not create, change or remove a tag whose key begins
//! with the prefix AWS reserves for its own system tags (matched without regard to
//! case), so every tagging call that names such a key — tag-on-create,
//! `TagResource` and `UntagResource` alike — is refused with the service's modelled
//! validation error (HTTP 400) and writes nothing, while a key that merely resembles
//! the prefix without starting with it is an ordinary caller tag.
//!
//! Every assertion is about that rule over a TABLE of inputs whose rows differ in the
//! CASE of the prefix (every combination of upper and lower case the three letters
//! allow), the rest of the key (nothing at all, one segment, several segments, the
//! longest key `TagKey` allows), how many other entries share the request, and which
//! writer the request goes through — plus a second table of near misses that differ
//! from the prefix in the separator, the position of the prefix inside the key and
//! the encoding of the opening character. `TagKey` is modelled with a length only
//! (`smithy_query(query="shape",shape="TagKey")` gives `@length(min: 1, max: 128)`
//! and no `@pattern`), so the reservation is a service rule and the refusal is the
//! modelled `InterconnectValidationException` the operations declare
//! (`smithy_query(query="operation",operation="TagResource")`, HTTP 400).

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use crate::emulator::{code, create_with_tags, message, placements, tags_of, text, Emulator};

/// `TagKey` is modelled `@length(min: 1, max: 128)`.
const TAG_KEY_MAX: usize = 128;

/// The error the tagging operations model for a fault in their input
/// (`InterconnectValidationException`, HTTP 400).
const VALIDATION_ERROR: &str = "InterconnectValidationException";

// ---------------------------------------------------------------------------
// Calls under test
// ---------------------------------------------------------------------------

/// `TagResource` for `arn`, writing `tags`.
async fn tag(emulator: &Emulator, arn: &str, tags: &BTreeMap<String, String>) -> (u16, Value) {
    emulator
        .ic("TagResource", json!({ "arn": arn, "tags": tags }))
        .await
}

/// `UntagResource` for `arn`, removing `keys`.
async fn untag(emulator: &Emulator, arn: &str, keys: &[String]) -> (u16, Value) {
    emulator
        .ic("UntagResource", json!({ "arn": arn, "tagKeys": keys }))
        .await
}

/// `CreateConnection` on `placement` carrying `tags`, as a `(status, body)` pair so
/// the tag-on-create refusal is readable. `seed` keeps each row's attach point
/// distinct.
async fn create(
    emulator: &Emulator,
    placement: &(String, String),
    seed: usize,
    tags: &BTreeMap<String, String>,
) -> (u16, Value) {
    let mut request = Map::new();
    request.insert("environmentId".to_string(), json!(placement.0));
    request.insert("bandwidth".to_string(), json!(placement.1));
    request.insert(
        "attachPoint".to_string(),
        json!({ "directConnectGateway": format!("aaaaaaaa-bbbb-4ccc-8ddd-{seed:012}") }),
    );
    request.insert(
        "description".to_string(),
        json!(format!("reserved tag case {seed}")),
    );
    request.insert("tags".to_string(), json!(tags));
    emulator.ic("CreateConnection", Value::Object(request)).await
}

/// How many connections the store holds, so a refused create can be shown to have
/// allocated nothing.
async fn connection_count(emulator: &Emulator) -> usize {
    let listed = emulator.ic_ok("ListConnections", json!({})).await;
    listed["connections"]
        .as_array()
        .map(Vec::len)
        .unwrap_or_default()
}

/// The tag set `ListTagsForResource` reports for `arn` — the other reader of the same
/// stored set, so "nothing was written" is proved through both readers.
async fn listed_tags(emulator: &Emulator, arn: &str, case: &str) -> BTreeMap<String, String> {
    let (status, body) = emulator
        .ic("ListTagsForResource", json!({ "arn": arn }))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from ListTagsForResource, got {status} with {body:?}"
    );
    tags_of(&body)
}

/// A `(key, value)` table as the map the wire carries.
fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

// ---------------------------------------------------------------------------
// The tables
// ---------------------------------------------------------------------------

/// The keys this suite expects every tag writer to refuse.
///
/// Rows differ in the case of the reserved prefix (all eight combinations its three
/// letters allow, generated rather than listed so none is missed), in what follows
/// the prefix (nothing, one segment, several segments, a segment the issue did not
/// cite, the longest key the modelled length allows) and in the characters after it.
fn reserved_keys() -> Vec<(String, String)> {
    let mut rows = Vec::new();
    // Every casing of the prefix: bit `i` of `mask` upper-cases letter `i`.
    let letters = ['a', 'w', 's'];
    for mask in 0u8..8 {
        let prefix: String = letters
            .iter()
            .enumerate()
            .map(|(index, letter)| {
                if mask & (1 << index) == 0 {
                    *letter
                } else {
                    letter.to_ascii_uppercase()
                }
            })
            .collect();
        rows.push((
            format!("the prefix cased {prefix:?} with one segment"),
            format!("{prefix}:system-tag"),
        ));
    }
    rows.extend([
        (
            "the bare prefix and nothing else".to_string(),
            "aws:".to_string(),
        ),
        (
            "several segments".to_string(),
            "aws:cloudformation:stack-name".to_string(),
        ),
        (
            // A system-tag namespace the issue did not cite.
            "a namespace the report did not name".to_string(),
            "aws:servicecatalog:provisioningPrincipalArn".to_string(),
        ),
        (
            "a space after the prefix".to_string(),
            "aws: spaced".to_string(),
        ),
        (
            "non-ASCII after the prefix".to_string(),
            "aws:日本語".to_string(),
        ),
        (
            "the longest key the modelled length allows".to_string(),
            format!("aws:{}", "k".repeat(TAG_KEY_MAX - 4)),
        ),
    ]);
    rows
}

/// Keys that resemble the reserved prefix without being in it, so they are ordinary
/// caller tags: rows differ in the separator, in where the prefix sits inside the
/// key, and in the encoding of the opening character (a multi-byte one, so the
/// prefix test cannot be a byte slice).
fn near_miss_keys() -> Vec<(&'static str, &'static str)> {
    vec![
        ("the letters without the separator", "aws"),
        ("the letters as a word", "awsome"),
        ("a hyphen instead of the separator", "aws-cloudformation"),
        ("an underscore instead of the separator", "aws_system"),
        ("no separator and more letters", "AWSBillingTag"),
        ("the prefix later in the key", "my-aws:thing"),
        ("the prefix after a space", " aws:thing"),
        ("the separator before the letters", "a:ws"),
        ("a multi-byte opening character", "日本語:aws:"),
        ("a multi-byte key", "ключ-日本語"),
    ]
}

// ---------------------------------------------------------------------------
// The rule
// ---------------------------------------------------------------------------

/// RULE: `TagResource` refuses a request carrying a key in the reserved namespace
/// with the modelled validation error (HTTP 400) and leaves the resource's tag set
/// exactly as it was — including the request's other, perfectly legal entries, since
/// a refused write writes nothing.
#[tokio::test]
async fn tag_resource_refuses_a_reserved_tag_key_and_writes_nothing() {
    let emulator = Emulator::new();
    let placements = placements(&emulator).await;
    let initial = map(&[("env", "test"), ("team", "network")]);
    let connection = create_with_tags(&emulator, &placements[0], 900, &initial).await;
    let arn = text(&connection, "arn");
    let id = text(&connection, "id");

    let rows = reserved_keys();
    assert!(
        rows.len() >= 8,
        "the reserved-key table must span the class of reserved keys, got {} rows",
        rows.len()
    );

    for (seed, (name, key)) in rows.iter().enumerate() {
        // Rows alternate between a request carrying ONLY the reserved key and one
        // carrying it beside legal entries, so the refusal is shown not to depend on
        // the request being nothing but the reserved key.
        let mut written: BTreeMap<String, String> =
            [(key.clone(), "mystack".to_string())].into_iter().collect();
        if seed.is_multiple_of(2) {
            written.insert("owner".to_string(), "ops".to_string());
            written.insert(format!("legal-{seed}"), "yes".to_string());
        }

        let (status, body) = tag(&emulator, &arn, &written).await;
        assert_eq!(
            status, 400,
            "case {name:?} (key {key:?}): expected HTTP 400 for a reserved tag key, got {status} with {body:?}"
        );
        assert!(
            code(&body).contains(VALIDATION_ERROR),
            "case {name:?} (key {key:?}): expected {VALIDATION_ERROR}, got {body:?}"
        );
        assert!(
            message(&body).contains(key.as_str()),
            "case {name:?}: expected the refusal to name the reserved key, got {:?}",
            message(&body)
        );

        // Nothing moved: neither the reserved key nor the legal entries that shared
        // the refused request.
        let stored = tags_of(&emulator.get_connection(&id).await.1["connection"]);
        assert_eq!(
            stored, initial,
            "case {name:?} (key {key:?}): expected the tag set untouched by a refused write, got {stored:?}"
        );
        let reported = listed_tags(&emulator, &arn, name).await;
        assert_eq!(
            reported, initial,
            "case {name:?} (key {key:?}): expected ListTagsForResource to report the untouched set, got {reported:?}"
        );
    }
}

/// RULE: the same reservation holds on tag-on-create — `CreateConnection` carrying a
/// reserved key is refused with the modelled validation error (HTTP 400) and
/// allocates no connection.
#[tokio::test]
async fn create_connection_refuses_a_reserved_tag_key_and_allocates_nothing() {
    let emulator = Emulator::new();
    let placements = placements(&emulator).await;
    let before = connection_count(&emulator).await;

    for (seed, (name, key)) in reserved_keys().into_iter().enumerate() {
        let placement = &placements[seed % placements.len()];
        let tags: BTreeMap<String, String> = [
            (key.clone(), "mystack".to_string()),
            ("env".to_string(), "test".to_string()),
        ]
        .into_iter()
        .collect();

        let (status, body) = create(&emulator, placement, 9_000 + seed, &tags).await;
        assert_eq!(
            status, 400,
            "case {name:?} (key {key:?}): expected HTTP 400 for a reserved tag key on create, got {status} with {body:?}"
        );
        assert!(
            code(&body).contains(VALIDATION_ERROR),
            "case {name:?} (key {key:?}): expected {VALIDATION_ERROR}, got {body:?}"
        );
        assert!(
            body["connection"].is_null(),
            "case {name:?} (key {key:?}): expected a refusal to carry no connection, got {body:?}"
        );
    }

    let after = connection_count(&emulator).await;
    assert_eq!(
        after, before,
        "case \"after the reserved-key create table\": expected no connection to have been allocated, got {after} where there were {before}"
    );
}

/// RULE: a reserved key is not the caller's to REMOVE either — `UntagResource`
/// naming one is refused with the modelled validation error (HTTP 400) and removes
/// nothing, including the legal keys that shared the request.
#[tokio::test]
async fn untag_resource_refuses_a_reserved_tag_key_and_removes_nothing() {
    let emulator = Emulator::new();
    let placements = placements(&emulator).await;
    let initial = map(&[("env", "test"), ("team", "network"), ("owner", "ops")]);
    let connection = create_with_tags(&emulator, &placements[0], 910, &initial).await;
    let arn = text(&connection, "arn");
    let id = text(&connection, "id");

    for (seed, (name, key)) in reserved_keys().into_iter().enumerate() {
        // Rows alternate between naming only the reserved key and naming it beside a
        // key the resource really holds, which a refused call must keep.
        let mut keys = vec![key.clone()];
        if seed.is_multiple_of(2) {
            keys.push("env".to_string());
        }

        let (status, body) = untag(&emulator, &arn, &keys).await;
        assert_eq!(
            status, 400,
            "case {name:?} (keys {keys:?}): expected HTTP 400 for a reserved tag key, got {status} with {body:?}"
        );
        assert!(
            code(&body).contains(VALIDATION_ERROR),
            "case {name:?} (keys {keys:?}): expected {VALIDATION_ERROR}, got {body:?}"
        );
        assert!(
            message(&body).contains(key.as_str()),
            "case {name:?}: expected the refusal to name the reserved key, got {:?}",
            message(&body)
        );

        let stored = tags_of(&emulator.get_connection(&id).await.1["connection"]);
        assert_eq!(
            stored, initial,
            "case {name:?} (keys {keys:?}): expected the tag set untouched by a refused removal, got {stored:?}"
        );
    }
}

/// RULE, the other side of it: a key that only RESEMBLES the reserved prefix is an
/// ordinary caller tag, so it is created, read back and removed like any other —
/// the reservation is a prefix rule, not a ban on the letters.
#[tokio::test]
async fn keys_outside_the_reserved_namespace_stay_writable() {
    let emulator = Emulator::new();
    let placements = placements(&emulator).await;

    let near_misses = near_miss_keys();
    assert!(
        near_misses.len() >= 5,
        "the near-miss table must span the ways a key can resemble the prefix, got {} rows",
        near_misses.len()
    );

    // Tag-on-create accepts them all at once, so the created record is the first
    // proof the reservation is not over-broad.
    let on_create: BTreeMap<String, String> = near_misses
        .iter()
        .map(|(_, key)| (key.to_string(), "created".to_string()))
        .collect();
    let connection = create_with_tags(&emulator, &placements[0], 920, &on_create).await;
    let arn = text(&connection, "arn");
    let id = text(&connection, "id");
    assert_eq!(
        tags_of(&connection),
        on_create,
        "case \"tag-on-create with near-miss keys\": expected every key to be stored, got {:?}",
        connection["tags"]
    );

    for (name, key) in &near_misses {
        // Written again through TagResource with a new value: the merge rule, on a
        // key that resembles the reserved prefix.
        let written: BTreeMap<String, String> =
            [(key.to_string(), "tagged".to_string())].into_iter().collect();
        let (status, body) = tag(&emulator, &arn, &written).await;
        assert_eq!(
            status, 200,
            "case {name:?} (key {key:?}): expected an ordinary key to be accepted, got {status} with {body:?}"
        );
        let stored = tags_of(&emulator.get_connection(&id).await.1["connection"]);
        assert_eq!(
            stored.get(*key).map(String::as_str),
            Some("tagged"),
            "case {name:?} (key {key:?}): expected the written value to be stored, got {stored:?}"
        );

        // And removed again: an ordinary key stays the caller's to delete.
        let (status, body) = untag(&emulator, &arn, &[key.to_string()]).await;
        assert_eq!(
            status, 200,
            "case {name:?} (key {key:?}): expected an ordinary key to be removable, got {status} with {body:?}"
        );
        let stored = tags_of(&emulator.get_connection(&id).await.1["connection"]);
        assert!(
            !stored.contains_key(*key),
            "case {name:?} (key {key:?}): expected the key to be gone, got {stored:?}"
        );
    }

    let left = listed_tags(&emulator, &arn, "after every near-miss key is removed").await;
    assert!(
        left.is_empty(),
        "case \"after every near-miss key is removed\": expected no tags left, got {left:?}"
    );
}
