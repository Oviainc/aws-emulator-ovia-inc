// SPDX-License-Identifier: CC0-1.0

//! Amazon EMR resource tagging, end to end over the wire.
//!
//! Every case drives the real HTTP path — `POST /` with
//! `X-Amz-Target: ElasticMapReduce.<Op>` and an `awsJson1_1` body — through the
//! router's `dispatch`, using the harness `tests/all/job_flow_lifecycle.rs` owns, so
//! these are the same bytes an SDK client sends. Nothing reaches into the service's
//! state struct: a tag set is only ever read back through `DescribeCluster` or
//! `DescribeStudio`, which is what makes these tests evidence that the responses are
//! computed from persisted state.
//!
//! THE RULE every table below asserts: `AddTags` merges the submitted key-value pairs
//! into the tag set of the Amazon EMR resource its `ResourceId` names — adding a key the
//! resource does not carry and replacing the value of one it does, changing nothing else
//! about the resource — and refuses an id naming no resource this account holds, a request
//! that names no tag at all, as well
//! as a tag outside what a tag may be: the Unicode letters, separators and numbers plus
//! `_ . : / = + \ -` of the tag character class, a key of at most 128 characters and a
//! value of at most 256.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_emr_elasticmapreduce::state::{
    MAX_TAGS_PER_RESOURCE, MAX_TAG_KEY_CHARS, MAX_TAG_VALUE_CHARS, TAG_PUNCTUATION,
};

use crate::job_flow_lifecycle::{assert_refused, at, kept_alive_request, Emr};
use crate::refusal_messages::{message, studio_request};
use crate::tag_character_class::tag_charset_refusal;

/// The resource kinds `AddTags` names: "a cluster identifier or an Amazon EMR Studio ID"
/// (`AddTags$ResourceId` doc), each with the read that reports its tags.
pub(crate) const CLUSTER: &str = "cluster";
pub(crate) const STUDIO: &str = "studio";

/// The error code the operation models, which every refusal below carries.
pub(crate) const INVALID_REQUEST_EXCEPTION: &str = "InvalidRequestException";

/// A tag key and its value, as a row states it. `None` is a tag submitted with no
/// `Value` at all, which the model allows ("an optional value string").
///
/// `pub(crate)` here and below so the sibling suite that drives `RemoveTags` states its
/// tags, creates its resources and reads them back through these helpers rather than its
/// own copies.
pub(crate) type Pair = (String, Option<String>);

pub(crate) fn pair(key: &str, value: Option<&str>) -> Pair {
    (key.to_string(), value.map(str::to_string))
}

/// A `TagList` as the wire carries it: `Value` is left out of an entry that has none.
pub(crate) fn tag_list(pairs: &[Pair]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(key, value)| {
                let mut tag = json!({ "Key": key });
                if let Some(value) = value {
                    tag["Value"] = json!(value);
                }
                tag
            })
            .collect(),
    )
}

/// The tag set a read reports, keyed by tag key — which is also what makes a duplicated
/// key visible, since the raw entry count is asserted beside it.
fn reported_tags(case: &str, tags: &Value) -> BTreeMap<String, Option<String>> {
    let entries = tags
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a Tags list, got {tags}"));
    entries
        .iter()
        .map(|tag| {
            let key = at(tag, &["Key"])
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a tag must carry a Key, got {tag}"))
                .to_string();
            (key, at(tag, &["Value"]).as_str().map(str::to_string))
        })
        .collect()
}

fn expected_map(pairs: &[Pair]) -> BTreeMap<String, Option<String>> {
    pairs.iter().cloned().collect()
}

/// A Studio this account holds, under `name`.
pub(crate) async fn create_studio(emr: &Emr, name: &str) -> String {
    emr.ok("CreateStudio", studio_request(name)).await["StudioId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateStudio must answer a StudioId for {name:?}"))
        .to_string()
}

/// The `Tags` member of the resource `resource_id` names, read back through the
/// operation that reports that kind of resource.
pub(crate) async fn read_tags(emr: &Emr, kind: &str, resource_id: &str) -> Value {
    if kind == CLUSTER {
        let cluster = emr.describe_cluster(resource_id).await;
        at(&cluster, &["Tags"]).clone()
    } else {
        let studio = emr
            .ok("DescribeStudio", json!({ "StudioId": resource_id }))
            .await;
        at(&studio, &["Studio", "Tags"]).clone()
    }
}

