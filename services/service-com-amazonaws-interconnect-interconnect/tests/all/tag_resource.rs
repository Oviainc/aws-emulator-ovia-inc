// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for `TagResource`, driven over the wire through the shared
//! [`crate::emulator`] harness.
//!
//! RULE under test: `TagResource` writes into the tag set of the resource its `arn`
//! names every key the request carries with the value the request gives it — a key
//! the resource already holds takes the new value and every key the request does not
//! name keeps the value it had — so the call is a MERGE, is idempotent, and changes
//! nothing else about the resource. An `arn` that names no resource of this service
//! is refused with HTTP 400 — the client fault for an in-scope ARN naming nothing,
//! the validation fault for one outside the scope the request addresses — and one
//! that is not a connection ARN at all is
//! refused as invalid input (HTTP 400), never with a 501. A resource's tag set is
//! also bounded: a write that would leave it holding more tags than one resource
//! holds is refused and changes nothing, on this operation and on tag-on-create
//! alike.
//!
//! Every assertion is about that rule over a TABLE of inputs whose rows differ in
//! the number of tags already on the resource and the number written (0, 1, 2, 3 and
//! 20), whether a written key is new or already present, the form of the keys
//! (plain, spaced, dotted, namespaced, non-ASCII), the length of the keys and values
//! (each modelled bound), and the environment and bandwidth the tagged connection
//! sits on. The lengths come from the model:
//! `smithy_query(query="shape",shape="TagKey,TagValue")` gives `TagKey`
//! `@length(min: 1, max: 128)` and `TagValue` `@length(min: 0, max: 256)`.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{json, Value};

use crate::emulator::{
    code, create_with_tags, message, placements, tags_of, text, without_tags, Emulator,
};

/// The account id every emulator ARN carries.
const ACCOUNT_ID: &str = engine::EMULATOR_ACCOUNT_ID;

/// The region the shared harness signs its requests for, so the region every ARN
/// it mints carries.
const REGION: &str = "us-east-1";

/// Long enough to cover the provisioning span `state.rs` documents (400 ms) with
/// margin; the transition is due at a stored instant, so any wait past it settles.
const SETTLE_WAIT: Duration = Duration::from_millis(700);

/// `TagKey` is modelled `@length(min: 1, max: 128)`.
const TAG_KEY_MAX: usize = 128;
/// `TagValue` is modelled `@length(min: 0, max: 256)`.
const TAG_VALUE_MAX: usize = 256;

// ---------------------------------------------------------------------------
// Calls under test
// ---------------------------------------------------------------------------

/// `TagResource` for `arn`, writing `tags`, as a `(status, body)` pair so the
/// refusals are readable.
async fn tag(emulator: &Emulator, arn: &str, tags: &BTreeMap<String, String>) -> (u16, Value) {
    emulator
        .ic("TagResource", json!({ "arn": arn, "tags": tags }))
        .await
}

/// `ListTagsForResource` for `arn`, required to succeed, as its reported tag set —
/// the other reader of the same stored tag set, so the write is proved through both.
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
// The table
// ---------------------------------------------------------------------------

/// One row: the tags the connection is created with, and the tags the call writes.
#[derive(Debug, Clone)]
struct TagCase {
    name: String,
    initial: BTreeMap<String, String>,
    written: BTreeMap<String, String>,
}

impl TagCase {
    fn new(name: &str, initial: &[(&str, &str)], written: &[(&str, &str)]) -> Self {
        Self {
            name: name.to_string(),
            initial: map(initial),
            written: map(written),
        }
    }

    /// What the rule says is left: the initial set with the written keys applied
    /// over it, every unnamed key keeping its value.
    fn expected_tags(&self) -> BTreeMap<String, String> {
        let mut expected = self.initial.clone();
        expected.extend(self.written.iter().map(|(k, v)| (k.clone(), v.clone())));
        expected
    }
}

