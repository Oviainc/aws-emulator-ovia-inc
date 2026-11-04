// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for `ListTagsForResource`, driven over the wire through the
//! shared [`crate::emulator`] harness.
//!
//! RULE under test: `ListTagsForResource` reports the tag set the resource its
//! `arn` names CURRENTLY holds — every key the store holds for it with its value,
//! and no other key — without changing it, so a repeated call with no tag write in
//! between answers the same and the resource reads back unchanged. A resource that
//! carries no tags lists the empty set, and so does an `arn` of the request's scope
//! that resolves to no resource at all: the read answers what is on the resource
//! rather than whether it is there, and nothing was ever tagged. What IS refused is
//! an `arn` the request may not address: one whose account or partition is not the
//! scope the request addresses (HTTP 400), and one that is not a connection ARN at all
//! (HTTP 400), never with a 501.
//!
//! Every assertion is about that rule over a TABLE of inputs whose rows differ in
//! the number of tags (0, 1, 3, 20), the length of the keys and values (each
//! modelled bound), the form of the keys, and the environment and bandwidth the
//! tagged connection sits on. The lengths come from the model:
//! `smithy_query(query="shape",shape="TagKey,TagValue")` gives `TagKey`
//! `@length(min: 1, max: 128)` and `TagValue` `@length(min: 0, max: 256)`.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{json, Value};

use crate::arn_scope_refusals::{assert_answers_fault, Fault};
use crate::emulator::{code, create_with_tags, message, placements, tags_of, text, Emulator};

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

/// `ListTagsForResource` for `arn`, as a `(status, body)` pair so the refusals are
/// readable.
async fn list_tags(emulator: &Emulator, arn: &str) -> (u16, Value) {
    emulator
        .ic("ListTagsForResource", json!({ "arn": arn }))
        .await
}

/// `ListTagsForResource` for `arn`, required to succeed, as its reported tag set.
async fn listed_tags(emulator: &Emulator, arn: &str, case: &str) -> BTreeMap<String, String> {
    let (status, body) = list_tags(emulator, arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from ListTagsForResource, got {status} with {body:?}"
    );
    tags_of(&body)
}

// ---------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------

/// One row: a name and the tag set the connection is created carrying.
#[derive(Debug, Clone)]
struct ListCase {
    name: String,
    tags: Vec<(String, String)>,
}

impl ListCase {
    fn new(name: &str, tags: &[(&str, &str)]) -> Self {
        Self {
            name: name.to_string(),
            tags: tags
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect(),
        }
    }

    fn tag_map(&self) -> BTreeMap<String, String> {
        self.tags.iter().cloned().collect()
    }
}

/// The table: rows differ in the tag count (0, 1, 3 and 20), the key form (plain,
/// spaced, dotted, namespaced, non-ASCII), and each modelled length bound for a key
/// (1 and 128 characters) and for a value (0 and 256 characters).
fn list_table() -> Vec<ListCase> {
    let longest_key = "k".repeat(TAG_KEY_MAX);
    let longest_value = "v".repeat(TAG_VALUE_MAX);
    let mut table = vec![
        ListCase::new("no tags at all", &[]),
        ListCase::new("a single tag", &[("env", "test")]),
        ListCase::new(
            "three tags",
            &[("env", "test"), ("team", "network"), ("owner", "ops")],
        ),
        ListCase::new(
            "keys differing in form",
            &[
                ("Key With Spaces", "1"),
                ("key.with.dots", "2"),
                ("namespace:key/part", "3"),
                ("ключ-日本語", "4"),
            ],
        ),
        // Each modelled bound: the shortest and longest key `TagKey` allows, and
        // the shortest (empty) and longest value `TagValue` allows.
        ListCase::new(
            "the shortest and the longest key",
            &[("k", "shortest key"), (longest_key.as_str(), "longest key")],
        ),
        ListCase::new(
            "the shortest and the longest value",
            &[("empty", ""), ("longest", longest_value.as_str())],
        ),
        ListCase::new(
            "a value that looks like a key, and the reverse",
            &[("env", "team"), ("team", "env")],
        ),
    ];

    // Twenty tags: the same rule at a size no row above reaches.
    table.push(ListCase {
        name: "twenty tags".to_string(),
        tags: (0..20)
            .map(|index| (format!("key-{index:02}"), format!("value-{index:02}")))
            .collect(),
    });

    table
}