/// Assert the resource carries exactly `expected`, read back through its own describe.
pub(crate) async fn assert_tags(
    emr: &Emr,
    case: &str,
    kind: &str,
    resource_id: &str,
    expected: &[Pair],
) {
    let tags = read_tags(emr, kind, resource_id).await;
    if expected.is_empty() {
        assert!(
            tags.is_null() || tags.as_array().is_some_and(|entries| entries.is_empty()),
            "case {case:?}: an untagged resource reports no tags, got {tags}"
        );
        return;
    }
    let entries = tags
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a Tags list, got {tags}"));
    assert_eq!(
        entries.len(),
        expected.len(),
        "case {case:?}: expected {} tags, got {} in {tags}",
        expected.len(),
        entries.len()
    );
    let reported = reported_tags(case, &tags);
    assert_eq!(
        reported,
        expected_map(expected),
        "case {case:?}: expected {:?}, got {reported:?}",
        expected_map(expected)
    );
}

/// One row: the `AddTags` calls it makes, in order, and the tag set the resource carries
/// once they have all been made.
struct Case {
    label: &'static str,
    calls: Vec<Vec<Pair>>,
    expected: Vec<Pair>,
}

/// The table, whose rows differ along what the rule generalises over: the FORM of a tag
/// value (ASCII, non-ASCII, astral-plane, empty, absent, at the documented maximum in
/// characters), the COUNT of tags in a call and on the resource (one, several, the
/// documented maximum), and the MERGE itself (a new key added, a held key replaced, the
/// same tag submitted twice).
fn cases() -> Vec<Case> {
    // SOURCE for both bounds: `AddTags$Tags` ("a required key string with a maximum of
    // 128 characters, and an optional value string with a maximum of 256 characters"),
    // exported as `MAX_TAG_KEY_CHARS` / `MAX_TAG_VALUE_CHARS`. The non-ASCII row is the
    // same length counted in CHARACTERS, where every character takes two bytes.
    let longest_key = "k".repeat(MAX_TAG_KEY_CHARS);
    let longest_value = "v".repeat(MAX_TAG_VALUE_CHARS);
    let longest_non_ascii_value = "é".repeat(MAX_TAG_VALUE_CHARS);
    // Every symbol a tag may carry beside letters, digits and separators, taken from the
    // exported list rather than a copy of it.
    let punctuation: String = TAG_PUNCTUATION.iter().collect();

    // The documented maximum number of tags one resource carries, each a distinct key.
    let full_set: Vec<Pair> = (0..MAX_TAGS_PER_RESOURCE)
        .map(|at| pair(&format!("quota-{at:02}"), Some(&format!("value-{at:02}"))))
        .collect();

    vec![
        Case {
            label: "one ASCII tag",
            calls: vec![vec![pair("stack", Some("Prod"))]],
            expected: vec![pair("stack", Some("Prod"))],
        },
        Case {
            label: "a non-ASCII key and value",
            calls: vec![vec![pair("окружение", Some("производство"))]],
            expected: vec![pair("окружение", Some("производство"))],
        },
        Case {
            label: "every symbol a tag may carry beside letters, digits and separators",
            calls: vec![vec![pair("cost:centre/9", Some(&format!("{punctuation} 42")))]],
            expected: vec![pair("cost:centre/9", Some(&format!("{punctuation} 42")))],
        },
        Case {
            label: "a value of mixed scripts and separators",
            calls: vec![vec![pair("說明", Some("プロダクション cluster α"))]],
            expected: vec![pair("說明", Some("プロダクション cluster α"))],
        },
        Case {
            label: "an empty value",
            calls: vec![vec![pair("audited", Some(""))]],
            expected: vec![pair("audited", Some(""))],
        },
        Case {
            label: "no value at all",
            calls: vec![vec![pair("placeholder", None)]],
            expected: vec![pair("placeholder", None)],
        },
        Case {
            label: "a key and a value of the documented maximum length",
            calls: vec![vec![pair(&longest_key, Some(&longest_value))]],
            expected: vec![pair(&longest_key, Some(&longest_value))],
        },
        Case {
            label: "a non-ASCII value of the documented maximum length in characters",
            calls: vec![vec![pair("длина", Some(&longest_non_ascii_value))]],
            expected: vec![pair("длина", Some(&longest_non_ascii_value))],
        },
        Case {
            label: "several tags in one call",
            calls: vec![vec![
                pair("stack", Some("Prod")),
                pair("team", Some("data")),
                pair("cost-centre", Some("4711")),
            ]],
            expected: vec![
                pair("cost-centre", Some("4711")),
                pair("stack", Some("Prod")),
                pair("team", Some("data")),
            ],
        },
        Case {
            label: "a second call adds a new key and replaces a held one",
            calls: vec![
                vec![pair("stack", Some("Prod")), pair("team", Some("data"))],
                vec![pair("stack", Some("Staging")), pair("owner", Some("ops"))],
            ],
            expected: vec![
                pair("owner", Some("ops")),
                pair("stack", Some("Staging")),
                pair("team", Some("data")),
            ],
        },
        Case {
            label: "a held key replaced by an absent value",
            calls: vec![
                vec![pair("stack", Some("Prod"))],
                vec![pair("stack", None)],
            ],
            expected: vec![pair("stack", None)],
        },
        Case {
            label: "the same tag submitted twice",
            calls: vec![
                vec![pair("stack", Some("Prod"))],
                vec![pair("stack", Some("Prod"))],
            ],
            expected: vec![pair("stack", Some("Prod"))],
        },
        Case {
            label: "a key repeated inside one call takes the last value",
            calls: vec![vec![
                pair("stack", Some("Prod")),
                pair("stack", Some("Staging")),
            ]],
            expected: vec![pair("stack", Some("Staging"))],
        },
        Case {
            label: "the documented maximum number of tags",
            calls: vec![full_set.clone()],
            expected: full_set,
        },
        Case {
            // The ceiling is counted on the SET a merge leaves behind, so a call longer
            // than the ceiling that names ONE key adds one tag and is accepted.
            label: "one key repeated more times than a resource carries tags",
            calls: vec![(0..=MAX_TAGS_PER_RESOURCE)
                .map(|at| pair("stack", Some(&format!("value-{at:02}"))))
                .collect()],
            expected: vec![pair(
                "stack",
                Some(&format!("value-{MAX_TAGS_PER_RESOURCE:02}")),
            )],
        },
    ]
}

