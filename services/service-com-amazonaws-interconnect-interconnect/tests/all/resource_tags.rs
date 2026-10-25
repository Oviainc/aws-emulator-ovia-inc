// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for `UntagResource`, driven over the wire through the shared
//! [`crate::emulator`] harness.
//!
//! RULE under test: `UntagResource` removes from the tag set of the resource its
//! `arn` names exactly the keys the request lists, leaves every other key with its
//! value, and is idempotent — a key the resource does not carry is removed from
//! nothing. An `arn` that names no resource of this service is refused with HTTP
//! 400 — the client fault for an in-scope ARN naming nothing, the validation fault
//! for one outside the scope the request addresses — and one that is not a
//! connection ARN at all is refused as invalid input (HTTP 400), never with a 501.
//!
//! Every assertion is about that rule over a TABLE of inputs whose rows differ in
//! the number of tags, the length of the keys and values (each modelled bound), the
//! form of the keys, how many of them are removed, whether a listed key is present
//! at all, and the environment and bandwidth the tagged connection sits on. The
//! lengths come from the model: `smithy_query(query="shape",shape="TagKey,TagValue")`
//! gives `TagKey` `@length(min: 1, max: 128)` and `TagValue` `@length(min: 0, max: 256)`.

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
// Harness helpers
// ---------------------------------------------------------------------------

/// `UntagResource` for `arn`, removing `keys`.
async fn untag(emulator: &Emulator, arn: &str, keys: &[String]) -> (u16, Value) {
    emulator
        .ic("UntagResource", json!({ "arn": arn, "tagKeys": keys }))
        .await
}

// ---------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------

/// One row: the tags the connection is created with, and the keys the call removes.
#[derive(Debug, Clone)]
struct UntagCase {
    name: String,
    initial: Vec<(String, String)>,
    remove: Vec<String>,
}

impl UntagCase {
    fn new(name: &str, initial: &[(&str, &str)], remove: &[&str]) -> Self {
        Self {
            name: name.to_string(),
            initial: initial
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect(),
            remove: remove.iter().map(|key| key.to_string()).collect(),
        }
    }

    fn initial_tags(&self) -> BTreeMap<String, String> {
        self.initial.iter().cloned().collect()
    }

    /// What the rule says is left: the initial set minus the listed keys.
    fn expected_tags(&self) -> BTreeMap<String, String> {
        self.initial
            .iter()
            .filter(|(key, _)| !self.remove.contains(key))
            .cloned()
            .collect()
    }
}

/// The table: rows differ in the tag count (0, 1, 3, 4 and 20), the share of keys
/// removed (none, one, some, all, one that is absent), the key form (plain,
/// spaced, dotted, namespaced, non-ASCII), duplicate keys in the request, and each
/// modelled length bound for a key and for a value.
fn untag_table() -> Vec<UntagCase> {
    let longest_key = "k".repeat(TAG_KEY_MAX);
    let longest_value = "v".repeat(TAG_VALUE_MAX);
    let mut table = vec![
        UntagCase::new("no tags at all", &[], &["env"]),
        UntagCase::new("the only tag", &[("env", "test")], &["env"]),
        UntagCase::new(
            "one of three",
            &[("env", "test"), ("team", "network"), ("owner", "ops")],
            &["team"],
        ),
        UntagCase::new(
            "two of three",
            &[("env", "test"), ("team", "network"), ("owner", "ops")],
            &["env", "owner"],
        ),
        UntagCase::new(
            "every tag of three",
            &[("env", "test"), ("team", "network"), ("owner", "ops")],
            &["env", "team", "owner"],
        ),
        UntagCase::new(
            "a key that was never applied",
            &[("env", "test")],
            &["a-key-nobody-applied"],
        ),
        UntagCase::new(
            "an empty key list",
            &[("env", "test"), ("team", "network")],
            &[],
        ),
        UntagCase::new(
            "the same key twice",
            &[("env", "test"), ("team", "network")],
            &["env", "env"],
        ),
        UntagCase::new(
            "keys differing in form",
            &[
                ("Key With Spaces", "1"),
                ("key.with.dots", "2"),
                ("namespace:key/part", "3"),
                ("ключ-日本語", "4"),
            ],
            &["Key With Spaces", "namespace:key/part"],
        ),
        // Each modelled bound: the shortest and the longest key `TagKey` allows,
        // and the shortest (empty) and longest value `TagValue` allows.
        UntagCase::new(
            "the longest key, keeping the shortest",
            &[("k", "short-key"), (longest_key.as_str(), "long-key")],
            &[longest_key.as_str()],
        ),
        UntagCase::new(
            "the shortest key, keeping the longest",
            &[("k", "short-key"), (longest_key.as_str(), "long-key")],
            &["k"],
        ),
        UntagCase::new(
            "an empty value, keeping the longest value",
            &[("empty", ""), ("longest", longest_value.as_str())],
            &["empty"],
        ),
        UntagCase::new(
            "the longest value, keeping the empty one",
            &[("empty", ""), ("longest", longest_value.as_str())],
            &["longest"],
        ),
    ];

    // Twenty tags with every other key removed: the same rule at a size no row
    // above reaches.
    let many: Vec<(String, String)> = (0..20)
        .map(|index| (format!("key-{index:02}"), format!("value-{index:02}")))
        .collect();
    table.push(UntagCase {
        name: "every other key of twenty".to_string(),
        remove: many
            .iter()
            .enumerate()
            .filter(|(index, _)| index.is_multiple_of(2))
            .map(|(_, (key, _))| key.clone())
            .collect(),
        initial: many,
    });

    table
}