/// The table: rows differ in how many tags the resource starts with (0, 1, 2, 3, 20)
/// and how many are written (0, 1, 2, 3, 20), whether a written key is new, already
/// present with another value, or already present with the SAME value, the key form
/// (plain, spaced, dotted, namespaced, non-ASCII), and each modelled length bound
/// for a key and for a value.
fn tag_table() -> Vec<TagCase> {
    let longest_key = "k".repeat(TAG_KEY_MAX);
    let longest_value = "v".repeat(TAG_VALUE_MAX);
    let mut table = vec![
        TagCase::new("the first tag on an untagged resource", &[], &[("env", "test")]),
        TagCase::new("three tags on an untagged resource", &[], &[
            ("env", "test"),
            ("team", "network"),
            ("owner", "ops"),
        ]),
        TagCase::new(
            "a new key beside an existing one",
            &[("env", "test")],
            &[("team", "network")],
        ),
        TagCase::new(
            "a new value for an existing key",
            &[("env", "test"), ("team", "network")],
            &[("env", "production")],
        ),
        TagCase::new(
            "the same value for an existing key",
            &[("env", "test"), ("team", "network")],
            &[("env", "test")],
        ),
        TagCase::new(
            "one new key and one overwritten",
            &[("env", "test"), ("team", "network")],
            &[("env", "staging"), ("owner", "ops")],
        ),
        TagCase::new(
            "every existing key overwritten",
            &[("env", "test"), ("team", "network"), ("owner", "ops")],
            &[("env", "prod"), ("team", "core"), ("owner", "sre")],
        ),
        TagCase::new("no tag at all", &[("env", "test")], &[]),
        TagCase::new("no tag at all on an untagged resource", &[], &[]),
        TagCase::new(
            "keys differing in form",
            &[("env", "test")],
            &[
                ("Key With Spaces", "1"),
                ("key.with.dots", "2"),
                ("namespace:key/part", "3"),
                ("ключ-日本語", "4"),
            ],
        ),
        // Each modelled bound: the shortest and the longest key `TagKey` allows,
        // and the shortest (empty) and the longest value `TagValue` allows.
        TagCase::new(
            "the shortest and the longest key",
            &[("env", "test")],
            &[("k", "shortest key"), (longest_key.as_str(), "longest key")],
        ),
        TagCase::new(
            "the shortest and the longest value",
            &[("env", "test")],
            &[("empty", ""), ("longest", longest_value.as_str())],
        ),
        TagCase::new(
            "an existing value replaced by the empty one",
            &[("env", "test")],
            &[("env", "")],
        ),
        TagCase::new(
            "an existing value replaced by the longest one",
            &[("env", "test")],
            &[("env", longest_value.as_str())],
        ),
    ];

    // Twenty tags written at once over twenty existing ones, half of them already
    // present: the same rule at a size no row above reaches.
    let initial: BTreeMap<String, String> = (0..20)
        .map(|index| (format!("key-{index:02}"), format!("value-{index:02}")))
        .collect();
    let written: BTreeMap<String, String> = (10..30)
        .map(|index| (format!("key-{index:02}"), format!("rewritten-{index:02}")))
        .collect();
    table.push(TagCase {
        name: "twenty tags over twenty, half of them new".to_string(),
        initial,
        written,
    });

    table
}

// ---------------------------------------------------------------------------
// The rule
// ---------------------------------------------------------------------------