/// `AddTags` on `resource_id`, which must succeed and answer a body with no members.
pub(crate) async fn add_tags_ok(emr: &Emr, case: &str, resource_id: &str, tags: &[Pair]) {
    let out = emr
        .ok(
            "AddTags",
            json!({ "ResourceId": resource_id, "Tags": tag_list(tags) }),
        )
        .await;
    assert!(
        out.as_object().is_some_and(serde_json::Map::is_empty),
        "case {case:?}: AddTags answers a body with no members, got {out}"
    );
}

// ---------------------------------------------------------------------------
// AddTags -> DescribeCluster / DescribeStudio
// ---------------------------------------------------------------------------

/// THE RULE: every row's calls leave the resource carrying exactly the merged tag set,
/// for both resource kinds `AddTags` names, and a read reports it unchanged however
/// often it is asked.
#[tokio::test]
async fn add_tags_merges_the_submitted_tags_and_the_describe_reports_the_merged_set() {
    let emr = Emr::new();

    for case in cases() {
        let label = case.label;
        // A fresh cluster and a fresh Studio per row, so this row's calls are the only
        // tags either of them carries.
        let cluster = emr.run_job_flow(kept_alive_request(label)).await;
        let studio = create_studio(&emr, label).await;

        for (kind, resource_id) in [(CLUSTER, &cluster), (STUDIO, &studio)] {
            let case_name = format!("{label} on a {kind}");
            for call in &case.calls {
                add_tags_ok(&emr, &case_name, resource_id, call).await;
            }
            assert_tags(&emr, &case_name, kind, resource_id, &case.expected).await;
            // A read changes nothing a later caller sees: the second read reports the
            // same set as the first.
            assert_tags(
                &emr,
                &format!("{case_name}, read twice"),
                kind,
                resource_id,
                &case.expected,
            )
            .await;
        }
    }
}

