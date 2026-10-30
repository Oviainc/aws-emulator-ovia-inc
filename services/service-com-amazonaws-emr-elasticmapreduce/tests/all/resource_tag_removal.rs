// SPDX-License-Identifier: CC0-1.0

//! Taking tags OFF an Amazon EMR resource, end to end over the wire.
//!
//! Every case drives the real HTTP path — `POST /` with
//! `X-Amz-Target: ElasticMapReduce.<Op>` and an `awsJson1_1` body — through the
//! router's `dispatch`, using the harness `tests/all/job_flow_lifecycle.rs` owns and the
//! tag helpers `tests/all/resource_tags.rs` owns, so these are the same bytes an SDK
//! client sends. Nothing reaches into the service's state struct: a tag set is only ever
//! written through `AddTags` and read back through `DescribeCluster` or `DescribeStudio`,
//! which is what makes these tests evidence that the responses are computed from
//! persisted state.
//!
//! THE RULE every table below asserts: `RemoveTags` takes the tags its `TagKeys` name off
//! the tag set of the Amazon EMR resource its `ResourceId` names — a key names that
//! resource's tag of that key whatever value the tag holds, a key the resource carries no
//! tag for takes nothing off, every tag beside the named ones keeps its value, and a
//! resource left carrying none reports no tags at all — and refuses an id naming no
//! resource this account holds, changing nothing.

use serde_json::{json, Value};
use service_com_amazonaws_emr_elasticmapreduce::state::{
    MAX_TAGS_PER_RESOURCE, MAX_TAG_KEY_CHARS, MAX_TAG_VALUE_CHARS, TAG_PUNCTUATION,
};

use crate::job_flow_lifecycle::{assert_refused, kept_alive_request, Emr};
use crate::refusal_messages::message;
use crate::resource_tags::{
    add_tags_ok, assert_tags, create_studio, error_code, pair, Pair, CLUSTER,
    INVALID_REQUEST_EXCEPTION, STUDIO,
};

/// `RemoveTags` on `resource_id`, which must succeed and answer a body with no members.
async fn remove_tags_ok(emr: &Emr, case: &str, resource_id: &str, keys: &[String]) {
    let out = emr
        .ok(
            "RemoveTags",
            json!({ "ResourceId": resource_id, "TagKeys": keys }),
        )
        .await;
    assert!(
        out.as_object().is_some_and(serde_json::Map::is_empty),
        "case {case:?}: RemoveTags answers a body with no members, got {out}"
    );
}

/// Tag keys as the wire carries them.
fn keys(keys: &[&str]) -> Vec<String> {
    keys.iter().map(|key| (*key).to_string()).collect()
}

/// One row: the tags the resource is given, the `RemoveTags` calls made against it in
/// order, and the tag set it carries once they have all been made.
struct Case {
    label: &'static str,
    held: Vec<Pair>,
    removals: Vec<Vec<String>>,
    expected: Vec<Pair>,
}