/// RULE: `TagResource` merges the requested tags into the resource's persisted tag
/// set — each requested key takes the requested value, every other key keeps the
/// value it had — changes nothing else about the resource, and is idempotent. The
/// stored record is what `GetConnection` and `ListTagsForResource` both read back.
#[tokio::test]
async fn tag_resource_merges_the_requested_tags_and_keeps_the_rest() {
    let emulator = Emulator::new();
    let placements = placements(&emulator).await;
    let table = tag_table();
    assert!(
        table.len() >= 8,
        "the tag table must span the class of tag writes, got {} rows",
        table.len()
    );

    for (seed, case) in table.into_iter().enumerate() {
        let placement = &placements[seed % placements.len()];
        let before = create_with_tags(&emulator, placement, 500 + seed, &case.initial).await;
        let arn = text(&before, "arn");
        let id = text(&before, "id");
        assert_eq!(
            tags_of(&before),
            case.initial,
            "case {:?}: expected the created connection to carry the supplied tags, got {:?}",
            case.name,
            before["tags"]
        );

        let (status, body) = tag(&emulator, &arn, &case.written).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from TagResource, got {status} with {body:?}",
            case.name
        );
        assert!(
            body.is_null() || body == json!({}),
            "case {:?}: TagResourceResponse models no members, so expected an empty body, got {body:?}",
            case.name
        );

        // The persisted record is what a reader sees, through both readers of it.
        let (get_status, fetched) = emulator.get_connection(&id).await;
        assert_eq!(
            get_status, 200,
            "case {:?}: expected the tagged connection to still be readable, got {get_status} with {fetched:?}",
            case.name
        );
        let after = fetched["connection"].clone();
        assert_eq!(
            tags_of(&after),
            case.expected_tags(),
            "case {:?}: expected the initial tags with {:?} applied over them, got {:?}",
            case.name,
            case.written,
            after["tags"]
        );
        assert_eq!(
            listed_tags(&emulator, &arn, &case.name).await,
            case.expected_tags(),
            "case {:?}: expected ListTagsForResource to report the same merged set as GetConnection, got {:?}",
            case.name,
            after["tags"]
        );
        assert_eq!(
            without_tags(&after),
            without_tags(&before),
            "case {:?}: expected only the tag set to change, got {after:?}",
            case.name
        );
        // A write of nothing onto a resource with nothing leaves "no tags" as it
        // was, rather than inventing an empty map's worth of state.
        if case.expected_tags().is_empty() {
            assert!(
                after["tags"].is_null() || after["tags"] == json!({}),
                "case {:?}: expected no tags when none were ever written, got {:?}",
                case.name,
                after["tags"]
            );
        }

        // Idempotent: the same call again succeeds and changes nothing.
        let (repeat_status, repeat_body) = tag(&emulator, &arn, &case.written).await;
        assert_eq!(
            repeat_status, 200,
            "case {:?}: expected a repeated TagResource to succeed, got {repeat_status} with {repeat_body:?}",
            case.name
        );
        let repeated = emulator.get_connection(&id).await.1["connection"].clone();
        assert_eq!(
            repeated, after,
            "case {:?}: expected the repeat to change nothing, got {repeated:?}",
            case.name
        );
    }
}

/// RULE: what a tag may be is bounded by the model — `TagKey` is
/// `@length(min: 1, max: 128)` and `TagValue` `@length(min: 0, max: 256)` — so a key
/// or value at either published bound is accepted and one past a bound is refused on
/// the input (HTTP 400), never with a 501, and leaves the tag set as it was.
#[tokio::test]
async fn tag_resource_holds_tags_to_the_modelled_lengths() {
    let emulator = Emulator::new();
    let placements = placements(&emulator).await;
    let initial = map(&[("env", "test")]);
    let connection = create_with_tags(&emulator, &placements[0], 600, &initial).await;
    let arn = text(&connection, "arn");
    let id = text(&connection, "id");

    for (name, key, value, expected) in [
        ("one-character key, the modelled minimum", "k".to_string(), "fine".to_string(), 200),
        (
            "128-character key, the modelled maximum",
            "k".repeat(TAG_KEY_MAX),
            "fine".to_string(),
            200,
        ),
        (
            "empty value, the modelled minimum",
            "empty-value".to_string(),
            String::new(),
            200,
        ),
        (
            "256-character value, the modelled maximum",
            "longest-value".to_string(),
            "v".repeat(TAG_VALUE_MAX),
            200,
        ),
        (
            "empty key, one below the minimum",
            String::new(),
            "refused".to_string(),
            400,
        ),
        (
            "129-character key, one past the maximum",
            "k".repeat(TAG_KEY_MAX + 1),
            "refused".to_string(),
            400,
        ),
        (
            "257-character value, one past the maximum",
            "too-long-value".to_string(),
            "v".repeat(TAG_VALUE_MAX + 1),
            400,
        ),
    ] {
        let written: BTreeMap<String, String> = [(key.clone(), value.clone())].into_iter().collect();
        let (status, body) = tag(&emulator, &arn, &written).await;
        assert_eq!(
            status,
            expected,
            "case {name:?} (key length {}, value length {}): expected HTTP {expected}, got {status} with {body:?}",
            // The modelled bound is in characters, so the diagnostic counts
            // characters too.
            key.chars().count(),
            value.chars().count()
        );
    }

    // Every accepted write above is on the resource, and no refused one is.
    let stored = tags_of(&emulator.get_connection(&id).await.1["connection"]);
    let mut expected = initial;
    expected.insert("k".to_string(), "fine".to_string());
    expected.insert("k".repeat(TAG_KEY_MAX), "fine".to_string());
    expected.insert("empty-value".to_string(), String::new());
    expected.insert("longest-value".to_string(), "v".repeat(TAG_VALUE_MAX));
    assert_eq!(
        stored, expected,
        "case \"after the bounds table\": expected only the accepted writes to be stored, got {stored:?}"
    );
}