/// THE RULE where the resource was created WITH tags: the merge is into the set the
/// create gave it, and nothing else about the resource moves.
#[tokio::test]
async fn add_tags_merges_into_the_tags_the_create_gave_the_resource() {
    let emr = Emr::new();

    let mut launch = kept_alive_request("created-with-tags");
    launch["Tags"] = tag_list(&[pair("stack", Some("Prod")), pair("team", Some("data"))]);
    let cluster = emr.run_job_flow(launch).await;
    let before = emr.describe_cluster(&cluster).await;

    add_tags_ok(
        &emr,
        "a cluster created with tags",
        &cluster,
        &[pair("stack", Some("Staging")), pair("owner", Some("ops"))],
    )
    .await;
    assert_tags(
        &emr,
        "a cluster created with tags",
        CLUSTER,
        &cluster,
        &[
            pair("owner", Some("ops")),
            pair("stack", Some("Staging")),
            pair("team", Some("data")),
        ],
    )
    .await;

    // Tagging changes the tags and nothing else: everything the cluster reported beside
    // its tags reads back the same.
    let after = emr.describe_cluster(&cluster).await;
    for member in ["Id", "Name", "ClusterArn", "ReleaseLabel", "AutoTerminate"] {
        assert_eq!(
            at(&after, &[member]),
            at(&before, &[member]),
            "case \"a cluster created with tags\": {member} must not move, got {after}"
        );
    }

    let mut studio_body = studio_request("created-with-tags");
    studio_body["Tags"] = tag_list(&[pair("stack", Some("Prod"))]);
    let studio = emr.ok("CreateStudio", studio_body).await["StudioId"]
        .as_str()
        .expect("CreateStudio answers a StudioId")
        .to_string();
    add_tags_ok(
        &emr,
        "a Studio created with tags",
        &studio,
        &[pair("stack", Some("Staging")), pair("owner", Some("ops"))],
    )
    .await;
    assert_tags(
        &emr,
        "a Studio created with tags",
        STUDIO,
        &studio,
        &[pair("owner", Some("ops")), pair("stack", Some("Staging"))],
    )
    .await;
    let described = emr
        .ok("DescribeStudio", json!({ "StudioId": &studio }))
        .await;
    assert_eq!(
        at(&described, &["Studio", "Name"]).as_str(),
        Some("created-with-tags"),
        "case \"a Studio created with tags\": the Studio's name must not move, got {described}"
    );
}

/// THE RULE's scope: a tag goes on the resource the request names and on no other, so
/// every other resource this account holds reads back as it was.
#[tokio::test]
async fn add_tags_leaves_every_other_resource_alone() {
    let emr = Emr::new();
    let tagged = emr.run_job_flow(kept_alive_request("tagged")).await;
    let untouched_cluster = emr.run_job_flow(kept_alive_request("untouched")).await;
    let tagged_studio = create_studio(&emr, "tagged-studio").await;
    let untouched_studio = create_studio(&emr, "untouched-studio").await;

    add_tags_ok(&emr, "the named cluster", &tagged, &[pair("stack", Some("Prod"))]).await;
    add_tags_ok(
        &emr,
        "the named Studio",
        &tagged_studio,
        &[pair("stack", Some("Staging"))],
    )
    .await;

    assert_tags(
        &emr,
        "the named cluster",
        CLUSTER,
        &tagged,
        &[pair("stack", Some("Prod"))],
    )
    .await;
    assert_tags(
        &emr,
        "the named Studio",
        STUDIO,
        &tagged_studio,
        &[pair("stack", Some("Staging"))],
    )
    .await;
    assert_tags(&emr, "another cluster", CLUSTER, &untouched_cluster, &[]).await;
    assert_tags(&emr, "another Studio", STUDIO, &untouched_studio, &[]).await;
}

// ---------------------------------------------------------------------------
// ClusterId: what scopes the tag operation
// ---------------------------------------------------------------------------