// ---------------------------------------------------------------------------
// The rule
// ---------------------------------------------------------------------------

/// RULE: the listing is exactly the tag set the store holds for the resource the
/// `arn` names — the same set `GetConnection` reports — whatever that set is.
#[tokio::test]
async fn list_tags_for_resource_reports_the_stored_tag_set() {
    let emulator = Emulator::new();
    let placements = placements(&emulator).await;
    let table = list_table();
    assert!(
        table.len() >= 8,
        "the listing table must span the class of tag sets, got {} rows",
        table.len()
    );

    for (seed, case) in table.into_iter().enumerate() {
        let placement = &placements[seed % placements.len()];
        let expected = case.tag_map();
        let connection = create_with_tags(&emulator, placement, seed, &expected).await;
        let arn = text(&connection, "arn");
        let id = text(&connection, "id");

        let (status, body) = list_tags(&emulator, &arn).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from ListTagsForResource, got {status} with {body:?}",
            case.name
        );
        assert_eq!(
            tags_of(&body),
            expected,
            "case {:?}: expected the stored tag set {expected:?}, got {:?}",
            case.name,
            body["tags"]
        );
        // "The tags on the specified ARN" is the only member the response models,
        // so nothing else may ride along.
        let members: Vec<String> = body
            .as_object()
            .map(|map| map.keys().cloned().collect())
            .unwrap_or_default();
        assert!(
            members.iter().all(|member| member == "tags"),
            "case {:?}: ListTagsForResourceResponse models only `tags`, got members {members:?}",
            case.name
        );

        // The listing agrees with the resource's own view of its tags.
        let (get_status, fetched) = emulator.get_connection(&id).await;
        assert_eq!(
            get_status, 200,
            "case {:?}: expected the connection to be readable, got {get_status} with {fetched:?}",
            case.name
        );
        assert_eq!(
            tags_of(&body),
            tags_of(&fetched["connection"]),
            "case {:?}: expected the listing to agree with GetConnection, got {:?} against {:?}",
            case.name,
            body["tags"],
            fetched["connection"]["tags"]
        );
    }
}

/// RULE: listing tags is a READ — it changes neither the tag set nor anything else
/// about the resource, so repeating it answers identically and the resource reads
/// back byte-identical.
#[tokio::test]
async fn list_tags_for_resource_changes_nothing() {
    let emulator = Emulator::new();
    let placements = placements(&emulator).await;

    for (seed, case) in list_table().into_iter().enumerate() {
        let placement = &placements[seed % placements.len()];
        let connection = create_with_tags(&emulator, placement, 500 + seed, &case.tag_map()).await;
        let arn = text(&connection, "arn");
        let id = text(&connection, "id");

        let first = listed_tags(&emulator, &arn, &case.name).await;
        let before = emulator.get_connection(&id).await.1["connection"].clone();
        for repeat in 0..3 {
            let again = listed_tags(&emulator, &arn, &case.name).await;
            assert_eq!(
                again, first,
                "case {:?} (read {repeat}): expected a repeated listing to answer the same {first:?}, got {again:?}",
                case.name
            );
        }
        let after = emulator.get_connection(&id).await.1["connection"].clone();
        assert_eq!(
            after, before,
            "case {:?}: expected the reads to leave the resource untouched, got {after:?}",
            case.name
        );
    }
}