/// The table, whose rows differ along what the rule generalises over: the COUNT of keys
/// one call names (none, one, several, every key held) and of tags the resource carries
/// (one, several, the documented maximum), the FORM of a key (ASCII, non-ASCII, every
/// symbol a tag may carry, at the documented maximum length, differing from a held key
/// only in case), whether the key names a tag the resource HOLDS, and the REPETITION of a
/// key within one call and across two.
fn cases() -> Vec<Case> {
    // SOURCE for the key bound: `AddTags$Tags` ("a required key string with a maximum of
    // 128 characters"), exported as `MAX_TAG_KEY_CHARS`; and for the symbols, the tag
    // character class the service states, exported as `TAG_PUNCTUATION`.
    let longest_key = "k".repeat(MAX_TAG_KEY_CHARS);
    let punctuated: String = format!("cost{}centre", TAG_PUNCTUATION.iter().collect::<String>());

    // The documented maximum number of tags one resource carries, each a distinct key.
    let full_set: Vec<Pair> = (0..MAX_TAGS_PER_RESOURCE)
        .map(|at| pair(&format!("quota-{at:02}"), Some(&format!("value-{at:02}"))))
        .collect();
    // Half of them removed in one call: the rest keep their own values.
    let half = MAX_TAGS_PER_RESOURCE / 2;
    let removed_half: Vec<String> = full_set
        .iter()
        .take(half)
        .map(|(key, _)| key.clone())
        .collect();
    let kept_half: Vec<Pair> = full_set.iter().skip(half).cloned().collect();

    let three = vec![
        pair("stack", Some("Prod")),
        pair("team", Some("data")),
        pair("owner", Some("ops")),
    ];

    vec![
        Case {
            label: "the only tag the resource carries",
            held: vec![pair("stack", Some("Prod"))],
            removals: vec![keys(&["stack"])],
            expected: vec![],
        },
        Case {
            label: "one key of several",
            held: three.clone(),
            removals: vec![keys(&["team"])],
            expected: vec![pair("owner", Some("ops")), pair("stack", Some("Prod"))],
        },
        Case {
            label: "several keys in one call",
            held: three.clone(),
            removals: vec![keys(&["stack", "owner"])],
            expected: vec![pair("team", Some("data"))],
        },
        Case {
            label: "every key the resource carries",
            held: three.clone(),
            removals: vec![keys(&["stack", "team", "owner"])],
            expected: vec![],
        },
        Case {
            label: "one key per call",
            held: three.clone(),
            removals: vec![keys(&["stack"]), keys(&["owner"])],
            expected: vec![pair("team", Some("data"))],
        },
        Case {
            label: "a key the resource carries no tag for",
            held: vec![pair("stack", Some("Prod"))],
            removals: vec![keys(&["absent"])],
            expected: vec![pair("stack", Some("Prod"))],
        },
        Case {
            label: "a held key beside one the resource carries no tag for",
            held: vec![pair("stack", Some("Prod")), pair("team", Some("data"))],
            removals: vec![keys(&["team", "absent"])],
            expected: vec![pair("stack", Some("Prod"))],
        },
        Case {
            label: "no keys at all",
            held: vec![pair("stack", Some("Prod"))],
            removals: vec![keys(&[])],
            expected: vec![pair("stack", Some("Prod"))],
        },
        Case {
            label: "the same key twice in one call",
            held: vec![pair("stack", Some("Prod")), pair("team", Some("data"))],
            removals: vec![keys(&["stack", "stack"])],
            expected: vec![pair("team", Some("data"))],
        },
        Case {
            label: "the same key removed in two calls",
            held: vec![pair("stack", Some("Prod")), pair("team", Some("data"))],
            removals: vec![keys(&["stack"]), keys(&["stack"])],
            expected: vec![pair("team", Some("data"))],
        },
        Case {
            label: "a non-ASCII key",
            held: vec![
                pair("окружение", Some("производство")),
                pair("stack", Some("Prod")),
            ],
            removals: vec![keys(&["окружение"])],
            expected: vec![pair("stack", Some("Prod"))],
        },
        Case {
            label: "a key carrying every symbol a tag may carry",
            held: vec![pair(&punctuated, Some("4711")), pair("stack", Some("Prod"))],
            removals: vec![vec![punctuated.clone()]],
            expected: vec![pair("stack", Some("Prod"))],
        },
        Case {
            label: "a key of the documented maximum length",
            held: vec![pair(&longest_key, Some("x")), pair("stack", Some("Prod"))],
            removals: vec![vec![longest_key.clone()]],
            expected: vec![pair("stack", Some("Prod"))],
        },
        Case {
            label: "a key differing from the held one only in case",
            held: vec![pair("Stack", Some("Prod"))],
            removals: vec![keys(&["stack"])],
            expected: vec![pair("Stack", Some("Prod"))],
        },
        Case {
            label: "a tag whose value is absent",
            held: vec![pair("placeholder", None), pair("stack", Some("Prod"))],
            removals: vec![keys(&["placeholder"])],
            expected: vec![pair("stack", Some("Prod"))],
        },
        Case {
            label: "a tag whose value is empty",
            held: vec![pair("audited", Some("")), pair("stack", Some("Prod"))],
            removals: vec![keys(&["audited"])],
            expected: vec![pair("stack", Some("Prod"))],
        },
        Case {
            label: "half of the documented maximum number of tags",
            held: full_set,
            removals: vec![removed_half],
            expected: kept_half,
        },
    ]
}