/// THE RULE for `ClusterId`, "the ID of the cluster that scopes the tag operation": a
/// request carrying one tags that cluster or something that lives on it, so the cluster
/// itself is tagged, a `ClusterId` naming no cluster is refused, and a `ResourceId`
/// naming neither the cluster nor a session of it is refused. An empty `ClusterId`
/// scopes nothing, so the `ResourceId` is resolved on its own.
#[tokio::test]
async fn cluster_id_scopes_the_tag_operation() {
    let emr = Emr::new();
    let cluster = emr.run_job_flow(kept_alive_request("scoped")).await;

    // The scoping cluster tagged through its own id.
    emr.ok(
        "AddTags",
        json!({
            "ClusterId": &cluster,
            "ResourceId": &cluster,
            "Tags": tag_list(&[pair("stack", Some("Prod"))]),
        }),
    )
    .await;
    assert_tags(
        &emr,
        "the scoping cluster as the resource",
        CLUSTER,
        &cluster,
        &[pair("stack", Some("Prod"))],
    )
    .await;

    // An empty `ClusterId` scopes nothing: the resource is resolved on its own.
    emr.ok(
        "AddTags",
        json!({
            "ClusterId": "",
            "ResourceId": &cluster,
            "Tags": tag_list(&[pair("team", Some("data"))]),
        }),
    )
    .await;
    assert_tags(
        &emr,
        "an empty ClusterId",
        CLUSTER,
        &cluster,
        &[pair("stack", Some("Prod")), pair("team", Some("data"))],
    )
    .await;

    // A `ClusterId` this account holds no cluster for: the cluster-id sentence.
    for unknown in ["j-ZZZZZZZZZZZZZ", "j-", "not-a-cluster-id"] {
        let case = format!("ClusterId {unknown:?}");
        let (status, payload) = emr
            .call(
                "AddTags",
                json!({
                    "ClusterId": unknown,
                    "ResourceId": &cluster,
                    "Tags": tag_list(&[pair("rejected", Some("yes"))]),
                }),
            )
            .await;
        assert_refused(&case, status, &payload, "InvalidRequestException");
        assert_eq!(
            message(&case, &payload),
            format!("Cluster id '{unknown}' is not valid."),
            "case {case:?}: unexpected message in {payload}"
        );
    }

    // A `ResourceId` that is neither the scoping cluster nor a session of it: the
    // session sentence, naming the id that did not resolve.
    for unknown in ["s-SESSIONZZZZZ", "es-NOPE", ""] {
        let case = format!("ResourceId {unknown:?} scoped to a cluster");
        let (status, payload) = emr
            .call(
                "AddTags",
                json!({
                    "ClusterId": &cluster,
                    "ResourceId": unknown,
                    "Tags": tag_list(&[pair("rejected", Some("yes"))]),
                }),
            )
            .await;
        assert_refused(&case, status, &payload, "InvalidRequestException");
        assert_eq!(
            message(&case, &payload),
            format!("Session id '{unknown}' is not valid."),
            "case {case:?}: unexpected message in {payload}"
        );
    }

    // Every refusal above left the cluster's tags exactly as the two accepted calls did.
    assert_tags(
        &emr,
        "the scoped cluster after the refusals",
        CLUSTER,
        &cluster,
        &[pair("stack", Some("Prod")), pair("team", Some("data"))],
    )
    .await;
}

// ---------------------------------------------------------------------------
// What AddTags refuses
// ---------------------------------------------------------------------------

/// The `ErrorCode` member of a refusal, or `None` when it carries none.
pub(crate) fn error_code(payload: &Value) -> Option<String> {
    at(payload, &["ErrorCode"]).as_str().map(str::to_string)
}