/// RULE: the listing tracks the tag WRITES — what `UntagResource` last left on the
/// resource is what the next listing reports, key for key.
#[tokio::test]
async fn list_tags_for_resource_tracks_the_tag_writes() {
    let emulator = Emulator::new();
    let placements = placements(&emulator).await;

    // Rows differ in how much of the set is removed: nothing, one key, a key that
    // was never applied, several keys, and every key.
    let initial: BTreeMap<String, String> = [
        ("env", "test"),
        ("team", "network"),
        ("owner", "ops"),
        ("cost-centre", "4711"),
    ]
    .iter()
    .map(|(key, value)| (key.to_string(), value.to_string()))
    .collect();
    let removals: Vec<(&str, Vec<&str>)> = vec![
        ("nothing removed", vec![]),
        ("one key", vec!["team"]),
        ("a key never applied", vec!["no-such-key"]),
        ("two keys, one absent", vec!["owner", "no-such-key"]),
        (
            "every key",
            vec!["env", "team", "owner", "cost-centre"],
        ),
    ];

    for (seed, (name, remove)) in removals.into_iter().enumerate() {
        let placement = &placements[seed % placements.len()];
        let connection = create_with_tags(&emulator, placement, 600 + seed, &initial).await;
        let arn = text(&connection, "arn");

        assert_eq!(
            listed_tags(&emulator, &arn, name).await,
            initial,
            "case {name:?}: expected the created tag set before any removal"
        );

        let keys: Vec<String> = remove.iter().map(|key| key.to_string()).collect();
        let (status, body) = emulator
            .ic("UntagResource", json!({ "arn": arn, "tagKeys": keys }))
            .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected HTTP 200 from UntagResource, got {status} with {body:?}"
        );

        let expected: BTreeMap<String, String> = initial
            .iter()
            .filter(|(key, _)| !remove.contains(&key.as_str()))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        let listed = listed_tags(&emulator, &arn, name).await;
        assert_eq!(
            listed, expected,
            "case {name:?}: expected the listing to report the set minus {remove:?}, got {listed:?}"
        );
    }
}

/// RULE: the listing is per resource — the `arn` picks out one resource's tag set,
/// never another's and never the union of them.
#[tokio::test]
async fn list_tags_for_resource_reports_only_the_resource_its_arn_names() {
    let emulator = Emulator::new();
    let placements = placements(&emulator).await;

    // Several connections whose tag sets OVERLAP in keys but differ in values, so a
    // handler answering from one shared map would be caught.
    let mut created: Vec<(String, BTreeMap<String, String>)> = Vec::new();
    for (seed, placement) in placements.iter().enumerate().take(4) {
        let tags: BTreeMap<String, String> = [
            ("env".to_string(), format!("env-{seed}")),
            (format!("only-on-{seed}"), "yes".to_string()),
        ]
        .into_iter()
        .collect();
        let connection = create_with_tags(&emulator, placement, 700 + seed, &tags).await;
        created.push((text(&connection, "arn"), tags));
    }
    assert!(
        created.len() >= 2,
        "this rule needs at least two resources, got {}",
        created.len()
    );

    for (index, (arn, expected)) in created.iter().enumerate() {
        let listed = listed_tags(&emulator, arn, arn).await;
        assert_eq!(
            &listed, expected,
            "case {arn:?} (index {index}): expected only this resource's tags {expected:?}, got {listed:?}"
        );
    }
}