// ---------------------------------------------------------------------------
// AddTags -> RemoveTags -> DescribeCluster / DescribeStudio
// ---------------------------------------------------------------------------

/// THE RULE: every row's removals leave the resource carrying exactly the remaining tag
/// set, for both resource kinds the operation names ("a cluster identifier or an Amazon
/// EMR Studio ID"), and a read reports it unchanged however often it is asked.
#[tokio::test]
async fn remove_tags_takes_the_named_keys_off_and_the_describe_reports_what_is_left() {
    let emr = Emr::new();

    for case in cases() {
        let label = case.label;
        // A fresh cluster and a fresh Studio per row, so this row's calls are the only
        // tags either of them ever carried.
        let cluster = emr.run_job_flow(kept_alive_request(label)).await;
        let studio = create_studio(&emr, label).await;

        for (kind, resource_id) in [(CLUSTER, &cluster), (STUDIO, &studio)] {
            let case_name = format!("{label} on a {kind}");
            add_tags_ok(&emr, &case_name, resource_id, &case.held).await;
            assert_tags(
                &emr,
                &format!("{case_name}, before the removal"),
                kind,
                resource_id,
                &case.held,
            )
            .await;

            for removal in &case.removals {
                remove_tags_ok(&emr, &case_name, resource_id, removal).await;
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

/// THE RULE round trip: a key taken off is gone from the resource, and adding it again
/// puts it back with the value the second `AddTags` submitted — so removal takes the tag
/// off the resource rather than hiding it from a read.
#[tokio::test]
async fn a_removed_tag_can_be_added_again() {
    let emr = Emr::new();
    let cluster = emr.run_job_flow(kept_alive_request("re-added")).await;
    let studio = create_studio(&emr, "re-added").await;

    for (kind, resource_id) in [(CLUSTER, &cluster), (STUDIO, &studio)] {
        let case = format!("a re-added tag on a {kind}");
        add_tags_ok(&emr, &case, resource_id, &[pair("stack", Some("Prod"))]).await;
        remove_tags_ok(&emr, &case, resource_id, &keys(&["stack"])).await;
        assert_tags(&emr, &format!("{case}, removed"), kind, resource_id, &[]).await;

        add_tags_ok(&emr, &case, resource_id, &[pair("stack", Some("Staging"))]).await;
        assert_tags(
            &emr,
            &format!("{case}, added again"),
            kind,
            resource_id,
            &[pair("stack", Some("Staging"))],
        )
        .await;
    }
}

/// THE RULE's scope: a key is taken off the resource the request names and off no other,
/// so every other resource this account holds keeps the tag of that key.
#[tokio::test]
async fn remove_tags_leaves_every_other_resource_alone() {
    let emr = Emr::new();
    let named = emr.run_job_flow(kept_alive_request("named")).await;
    let other_cluster = emr.run_job_flow(kept_alive_request("other")).await;
    let named_studio = create_studio(&emr, "named-studio").await;
    let other_studio = create_studio(&emr, "other-studio").await;

    let held = [pair("stack", Some("Prod")), pair("team", Some("data"))];
    for resource_id in [&named, &other_cluster, &named_studio, &other_studio] {
        add_tags_ok(&emr, "every resource", resource_id, &held).await;
    }

    remove_tags_ok(&emr, "the named cluster", &named, &keys(&["stack"])).await;
    remove_tags_ok(&emr, "the named Studio", &named_studio, &keys(&["stack"])).await;

    let left = [pair("team", Some("data"))];
    assert_tags(&emr, "the named cluster", CLUSTER, &named, &left).await;
    assert_tags(&emr, "the named Studio", STUDIO, &named_studio, &left).await;
    assert_tags(&emr, "another cluster", CLUSTER, &other_cluster, &held).await;
    assert_tags(&emr, "another Studio", STUDIO, &other_studio, &held).await;
}

// ---------------------------------------------------------------------------
// ClusterId: what scopes the tag operation
// ---------------------------------------------------------------------------

/// THE RULE for `ClusterId`, "the ID of the cluster that scopes the tag operation": a
/// request carrying one is about that cluster or something that lives on it, so the
/// cluster itself is untagged through its own id, a `ClusterId` naming no cluster is
/// refused, and a `ResourceId` naming neither the cluster nor a session of it is refused.
/// An empty `ClusterId` scopes nothing, so the `ResourceId` is resolved on its own.
#[tokio::test]
async fn cluster_id_scopes_the_tag_removal() {
    let emr = Emr::new();
    let cluster = emr.run_job_flow(kept_alive_request("scoped")).await;
    let held = [
        pair("stack", Some("Prod")),
        pair("team", Some("data")),
        pair("owner", Some("ops")),
    ];
    add_tags_ok(&emr, "the scoped cluster", &cluster, &held).await;

    // The scoping cluster untagged through its own id.
    emr.ok(
        "RemoveTags",
        json!({
            "ClusterId": &cluster,
            "ResourceId": &cluster,
            "TagKeys": keys(&["stack"]),
        }),
    )
    .await;
    assert_tags(
        &emr,
        "the scoping cluster as the resource",
        CLUSTER,
        &cluster,
        &[pair("owner", Some("ops")), pair("team", Some("data"))],
    )
    .await;

    // An empty `ClusterId` scopes nothing: the resource is resolved on its own.
    emr.ok(
        "RemoveTags",
        json!({
            "ClusterId": "",
            "ResourceId": &cluster,
            "TagKeys": keys(&["owner"]),
        }),
    )
    .await;
    assert_tags(
        &emr,
        "an empty ClusterId",
        CLUSTER,
        &cluster,
        &[pair("team", Some("data"))],
    )
    .await;

    // A `ClusterId` this account holds no cluster for: the cluster-id sentence.
    for unknown in ["j-ZZZZZZZZZZZZZ", "j-", "not-a-cluster-id"] {
        let case = format!("ClusterId {unknown:?}");
        let (status, payload) = emr
            .call(
                "RemoveTags",
                json!({
                    "ClusterId": unknown,
                    "ResourceId": &cluster,
                    "TagKeys": keys(&["team"]),
                }),
            )
            .await;
        assert_refused(&case, status, &payload, INVALID_REQUEST_EXCEPTION);
        assert_eq!(
            message(&case, &payload),
            format!("Cluster id '{unknown}' is not valid."),
            "case {case:?}: unexpected message in {payload}"
        );
    }

    // A `ResourceId` that is neither the scoping cluster nor a session of it: the
    // session sentence, naming both ids.
    for unknown in ["s-SESSIONZZZZZ", "es-NOPE", ""] {
        let case = format!("ResourceId {unknown:?} scoped to a cluster");
        let (status, payload) = emr
            .call(
                "RemoveTags",
                json!({
                    "ClusterId": &cluster,
                    "ResourceId": unknown,
                    "TagKeys": keys(&["team"]),
                }),
            )
            .await;
        assert_refused(&case, status, &payload, INVALID_REQUEST_EXCEPTION);
        assert_eq!(
            message(&case, &payload),
            format!("Session id '{unknown}' is not valid or does not exist on cluster '{cluster}'."),
            "case {case:?}: unexpected message in {payload}"
        );
    }

    // Every refusal above left the cluster's tags exactly as the two accepted calls did.
    assert_tags(
        &emr,
        "the scoped cluster after the refusals",
        CLUSTER,
        &cluster,
        &[pair("team", Some("data"))],
    )
    .await;
}

// ---------------------------------------------------------------------------
// What RemoveTags refuses
// ---------------------------------------------------------------------------

/// THE OTHER HALF of the rule: a `ResourceId` naming no resource this account holds is
/// refused — with the sentence the surface that owns an id of that form answers — and
/// nothing is taken off any resource. Rows span the id forms a caller sends, including a
/// Studio-shaped id, and both a never-created and a never-creatable id.
#[tokio::test]
async fn an_unresolvable_resource_id_is_refused() {
    let emr = Emr::new();
    // A resource EXISTS throughout, so every refusal below is a lookup rather than a
    // blanket refusal of the operation.
    let held_cluster = emr.run_job_flow(kept_alive_request("held")).await;
    let held = [pair("stack", Some("Prod"))];
    add_tags_ok(&emr, "the held cluster", &held_cluster, &held).await;

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
                "RemoveTags",
                json!({ "ResourceId": unknown, "TagKeys": keys(&["stack"]) }),
            )
            .await;
        assert_refused(&case, status, &payload, INVALID_REQUEST_EXCEPTION);
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
                "RemoveTags",
                json!({ "ResourceId": unknown, "TagKeys": keys(&["stack"]) }),
            )
            .await;
        assert_refused(&case, status, &payload, INVALID_REQUEST_EXCEPTION);
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
        &held_cluster,
        &held,
    )
    .await;
}

/// A tag key no tag of this service could carry is not a bad request: `TagKeys` is "a
/// list of tag keys to remove from the resource", so such a key names no tag of the
/// resource and takes nothing off, and the call succeeds rather than being refused for
/// something the model admits.
#[tokio::test]
async fn a_key_no_tag_could_carry_takes_nothing_off_and_succeeds() {
    let emr = Emr::new();
    let cluster = emr.run_job_flow(kept_alive_request("odd-keys")).await;
    let held = [pair("stack", Some("Prod"))];
    add_tags_ok(&emr, "the held tag", &cluster, &held).await;

    // One key past the documented maximum, a key outside the tag character class, and
    // the empty string: none of them names a tag any resource of this service holds.
    let past_the_bound = "k".repeat(MAX_TAG_KEY_CHARS + 1);
    let outside_the_class = "no-🚀-go".to_string();
    for key in [past_the_bound, outside_the_class, String::new()] {
        let case = format!("TagKeys [{key:?}]");
        remove_tags_ok(&emr, &case, &cluster, &[key]).await;
        assert_tags(&emr, &case, CLUSTER, &cluster, &held).await;
    }
}

/// What a `Value` is does not decide what a key names: a key removes the resource's tag
/// of that key whatever value it holds, so the same key removes the same tag however the
/// value was submitted.
#[tokio::test]
async fn a_key_names_its_tag_whatever_value_it_holds() {
    let emr = Emr::new();
    // The value forms a tag may hold: ASCII, empty, absent, non-ASCII, and one at the
    // documented maximum length (`AddTags$Tags`, exported as `MAX_TAG_VALUE_CHARS`).
    let values: Vec<Option<String>> = vec![
        Some("Prod".to_string()),
        Some(String::new()),
        None,
        Some("производство".to_string()),
        Some("v".repeat(MAX_TAG_VALUE_CHARS)),
    ];

    for value in values {
        let case = format!("the value {value:?}");
        let cluster = emr.run_job_flow(kept_alive_request("valued")).await;
        add_tags_ok(
            &emr,
            &case,
            &cluster,
            &[
                pair("stack", value.as_deref()),
                pair("team", Some("data")),
            ],
        )
        .await;
        remove_tags_ok(&emr, &case, &cluster, &keys(&["stack"])).await;
        assert_tags(
            &emr,
            &case,
            CLUSTER,
            &cluster,
            &[pair("team", Some("data"))],
        )
        .await;
    }
}

/// `RemoveTags` answers a body with no members: the model declares `RemoveTagsOutput`
/// with none, so a caller reads the removal from a later describe rather than from the
/// response.
#[tokio::test]
async fn remove_tags_answers_an_empty_body() {
    let emr = Emr::new();
    let cluster = emr.run_job_flow(kept_alive_request("empty-body")).await;
    add_tags_ok(&emr, "an empty body", &cluster, &[pair("stack", Some("Prod"))]).await;

    let out: Value = emr
        .ok(
            "RemoveTags",
            json!({ "ResourceId": &cluster, "TagKeys": keys(&["stack"]) }),
        )
        .await;
    assert_eq!(
        out,
        json!({}),
        "case \"an empty body\": RemoveTags answers no members, got {out}"
    );
}