/// THE OTHER HALF of the rule: a `ResourceId` naming no resource this account holds is
/// refused — with the sentence the surface that owns an id of that form answers, and
/// that surface's discriminator — and nothing is stored. Rows span the id forms a caller
/// sends, including a Studio-shaped id, and both a never-created and a never-creatable
/// id.
#[tokio::test]
async fn an_unresolvable_resource_id_is_refused() {
    let emr = Emr::new();
    // A resource EXISTS throughout, so every refusal below is a lookup rather than a
    // blanket refusal of the operation.
    let held = emr.run_job_flow(kept_alive_request("held")).await;
    add_tags_ok(&emr, "the held cluster", &held, &[pair("stack", Some("Prod"))]).await;

    for unknown in [
        "j-ZZZZZZZZZZZZZ",
        "j-1",
        "j-",
        "",
        "not-a-resource-id",
        "ex-NOTANEXECUTION",
    ] {
        let case = format!("ResourceId {unknown:?}");
        let (status, payload) = emr
            .call(
                "AddTags",
                json!({
                    "ResourceId": unknown,
                    "Tags": tag_list(&[pair("stack", Some("Prod"))]),
                }),
            )
            .await;
        assert_refused(&case, status, &payload, "InvalidRequestException");
        assert_eq!(
            message(&case, &payload),
            format!("Cluster id '{unknown}' is not valid."),
            "case {case:?}: unexpected message in {payload}"
        );
        assert_eq!(
            error_code(&payload).as_deref(),
            Some("MalformedClusterId"),
            "case {case:?}: a cluster id resolving to no cluster carries the cluster \
             surface's discriminator, got {payload}"
        );
    }

    // A Studio-shaped id no Studio carries is the Studio surface's refusal, which
    // reports an ABSENT resource and so carries that discriminator.
    for unknown in ["es-NOPE", "es-ZZZZZZZZZZZZZ", "es-"] {
        let case = format!("ResourceId {unknown:?}");
        let (status, payload) = emr
            .call(
                "AddTags",
                json!({
                    "ResourceId": unknown,
                    "Tags": tag_list(&[pair("stack", Some("Prod"))]),
                }),
            )
            .await;
        assert_refused(&case, status, &payload, "InvalidRequestException");
        assert!(
            message(&case, &payload).starts_with("Studio does not exist."),
            "case {case:?}: expected the Studio sentence, got {payload}"
        );
        assert_eq!(
            error_code(&payload).as_deref(),
            Some("ResourceNotFound"),
            "case {case:?}: an absent resource carries that discriminator, got {payload}"
        );
    }

    // The held cluster still carries exactly what it was given.
    assert_tags(
        &emr,
        "the held cluster after the refusals",
        CLUSTER,
        &held,
        &[pair("stack", Some("Prod"))],
    )
    .await;
}

/// Characters outside the class a tag is drawn from: a symbol (the emoji class),
/// punctuation the class does not name (`@` among it), and a control character
/// (whitespace that is no separator).
///
/// SOURCE: the tag character class the service states, exported as
/// `state::TAG_CHARACTER_PATTERN` — Unicode letters, separators and numbers plus
/// `_ . : / = + \ -` — so every character outside it is refused. The rows test characters
/// the issue did not cite beside the one it did; the whole class is walked by the sibling
/// suite `tests/all/tag_character_class.rs`.
const REJECTED_TAG_CHARS: [char; 9] = ['🚀', '#', '!', '%', '*', ',', '"', '\t', '@'];

/// The sentence a tag key or value PAST ITS LENGTH BOUND is refused with: which half of
/// the pair it was and the value the request sent, in this service's own wording for a
/// value it will not hold. Spelled out here, not read from the service.
///
/// A key or value carrying a character the class leaves out is a different class of
/// refusal, with Amazon EMR's own sentence for it
/// ([`crate::tag_character_class::tag_charset_refusal`]).
fn tag_too_long(member: &str, value: &str) -> String {
    format!("Tag {member} '{value}' is not valid.")
}