// ---------------------------------------------------------------------------
// The rule
// ---------------------------------------------------------------------------

/// RULE: `UntagResource` removes exactly the listed keys from the resource's
/// persisted tag set, keeps every other key and value, changes nothing else about
/// the resource, and is idempotent — and the stored record is what `GetConnection`
/// reads back.
#[tokio::test]
async fn untag_resource_removes_exactly_the_listed_keys_and_keeps_the_rest() {
    let emulator = Emulator::new();
    let placements = placements(&emulator).await;
    let table = untag_table();
    assert!(
        table.len() >= 8,
        "the untag table must span the class of tag sets, got {} rows",
        table.len()
    );

    for (seed, case) in table.into_iter().enumerate() {
        let placement = &placements[seed % placements.len()];
        let before =
            create_with_tags(&emulator, placement, seed, &case.initial_tags()).await;
        let arn = text(&before, "arn");
        let id = text(&before, "id");
        assert_eq!(
            tags_of(&before),
            case.initial_tags(),
            "case {:?}: expected the created connection to carry the supplied tags, got {:?}",
            case.name,
            before["tags"]
        );

        let (status, body) = untag(&emulator, &arn, &case.remove).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from UntagResource, got {status} with {body:?}",
            case.name
        );
        assert!(
            body.is_null() || body == json!({}),
            "case {:?}: UntagResourceResponse models no members, so expected an empty body, got {body:?}",
            case.name
        );

        // The persisted record is what a reader sees.
        let (get_status, fetched) = emulator.get_connection(&id).await;
        assert_eq!(
            get_status, 200,
            "case {:?}: expected the untagged connection to still be readable, got {get_status} with {fetched:?}",
            case.name
        );
        let after = fetched["connection"].clone();
        assert_eq!(
            tags_of(&after),
            case.expected_tags(),
            "case {:?}: expected the tag set minus {:?}, got {:?}",
            case.name,
            case.remove,
            after["tags"]
        );
        assert_eq!(
            without_tags(&after),
            without_tags(&before),
            "case {:?}: expected only the tag set to change, got {:?}",
            case.name,
            after
        );
        // Removing every key leaves no tags at all, not a key with an empty value.
        if case.expected_tags().is_empty() {
            assert!(
                after["tags"].is_null() || after["tags"] == json!({}),
                "case {:?}: expected no tags once every key is removed, got {:?}",
                case.name,
                after["tags"]
            );
        }

        // Idempotent: the same call again succeeds and changes nothing.
        let (repeat_status, repeat_body) = untag(&emulator, &arn, &case.remove).await;
        assert_eq!(
            repeat_status, 200,
            "case {:?}: expected a repeated UntagResource to succeed, got {repeat_status} with {repeat_body:?}",
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

/// RULE: the keys a request may name are bounded by the model — `TagKey` is
/// `@length(min: 1, max: 128)` — so a key at either published bound is accepted and
/// one past either bound is refused on the input (HTTP 400), never with a 501.
#[tokio::test]
async fn untag_resource_holds_tag_keys_to_the_modelled_length() {
    let emulator = Emulator::new();
    let placements = placements(&emulator).await;
    let tags: BTreeMap<String, String> = [
        ("k".to_string(), "at the minimum".to_string()),
        ("k".repeat(TAG_KEY_MAX), "at the maximum".to_string()),
    ]
    .into_iter()
    .collect();
    let connection = create_with_tags(&emulator, &placements[0], 400, &tags).await;
    let arn = text(&connection, "arn");

    for (name, key, expected) in [
        ("one character, the modelled minimum", "k".to_string(), 200),
        (
            "128 characters, the modelled maximum",
            "k".repeat(TAG_KEY_MAX),
            200,
        ),
        ("empty, one below the minimum", String::new(), 400),
        (
            "129 characters, one past the maximum",
            "k".repeat(TAG_KEY_MAX + 1),
            400,
        ),
    ] {
        let (status, body) = untag(&emulator, &arn, std::slice::from_ref(&key)).await;
        assert_eq!(
            status,
            expected,
            "case {name:?} (key length {}): expected HTTP {expected}, got {status} with {body:?}",
            key.len()
        );
    }
}

/// RULE: untagging one resource touches no other resource's tags.
#[tokio::test]
async fn untag_resource_touches_only_the_resource_its_arn_names() {
    let emulator = Emulator::new();
    let placements = placements(&emulator).await;

    // Several connections, all carrying the SAME keys, so a handler that removed a
    // key globally rather than per resource would be caught.
    let shared: BTreeMap<String, String> = [("env", "test"), ("team", "network")]
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();
    let mut created = Vec::new();
    for (seed, placement) in placements.iter().enumerate().take(4) {
        created.push(create_with_tags(&emulator, placement, 100 + seed, &shared).await);
    }
    assert!(
        created.len() >= 2,
        "this rule needs at least two resources, got {}",
        created.len()
    );

    let target = created[0].clone();
    let (status, body) = untag(&emulator, &text(&target, "arn"), &["env".to_string()]).await;
    assert_eq!(
        status, 200,
        "case \"one of several resources\": expected HTTP 200, got {status} with {body:?}"
    );

    for (index, connection) in created.iter().enumerate() {
        let id = text(connection, "id");
        let fetched = emulator.get_connection(&id).await.1["connection"].clone();
        let expected: BTreeMap<String, String> = if index == 0 {
            shared
                .iter()
                .filter(|(key, _)| key.as_str() != "env")
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
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
/// changes nothing, with DIFFERENT codes for the two ways it can name none:
/// `InterconnectValidationException` for an ARN outside the partition, Region and
/// account this request addresses, and `InterconnectClientException` for an in-scope
/// ARN that names no connection. An `arn` that is not a connection ARN at all is
/// likewise invalid input (HTTP 400). None of them is ever a 404 or a 501.
#[tokio::test]
async fn untag_resource_refuses_an_arn_that_names_no_connection() {
    let emulator = Emulator::new();
    let placements = placements(&emulator).await;
    let tags: BTreeMap<String, String> = [("env".to_string(), "test".to_string())]
        .into_iter()
        .collect();
    let live = create_with_tags(&emulator, &placements[0], 200, &tags).await;
    let live_id = text(&live, "id");
    let live_arn = text(&live, "arn");

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
        let (status, body) = untag(&emulator, &arn, &["env".to_string()]).await;
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
    }

    // An `arn` that is not an Interconnect connection ARN is a 400 on the input,
    // whether the modelled `@pattern`/`@length` catches it at the boundary or the
    // handler does: what the rule forbids is a 501 or a silent success.
    let malformed = vec![
        ("not an ARN at all", "untag-me".to_string()),
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
        let (status, body) = untag(&emulator, &arn, &["env".to_string()]).await;
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

/// RULE: a resource exists until its delete completes — untagging a connection
/// that is still `deleting` succeeds, and once the clock has settled it into the
/// terminal `deleted` state the resource is gone and untagging it is refused as the
/// client fault for an in-scope ARN that names no connection.
#[tokio::test]
async fn untag_resource_follows_the_resource_out_of_existence() {
    let emulator = Emulator::new();
    let placements = placements(&emulator).await;
    let tags: BTreeMap<String, String> = [
        ("env".to_string(), "test".to_string()),
        ("team".to_string(), "network".to_string()),
    ]
    .into_iter()
    .collect();
    let connection = create_with_tags(&emulator, &placements[0], 300, &tags).await;
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

    // Still `deleting`: the resource is there, so its tags can still be changed.
    let (status, body) = untag(&emulator, &arn, &["env".to_string()]).await;
    assert_eq!(
        status, 200,
        "case \"while deleting\": expected HTTP 200 while the resource still exists, got {status} with {body:?}"
    );
    let fetched = emulator.get_connection(&id).await.1["connection"].clone();
    assert_eq!(
        tags_of(&fetched),
        [("team".to_string(), "network".to_string())]
            .into_iter()
            .collect::<BTreeMap<String, String>>(),
        "case \"while deleting\": expected only 'env' to have gone, got {:?}",
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

    let (status, body) = untag(&emulator, &arn, &["team".to_string()]).await;
    assert_eq!(
        status, 400,
        "case \"after the delete settles\": expected HTTP 400 for a deleted resource, got {status} with {body:?}"
    );
    assert!(
        code(&body).contains("InterconnectClientException"),
        "case \"after the delete settles\": expected InterconnectClientException, got {body:?}"
    );
}