/// RULE: tagging one resource touches no other resource's tags.
#[tokio::test]
async fn tag_resource_touches_only_the_resource_its_arn_names() {
    let emulator = Emulator::new();
    let placements = placements(&emulator).await;

    // Several connections, all carrying the SAME keys, so a handler that wrote a tag
    // globally rather than per resource would be caught.
    let shared = map(&[("env", "test"), ("team", "network")]);
    let mut created = Vec::new();
    for (seed, placement) in placements.iter().enumerate().take(4) {
        created.push(create_with_tags(&emulator, placement, 700 + seed, &shared).await);
    }
    assert!(
        created.len() >= 2,
        "this rule needs at least two resources, got {}",
        created.len()
    );

    let target = created[0].clone();
    let written = map(&[("env", "production"), ("owner", "ops")]);
    let (status, body) = tag(&emulator, &text(&target, "arn"), &written).await;
    assert_eq!(
        status, 200,
        "case \"one of several resources\": expected HTTP 200, got {status} with {body:?}"
    );

    for (index, connection) in created.iter().enumerate() {
        let id = text(connection, "id");
        let fetched = emulator.get_connection(&id).await.1["connection"].clone();
        let expected: BTreeMap<String, String> = if index == 0 {
            let mut merged = shared.clone();
            merged.extend(written.iter().map(|(k, v)| (k.clone(), v.clone())));
            merged
        } else {
            shared.clone()
        };
        assert_eq!(
            tags_of(&fetched),
            expected,
            "case {id:?} (index {index}): expected {expected:?}, got {:?}",
            fetched["tags"]
        );
    }
}

/// RULE: an `arn` that names no resource of this store is refused with HTTP 400 and
/// creates nothing — tagging never brings a resource into existence — with DIFFERENT
/// codes for the two ways it can name none: `InterconnectValidationException` for an
/// ARN outside the partition, Region and account this request addresses, and
/// `InterconnectClientException` for an in-scope ARN that names no connection. An
/// `arn` that is not a connection ARN at all is likewise invalid input (HTTP 400).
/// None of them is ever a 404 or a 501.
#[tokio::test]
async fn tag_resource_refuses_an_arn_that_names_no_connection() {
    let emulator = Emulator::new();
    let placements = placements(&emulator).await;
    let tags = map(&[("env", "test")]);
    let live = create_with_tags(&emulator, &placements[0], 800, &tags).await;
    let live_id = text(&live, "id");
    let live_arn = text(&live, "arn");
    let written = map(&[("team", "network")]);

    // In-scope, well-formed connection ARNs that name nothing this account holds
    // (the client fault), then ARNs outside the scope this request addresses (the
    // validation fault): two negative classes with two codes.
    let elsewhere = vec![
        (
            "an id that was never allocated",
            format!("arn:aws:interconnect:{REGION}:{ACCOUNT_ID}:connection/mcc-00000000"),
            "InterconnectClientException",
        ),
        (
            "the other id prefix, never allocated",
            format!("arn:aws:interconnect:{REGION}:{ACCOUNT_ID}:connection/lmcc-zzzzzzzz"),
            "InterconnectClientException",
        ),
        (
            "another account",
            live_arn.replace(ACCOUNT_ID, "123456789012"),
            "InterconnectValidationException",
        ),
        (
            "another region",
            live_arn.replace(REGION, "eu-west-3"),
            "InterconnectValidationException",
        ),
        (
            "another partition",
            live_arn.replace("arn:aws:", "arn:aws-cn:"),
            "InterconnectValidationException",
        ),
    ];
    for (name, arn, expected_code) in elsewhere {
        let (status, body) = tag(&emulator, &arn, &written).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400 for an ARN naming no connection, got {status} with {body:?}"
        );
        assert!(
            code(&body).contains(expected_code),
            "case {name:?}: expected {expected_code}, got {body:?}"
        );
        assert!(
            message(&body).contains(&arn),
            "case {name:?}: expected the refusal to name the requested ARN, got {:?}",
            message(&body)
        );
        // The refusal brought no resource into existence.
        let (listed_status, listed) = emulator
            .ic("ListTagsForResource", json!({ "arn": arn }))
            .await;
        assert_eq!(
            listed_status, 400,
            "case {name:?}: expected the refused ARN to still name no resource, got {listed_status} with {listed:?}"
        );
    }

    // An `arn` that is not an Interconnect connection ARN is a 400 on the input,
    // whether the modelled `@pattern`/`@length` catches it at the boundary or the
    // handler does: what the rule forbids is a 501 or a silent success.
    let malformed = vec![
        ("not an ARN at all", "tag-me".to_string()),
        (
            "another service's resource",
            format!("arn:aws:directconnect:{REGION}:{ACCOUNT_ID}:connection/mcc-aaaaaaaa"),
        ),
        (
            "another resource type",
            format!("arn:aws:interconnect:{REGION}:{ACCOUNT_ID}:environment/mcc-aaaaaaaa"),
        ),
        (
            "no account id",
            format!("arn:aws:interconnect:{REGION}::connection/mcc-aaaaaaaa"),
        ),
    ];
    for (name, arn) in malformed {
        let (status, body) = tag(&emulator, &arn, &written).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400 for an ARN that is not a connection ARN, got {status} with {body:?}"
        );
    }

    // Every refusal above left the live connection's tags exactly as they were.
    let fetched = emulator.get_connection(&live_id).await.1["connection"].clone();
    assert_eq!(
        tags_of(&fetched),
        tags,
        "case \"after every refusal\": expected the live connection's tags untouched, got {:?}",
        fetched["tags"]
    );
}