/// THE RULE's bounds: a tag no Amazon EMR resource could carry is refused and NOTHING of
/// the request is stored — not even the entries beside the bad one. Rows walk each
/// published bound (`MAX_TAG_KEY_CHARS`, `MAX_TAG_VALUE_CHARS`, `MAX_TAGS_PER_RESOURCE`)
/// one past it, counted in characters, the key a tag cannot be without, and each
/// character class the service states leaves out ([`REJECTED_TAG_CHARS`]) in both members
/// a tag has.
///
/// Every row is the client error the operation models — what a tag may be is Amazon EMR's
/// own rule, which the model states nowhere, so the service states it rather than the
/// request validator — and none of them carries the absent-resource discriminator.
#[tokio::test]
async fn a_tag_outside_the_documented_bounds_is_refused() {
    let emr = Emr::new();
    let cluster = emr.run_job_flow(kept_alive_request("bounded")).await;
    let held = [pair("stack", Some("Prod"))];
    add_tags_ok(&emr, "the held tag", &cluster, &held).await;

    let over_long_key = "k".repeat(MAX_TAG_KEY_CHARS + 1);
    let over_long_value = "v".repeat(MAX_TAG_VALUE_CHARS + 1);
    // One character past the bound, each character two bytes: the bound is characters.
    let over_long_non_ascii_value = "é".repeat(MAX_TAG_VALUE_CHARS + 1);
    // The sentence an entry that names no tag at all is refused with: there is no value
    // to quote.
    let no_key = "Tag key is not valid.".to_string();

    // Every row: the case, the `Tags` it sends, and the sentence it must state.
    let mut rows: Vec<(String, Value, String)> = vec![
        (
            "a tag with no Key member".to_string(),
            json!([{ "Value": "orphan" }]),
            no_key.clone(),
        ),
        (
            "a tag with an empty Key".to_string(),
            json!([{ "Key": "", "Value": "orphan" }]),
            no_key.clone(),
        ),
        (
            "a good tag beside one with no Key".to_string(),
            json!([{ "Key": "team", "Value": "data" }, { "Value": "orphan" }]),
            no_key,
        ),
        (
            "a Key one character past the maximum".to_string(),
            tag_list(&[pair(&over_long_key, Some("x"))]),
            tag_too_long("key", &over_long_key),
        ),
        (
            "a Value one character past the maximum".to_string(),
            tag_list(&[pair("long", Some(&over_long_value))]),
            tag_too_long("value", &over_long_value),
        ),
        (
            "a non-ASCII Value one character past the maximum".to_string(),
            tag_list(&[pair("длина", Some(&over_long_non_ascii_value))]),
            tag_too_long("value", &over_long_non_ascii_value),
        ),
    ];

    // One call carrying one tag more than a resource may hold: the rule is about the
    // resource rather than the tag, so it states what the resource would have carried.
    let past_quota: Vec<Pair> = (0..=MAX_TAGS_PER_RESOURCE)
        .map(|at| pair(&format!("quota-{at:02}"), Some("x")))
        .collect();
    rows.push((
        "one tag more than a resource carries".to_string(),
        tag_list(&past_quota),
        format!(
            "These tags would leave the resource carrying {} tags; a resource carries at \
             most {MAX_TAGS_PER_RESOURCE} tags",
            past_quota.len() + held.len()
        ),
    ));

    // A character outside the class, in each member it could be in: the sentence names
    // that member and the value it held.
    for rejected in REJECTED_TAG_CHARS {
        let text = format!("no-{rejected}-go");
        rows.push((
            format!("the character {rejected:?} in a Value"),
            tag_list(&[pair("charset", Some(&text))]),
            tag_charset_refusal("value", &text),
        ));
        rows.push((
            format!("the character {rejected:?} in a Key"),
            tag_list(&[pair(&text, Some("x"))]),
            tag_charset_refusal("key", &text),
        ));
    }
    // A bad tag BEHIND a good one is refused the same way, and the good one beside it is
    // not stored either.
    let behind = format!("no-{}-go", REJECTED_TAG_CHARS[0]);
    rows.push((
        "a character outside the class in the second entry".to_string(),
        tag_list(&[pair("team", Some("data")), pair("charset", Some(&behind))]),
        tag_charset_refusal("value", &behind),
    ));

    for (case, tags, says) in rows {
        let (status, payload) = emr
            .call("AddTags", json!({ "ResourceId": &cluster, "Tags": tags }))
            .await;
        assert_refused(&case, status, &payload, INVALID_REQUEST_EXCEPTION);
        assert_eq!(
            error_code(&payload),
            None,
            "case {case:?}: a rejected tag carries no ErrorCode, got {payload}"
        );
        assert_eq!(
            message(&case, &payload),
            says,
            "case {case:?}: unexpected message in {payload}"
        );
        // Nothing of a refused call reached the resource.
        assert_tags(&emr, &case, CLUSTER, &cluster, &held).await;
    }

    // The ceiling itself is reachable: a resource filled to the maximum keeps every tag,
    // and one further NEW key is refused while a re-submitted key still merges.
    let full = emr.run_job_flow(kept_alive_request("full")).await;
    let full_set: Vec<Pair> = (0..MAX_TAGS_PER_RESOURCE)
        .map(|at| pair(&format!("quota-{at:02}"), Some("x")))
        .collect();
    add_tags_ok(&emr, "a resource filled to the maximum", &full, &full_set).await;
    let (status, payload) = emr
        .call(
            "AddTags",
            json!({
                "ResourceId": &full,
                "Tags": tag_list(&[pair("one-too-many", Some("x"))]),
            }),
        )
        .await;
    assert_refused(
        "one new key past a full resource",
        status,
        &payload,
        INVALID_REQUEST_EXCEPTION,
    );
    assert_tags(
        &emr,
        "a full resource after the refusal",
        CLUSTER,
        &full,
        &full_set,
    )
    .await;

    // Replacing a key a full resource already carries adds no entry, so it is accepted.
    let mut replaced = full_set.clone();
    replaced[0] = pair("quota-00", Some("y"));
    add_tags_ok(
        &emr,
        "a replacement on a full resource",
        &full,
        &[pair("quota-00", Some("y"))],
    )
    .await;
    assert_tags(
        &emr,
        "a replacement on a full resource",
        CLUSTER,
        &full,
        &replaced,
    )
    .await;
}