/// RULE: "no tags" is the answer to BOTH ways an `arn` of the request's scope can
/// carry none — a resource that exists holding nothing, and an `arn` that resolves to
/// no resource at all — so the read answers HTTP 200 with the empty tag set rather
/// than an error. An `arn` the request may not address is refused instead: another
/// account's or another partition's is `InterconnectValidationException`, and one that
/// is not an Interconnect connection ARN at all is invalid input. Neither is ever a
/// 404 or a 501.
#[tokio::test]
async fn list_tags_for_resource_answers_the_empty_set_for_an_arn_that_names_no_connection() {
    let emulator = Emulator::new();
    let placements = placements(&emulator).await;
    let tags: BTreeMap<String, String> = [("env".to_string(), "test".to_string())]
        .into_iter()
        .collect();
    let live = create_with_tags(&emulator, &placements[0], 800, &tags).await;
    let live_arn = text(&live, "arn");

    // Well-formed connection ARNs of this request's scope that resolve to nothing: an
    // unallocated id under either prefix, and one spelled with a Region other than the
    // called one, which puts the ARN out of no scope at all.
    let absent = vec![
        (
            "an id that was never allocated",
            format!("arn:aws:interconnect:{REGION}:{ACCOUNT_ID}:connection/mcc-00000000"),
        ),
        (
            "the other id prefix, never allocated",
            format!("arn:aws:interconnect:{REGION}:{ACCOUNT_ID}:connection/lmcc-zzzzzzzz"),
        ),
        (
            "an unallocated id spelled with another region",
            format!("arn:aws:interconnect:eu-west-3:{ACCOUNT_ID}:connection/mcc-00000000"),
        ),
    ];
    for (name, arn) in absent {
        let (status, body) = list_tags(&emulator, &arn).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected HTTP 200 for an ARN that resolves to no resource, got {status} with {body:?}"
        );
        assert_eq!(
            body["tags"],
            json!({}),
            "case {name:?}: expected the empty tag set, got {:?}",
            body["tags"]
        );
        assert_eq!(
            tags_of(&body),
            BTreeMap::new(),
            "case {name:?}: expected no tag at all, got {:?}",
            body["tags"]
        );
    }

    // ARNs the request may not address at all — another account, another partition —
    // are faults in the REQUEST, and say which field put them elsewhere, the value the
    // ARN carried for it, the caller's own value and the input shape the ARN arrived
    // in, while reproducing nothing of the ARN itself.
    let out_of_scope = vec![
        (
            "another account",
            live_arn.replace(ACCOUNT_ID, "123456789012"),
        ),
        (
            "another partition",
            live_arn.replace("arn:aws:", "arn:aws-cn:"),
        ),
    ];
    for (name, arn) in out_of_scope {
        let (status, body) = list_tags(&emulator, &arn).await;
        assert_answers_fault(
            name,
            "ListTagsForResource",
            Fault::OutOfScope,
            &arn,
            status,
            &body,
        );
        assert!(
            !message(&body).contains(&arn),
            "case {name:?}: expected the scope refusal not to reproduce the requested ARN, got {:?}",
            message(&body)
        );
        assert!(
            message(&body).ends_with(" for ListTagsForResourceRequest"),
            "case {name:?}: expected the scope refusal to name this request's input shape, got {:?}",
            message(&body)
        );
    }

    // An `arn` that is not an Interconnect connection ARN is a 400 on the input,
    // whether the modelled `@pattern`/`@length` catches it at the boundary or the
    // handler does: what the rule forbids is a 501 or a silent empty listing.
    let malformed = vec![
        ("not an ARN at all", "list-my-tags".to_string()),
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
        let (status, body) = list_tags(&emulator, &arn).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400 for an ARN that is not a connection ARN, got {status} with {body:?}"
        );
        assert!(
            code(&body).contains("ValidationException"),
            "case {name:?}: expected a ValidationException, got {:?}",
            code(&body)
        );
    }

    // Every answer above left the live connection's tags exactly as they were.
    assert_eq!(
        listed_tags(&emulator, &live_arn, "after every refusal").await,
        tags,
        "case \"after every refusal\": expected the live connection's tags untouched"
    );
}

/// RULE: a resource has a tag set for as long as it exists — a connection that is
/// still `deleting` lists its tags, and once the clock has settled it into the
/// terminal `deleted` state the resource is gone, so the listing answers the empty
/// tag set: nothing is on a resource that is not there.
#[tokio::test]
async fn list_tags_for_resource_follows_the_resource_out_of_existence() {
    let emulator = Emulator::new();
    let placements = placements(&emulator).await;
    let tags: BTreeMap<String, String> = [
        ("env".to_string(), "test".to_string()),
        ("team".to_string(), "network".to_string()),
    ]
    .into_iter()
    .collect();
    let connection = create_with_tags(&emulator, &placements[0], 900, &tags).await;
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

    // Still `deleting`: the resource is there, so its tags are still listable.
    assert_eq!(
        listed_tags(&emulator, &arn, "while deleting").await,
        tags,
        "case \"while deleting\": expected the tags of a resource that still exists"
    );

    tokio::time::sleep(SETTLE_WAIT).await;
    let settled = emulator.get_connection(&id).await.1["connection"].clone();
    assert_eq!(
        text(&settled, "state"),
        "deleted",
        "case \"after the delete settles\": expected the terminal state, got {:?}",
        settled["state"]
    );

    let (status, body) = list_tags(&emulator, &arn).await;
    assert_eq!(
        status, 200,
        "case \"after the delete settles\": expected HTTP 200 for a deleted resource, got {status} with {body:?}"
    );
    assert_eq!(
        tags_of(&body),
        BTreeMap::new(),
        "case \"after the delete settles\": expected the empty tag set, got {:?}",
        body["tags"]
    );
}