/// RULE: a resource's tag set is bounded — at most `MAX_TAGS_PER_RESOURCE` tags are
/// held at once — so a write that would leave the resource holding more is refused
/// with the modelled 400 and changes nothing, while a write that only rewrites keys
/// the resource already carries is accepted even at the bound. The same bound holds
/// on the other writer of the set, tag-on-create.
#[tokio::test]
async fn tag_resource_bounds_the_tags_one_resource_holds() {
    let emulator = Emulator::new();
    let placements = placements(&emulator).await;
    let connection = create_with_tags(&emulator, &placements[0], 1000, &BTreeMap::new()).await;
    let arn = text(&connection, "arn");
    let id = text(&connection, "id");

    /// The per-resource tag bound `state.rs` documents.
    const MAX_TAGS: usize = 50;

    let at_cap: BTreeMap<String, String> = (0..MAX_TAGS)
        .map(|index| (format!("key-{index:02}"), format!("value-{index:02}")))
        .collect();

    // At the bound: accepted, and every tag is on the resource.
    let (status, body) = tag(&emulator, &arn, &at_cap).await;
    assert_eq!(
        status, 200,
        "case \"exactly at the bound\": expected HTTP 200 for {MAX_TAGS} tags, got {status} with {body:?}"
    );
    let stored = tags_of(&emulator.get_connection(&id).await.1["connection"]);
    assert_eq!(
        stored, at_cap,
        "case \"exactly at the bound\": expected all {MAX_TAGS} tags stored, got {} entries",
        stored.len()
    );

    // Rewriting keys the resource already holds does not grow the set, so it is
    // accepted at the bound.
    let rewrite: BTreeMap<String, String> = at_cap
        .keys()
        .take(5)
        .map(|key| (key.clone(), "rewritten".to_string()))
        .collect();
    let (status, body) = tag(&emulator, &arn, &rewrite).await;
    assert_eq!(
        status, 200,
        "case \"rewriting existing keys at the bound\": expected HTTP 200, got {status} with {body:?}"
    );

    // One fresh key past the bound: refused, and the set is unchanged.
    let expected_after_rewrite = {
        let mut expected = at_cap.clone();
        expected.extend(rewrite.iter().map(|(k, v)| (k.clone(), v.clone())));
        expected
    };
    for (name, written) in [
        ("one key past the bound", map(&[("one-too-many", "x")])),
        (
            "several keys past the bound",
            map(&[("extra-a", "1"), ("extra-b", "2")]),
        ),
    ] {
        let (status, body) = tag(&emulator, &arn, &written).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400 once the resource holds {MAX_TAGS} tags, got {status} with {body:?}"
        );
        let stored = tags_of(&emulator.get_connection(&id).await.1["connection"]);
        assert_eq!(
            stored, expected_after_rewrite,
            "case {name:?}: expected the refusal to change nothing, got {} entries",
            stored.len()
        );
    }

    // One request past the bound on an untagged resource is refused the same way,
    // and one exactly at the bound is accepted.
    let fresh = create_with_tags(&emulator, &placements[1 % placements.len()], 1001, &BTreeMap::new())
        .await;
    let fresh_arn = text(&fresh, "arn");
    let past_cap: BTreeMap<String, String> = (0..=MAX_TAGS)
        .map(|index| (format!("key-{index:02}"), format!("value-{index:02}")))
        .collect();
    let (status, body) = tag(&emulator, &fresh_arn, &past_cap).await;
    assert_eq!(
        status, 400,
        "case \"one request past the bound\": expected HTTP 400 for {} tags, got {status} with {body:?}",
        past_cap.len()
    );
    assert!(
        tags_of(&emulator.get_connection(&text(&fresh, "id")).await.1["connection"]).is_empty(),
        "case \"one request past the bound\": expected the refusal to write nothing"
    );

    // The other writer of the same set — tag-on-create — is bounded identically.
    for (name, count, expected) in [
        ("tag-on-create at the bound", MAX_TAGS, 200),
        ("tag-on-create past the bound", MAX_TAGS + 1, 400),
    ] {
        let tags: BTreeMap<String, String> = (0..count)
            .map(|index| (format!("key-{index:02}"), format!("value-{index:02}")))
            .collect();
        let (status, body) = emulator
            .ic(
                "CreateConnection",
                json!({
                    "environmentId": placements[0].0,
                    "bandwidth": placements[0].1,
                    "attachPoint": { "directConnectGateway": format!("aaaaaaaa-bbbb-4ccc-8ddd-{:012}", 1100 + count) },
                    "description": format!("cap case {count}"),
                    "tags": tags,
                }),
            )
            .await;
        assert_eq!(
            status, expected,
            "case {name:?} ({count} tags): expected HTTP {expected}, got {status} with {body:?}"
        );
    }
}