/// A present but EMPTY `Tags` list: the model requires the member and constrains its
/// length nowhere, so the list reaches the service — where it names no tag to associate
/// and is refused, leaving the resource exactly as it was.
#[tokio::test]
async fn an_empty_tag_list_is_refused_and_changes_nothing() {
    let emr = Emr::new();
    let cluster = emr.run_job_flow(kept_alive_request("empty-list")).await;
    let studio = create_studio(&emr, "empty-list").await;

    // The sentence Amazon EMR refuses a tag call carrying no tag with: fixed text,
    // naming neither the resource nor the member's path.
    let says = "Tags cannot be null or empty.";

    for (kind, resource_id) in [(CLUSTER, &cluster), (STUDIO, &studio)] {
        let case = format!("an empty Tags list on a {kind}");
        let (status, payload) = emr
            .call("AddTags", json!({ "ResourceId": resource_id, "Tags": [] }))
            .await;
        assert_refused(&case, status, &payload, INVALID_REQUEST_EXCEPTION);
        assert_eq!(
            message(&case, &payload),
            says,
            "case {case:?}: unexpected message in {payload}"
        );
        assert_tags(&emr, &case, kind, resource_id, &[]).await;
    }

    // A resource that already carries tags keeps exactly them.
    let held = [pair("stack", Some("Prod"))];
    add_tags_ok(&emr, "a tagged cluster", &cluster, &held).await;
    let case = "an empty Tags list on a tagged cluster";
    let (status, payload) = emr
        .call("AddTags", json!({ "ResourceId": &cluster, "Tags": [] }))
        .await;
    assert_refused(case, status, &payload, INVALID_REQUEST_EXCEPTION);
    assert_eq!(
        message(case, &payload),
        says,
        "case {case:?}: unexpected message in {payload}"
    );
    assert_tags(&emr, case, CLUSTER, &cluster, &held).await;

    // The RESOURCE is resolved first: a call that gets both the resource and the list
    // wrong is refused for the resource.
    let case = "an empty Tags list on a cluster id naming no cluster";
    let (status, payload) = emr
        .call(
            "AddTags",
            json!({ "ResourceId": "j-ZZZZZZZZZZZZZ", "Tags": [] }),
        )
        .await;
    assert_refused(case, status, &payload, INVALID_REQUEST_EXCEPTION);
    assert_eq!(
        message(case, &payload),
        "Cluster id 'j-ZZZZZZZZZZZZZ' is not valid.",
        "case {case:?}: the resource is resolved before the list is judged, got {payload}"
    );
}