/// RULE: a resource exists until its delete completes — tagging a connection that is
/// still `deleting` succeeds, and once the clock has settled it into the terminal
/// `deleted` state the resource is gone and tagging it is refused as the client
/// fault for an in-scope ARN that names no connection.
#[tokio::test]
async fn tag_resource_follows_the_resource_out_of_existence() {
    let emulator = Emulator::new();
    let placements = placements(&emulator).await;
    let connection = create_with_tags(&emulator, &placements[0], 900, &map(&[("env", "test")])).await;
    let id = text(&connection, "id");
    let arn = text(&connection, "arn");

    let deleted = emulator
        .ic_ok("DeleteConnection", json!({ "identifier": id }))
        .await;
    assert_eq!(
        text(&deleted["connection"], "state"),
        "deleting",
        "case \"straight after the delete\": expected the transient state, got {:?}",
        deleted["connection"]["state"]
    );

    // Still `deleting`: the resource is there, so its tags can still be written.
    let (status, body) = tag(&emulator, &arn, &map(&[("team", "network")])).await;
    assert_eq!(
        status, 200,
        "case \"while deleting\": expected HTTP 200 while the resource still exists, got {status} with {body:?}"
    );
    let fetched = emulator.get_connection(&id).await.1["connection"].clone();
    assert_eq!(
        tags_of(&fetched),
        map(&[("env", "test"), ("team", "network")]),
        "case \"while deleting\": expected the written tag to be on the resource, got {:?}",
        fetched["tags"]
    );

    tokio::time::sleep(SETTLE_WAIT).await;
    let settled = emulator.get_connection(&id).await.1["connection"].clone();
    assert_eq!(
        text(&settled, "state"),
        "deleted",
        "case \"after the delete settles\": expected the terminal state, got {:?}",
        settled["state"]
    );

    let (status, body) = tag(&emulator, &arn, &map(&[("owner", "ops")])).await;
    assert_eq!(
        status, 400,
        "case \"after the delete settles\": expected HTTP 400 for a deleted resource, got {status} with {body:?}"
    );
    assert!(
        code(&body).contains("InterconnectClientException"),
        "case \"after the delete settles\": expected InterconnectClientException, got {body:?}"
    );
}
