// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the tagging surface — `TagResource`, `UntagResource` and
//! `ListTagsForResource` — over the wire, on BOTH of this service's taggable resource
//! kinds (a namespace and a service).
//!
//! The rule under test: a resource's tags are a set of key-value pairs keyed by tag
//! key, addressed by the resource's ARN; tagging SETS the keys it names (replacing a
//! stored value, appending a key that is new) and leaves every other stored tag alone,
//! untagging REMOVES only the keys it names, and the read reports exactly what is
//! stored. Every assertion below is a row of that rule, not a replay of one request.
//!
//! Bounds and quotas cited here come from the model and the docs, named at their use:
//! `Tag$Key` is 1..128 and `Tag$Value` is 0..256
//! (`smithy_query(query="shape", shape="Tag")`), and `TooManyTagsException` documents
//! "The maximum number of tags that can be applied to a resource is 50."

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloud_map_core::{call, emulator, failure, namespace, ok, service};

/// The quota the `TooManyTagsException` documentation states: "The maximum number of
/// tags that can be applied to a resource is 50."
const MAX_TAGS_PER_RESOURCE: usize = 50;

/// `Tag$Key` length, from the model (`smithy_query(query="shape", shape="Tag")`).
const MAX_TAG_KEY_LEN: usize = 128;
/// `Tag$Value` length, from the model (`smithy_query(query="shape", shape="Tag")`).
const MAX_TAG_VALUE_LEN: usize = 256;

/// One namespace ARN and one service ARN, so every rule is asserted on both taggable
/// resource kinds rather than only on the one a recording happened to use.
async fn taggable_arns(router: &MultiServiceRouter, suffix: &str) -> Vec<(&'static str, String)> {
    let (_, namespace_id) = namespace(router, &format!("tag-ns-{suffix}")).await;
    let read = ok(router, "GetNamespace", json!({ "Id": namespace_id })).await;
    let namespace_arn = read["Namespace"]["Arn"]
        .as_str()
        .expect("a stored namespace carries its ARN")
        .to_string();

    let service_id = service(router, &namespace_id, &format!("tag-svc-{suffix}")).await;
    let read = ok(router, "GetService", json!({ "Id": service_id })).await;
    let service_arn = read["Service"]["Arn"]
        .as_str()
        .expect("a stored service carries its ARN")
        .to_string();

    vec![("namespace", namespace_arn), ("service", service_arn)]
}

/// The tags `ListTagsForResource` reports, as a key -> value map.
///
/// The read is issued TWICE and the two bodies compared, because a report must not
/// change what the next caller sees.
async fn listed_tags(router: &MultiServiceRouter, resource_arn: &str) -> BTreeMap<String, String> {
    let first = ok(
        router,
        "ListTagsForResource",
        json!({ "ResourceARN": resource_arn }),
    )
    .await;
    let second = ok(
        router,
        "ListTagsForResource",
        json!({ "ResourceARN": resource_arn }),
    )
    .await;
    assert_eq!(
        first, second,
        "reading tags of {resource_arn} must not change them: {first} then {second}"
    );
    first["Tags"]
        .as_array()
        .expect("Tags is a list on a resource that exists")
        .iter()
        .map(|tag| {
            (
                tag["Key"]
                    .as_str()
                    .expect("a stored tag carries its key")
                    .to_string(),
                tag["Value"]
                    .as_str()
                    .expect("a stored tag carries its value")
                    .to_string(),
            )
        })
        .collect()
}

/// A `TagList` wire value for the given pairs.
fn tag_list<'a>(pairs: impl IntoIterator<Item = (&'a str, &'a str)>) -> Value {
    Value::Array(
        pairs
            .into_iter()
            .map(|(key, value)| json!({ "Key": key, "Value": value }))
            .collect(),
    )
}

/// A tag's key and value round-trip byte for byte, whatever characters they are made
/// of and whatever their length inside the modelled bounds.
#[tokio::test]
async fn a_tag_round_trips_byte_for_byte_through_list_tags_for_resource() {
    let router = emulator();

    // Rows differ along what the rule generalises over: the character repertoire of
    // the key and the value, and the length of each, including the one case the docs
    // call out ("You can set the value of a tag to an empty string").
    let cases: Vec<(&str, String, String)> = vec![
        ("ascii", "stage".to_string(), "prod".to_string()),
        (
            "an empty value, which the Tag$Value docs allow",
            "unset".to_string(),
            String::new(),
        ),
        ("emoji", "mood".to_string(), "🚀🌍".to_string()),
        ("cjk", "名前".to_string(), "本番環境".to_string()),
        (
            "right-to-left script",
            "اسم".to_string(),
            "قيمة".to_string(),
        ),
        (
            "combining marks",
            "accent".to_string(),
            "e\u{0301}cole".to_string(),
        ),
        (
            "punctuation and surrounding spaces",
            "note".to_string(),
            " a: b/c_d.e+f-g ".to_string(),
        ),
        (
            "a key at the modelled maximum length",
            "k".repeat(MAX_TAG_KEY_LEN),
            "at-the-key-bound".to_string(),
        ),
        (
            "a value at the modelled maximum length",
            "max-value".to_string(),
            "v".repeat(MAX_TAG_VALUE_LEN),
        ),
    ];

    for (kind, arn) in taggable_arns(&router, "roundtrip").await {
        assert!(
            listed_tags(&router, &arn).await.is_empty(),
            "a freshly created {kind} carries no tags"
        );

        let mut expected: BTreeMap<String, String> = BTreeMap::new();
        for (case, key, value) in &cases {
            ok(
                &router,
                "TagResource",
                json!({
                    "ResourceARN": arn,
                    "Tags": tag_list([(key.as_str(), value.as_str())]),
                }),
            )
            .await;
            expected.insert(key.clone(), value.clone());

            let listed = listed_tags(&router, &arn).await;
            assert_eq!(
                listed.get(key).map(String::as_str),
                Some(value.as_str()),
                "case {case:?} on a {kind}: expected the stored value {value:?}, \
                 got {:?}",
                listed.get(key)
            );
            assert_eq!(
                listed, expected,
                "case {case:?} on a {kind}: tagging must add to the stored set, \
                 expected {expected:?}, got {listed:?}"
            );
        }
    }
}

/// Tagging merges by key and untagging removes only the keys it names.
#[tokio::test]
async fn tagging_merges_by_key_and_untagging_removes_only_the_keys_it_names() {
    let router = emulator();

    for (kind, arn) in taggable_arns(&router, "merge").await {
        // Two tags to start from.
        ok(
            &router,
            "TagResource",
            json!({ "ResourceARN": arn, "Tags": tag_list([("a", "1"), ("b", "2")]) }),
        )
        .await;

        // A second call REPLACES the value of a key it names and APPENDS one it does
        // not, leaving every other stored tag alone.
        let steps: Vec<(&str, Value, BTreeMap<String, String>)> = vec![
            (
                "overwrite one key and add another",
                tag_list([("b", "3"), ("c", "4")]),
                BTreeMap::from([
                    ("a".to_string(), "1".to_string()),
                    ("b".to_string(), "3".to_string()),
                    ("c".to_string(), "4".to_string()),
                ]),
            ),
            (
                "a key repeated inside one request keeps the last value given",
                tag_list([("b", "5"), ("b", "6")]),
                BTreeMap::from([
                    ("a".to_string(), "1".to_string()),
                    ("b".to_string(), "6".to_string()),
                    ("c".to_string(), "4".to_string()),
                ]),
            ),
            (
                "an empty tag list changes nothing",
                tag_list([]),
                BTreeMap::from([
                    ("a".to_string(), "1".to_string()),
                    ("b".to_string(), "6".to_string()),
                    ("c".to_string(), "4".to_string()),
                ]),
            ),
            (
                "setting a key to the value it already carries is a no-op",
                tag_list([("a", "1")]),
                BTreeMap::from([
                    ("a".to_string(), "1".to_string()),
                    ("b".to_string(), "6".to_string()),
                    ("c".to_string(), "4".to_string()),
                ]),
            ),
        ];
        for (case, tags, expected) in steps {
            ok(
                &router,
                "TagResource",
                json!({ "ResourceARN": arn, "Tags": tags }),
            )
            .await;
            let listed = listed_tags(&router, &arn).await;
            assert_eq!(
                listed, expected,
                "case {case:?} on a {kind}: expected {expected:?}, got {listed:?}"
            );
        }

        let removals: Vec<(&str, Value, BTreeMap<String, String>)> = vec![
            (
                "remove one stored key alongside a key the resource never carried",
                json!(["b", "nosuchkey"]),
                BTreeMap::from([
                    ("a".to_string(), "1".to_string()),
                    ("c".to_string(), "4".to_string()),
                ]),
            ),
            (
                "removing the same key again removes nothing and is not a failure",
                json!(["b"]),
                BTreeMap::from([
                    ("a".to_string(), "1".to_string()),
                    ("c".to_string(), "4".to_string()),
                ]),
            ),
            (
                "an empty key list changes nothing",
                json!([]),
                BTreeMap::from([
                    ("a".to_string(), "1".to_string()),
                    ("c".to_string(), "4".to_string()),
                ]),
            ),
            (
                "remove every remaining key",
                json!(["a", "c"]),
                BTreeMap::new(),
            ),
        ];
        for (case, tag_keys, expected) in removals {
            ok(
                &router,
                "UntagResource",
                json!({ "ResourceARN": arn, "TagKeys": tag_keys }),
            )
            .await;
            let listed = listed_tags(&router, &arn).await;
            assert_eq!(
                listed, expected,
                "case {case:?} on a {kind}: expected {expected:?}, got {listed:?}"
            );
        }

        // And a resource whose tags were all removed reports an EMPTY list, not an
        // absent one: it still exists.
        let read = ok(
            &router,
            "ListTagsForResource",
            json!({ "ResourceARN": arn }),
        )
        .await;
        assert_eq!(
            read["Tags"],
            json!([]),
            "an untagged {kind} reports an empty tag list, got {read}"
        );
    }
}

/// Tags given at create time are the resource's stored tags, so a later tagging call
/// merges onto them instead of starting from nothing.
#[tokio::test]
async fn tags_set_at_create_time_are_what_the_read_reports() {
    let router = emulator();

    let created = ok(
        &router,
        "CreateHttpNamespace",
        json!({
            "Name": "created-with-tags-ns",
            "Tags": tag_list([("owner", "platform"), ("stage", "dev")]),
        }),
    )
    .await;
    let operation_id = created["OperationId"].as_str().expect("an operation id");
    let operation = ok(
        &router,
        "GetOperation",
        json!({ "OperationId": operation_id }),
    )
    .await;
    let namespace_id = operation["Operation"]["Targets"]["NAMESPACE"]
        .as_str()
        .expect("the CREATE_NAMESPACE operation targets its namespace")
        .to_string();
    let read = ok(&router, "GetNamespace", json!({ "Id": namespace_id })).await;
    let namespace_arn = read["Namespace"]["Arn"].as_str().expect("an ARN").to_string();

    let created = ok(
        &router,
        "CreateService",
        json!({
            "Name": "created-with-tags-svc",
            "NamespaceId": namespace_id,
            "Tags": tag_list([("owner", "platform"), ("stage", "dev")]),
        }),
    )
    .await;
    let service_arn = created["Service"]["Arn"].as_str().expect("an ARN").to_string();

    for (kind, arn) in [("namespace", namespace_arn), ("service", service_arn)] {
        let listed = listed_tags(&router, &arn).await;
        assert_eq!(
            listed,
            BTreeMap::from([
                ("owner".to_string(), "platform".to_string()),
                ("stage".to_string(), "dev".to_string()),
            ]),
            "case {kind:?}: the tags the create stored are what the read reports, \
             got {listed:?}"
        );

        ok(
            &router,
            "TagResource",
            json!({ "ResourceARN": arn, "Tags": tag_list([("stage", "prod"), ("team", "maps")]) }),
        )
        .await;
        let listed = listed_tags(&router, &arn).await;
        assert_eq!(
            listed,
            BTreeMap::from([
                ("owner".to_string(), "platform".to_string()),
                ("stage".to_string(), "prod".to_string()),
                ("team".to_string(), "maps".to_string()),
            ]),
            "case {kind:?}: tagging merges onto the create-time tags, got {listed:?}"
        );
    }
}

/// The quota is on the tags the RESOURCE ends up carrying, and a refused call stores
/// nothing.
#[tokio::test]
async fn the_tag_quota_counts_the_merged_set_and_a_refused_call_stores_nothing() {
    let router = emulator();

    for (kind, arn) in taggable_arns(&router, "quota").await {
        let keys: Vec<String> = (0..MAX_TAGS_PER_RESOURCE)
            .map(|index| format!("k{index:02}"))
            .collect();
        let at_quota = tag_list(keys.iter().map(|key| (key.as_str(), "stored")));
        ok(
            &router,
            "TagResource",
            json!({ "ResourceARN": arn, "Tags": at_quota }),
        )
        .await;
        let listed = listed_tags(&router, &arn).await;
        assert_eq!(
            listed.len(),
            MAX_TAGS_PER_RESOURCE,
            "a {kind} holds the quota of {MAX_TAGS_PER_RESOURCE} tags, got {listed:?}"
        );

        // One tag past the quota is refused, and the stored set is untouched.
        let (status, code) = failure(
            &router,
            "TagResource",
            json!({ "ResourceARN": arn, "Tags": tag_list([("one-too-many", "x")]) }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "TooManyTagsException"),
            "case \"one tag past the quota\" on a {kind}: expected 400 TooManyTagsException"
        );
        assert_eq!(
            listed_tags(&router, &arn).await,
            listed,
            "case \"one tag past the quota\" on a {kind}: a refused call stores nothing"
        );

        // Replacing the value of a key already stored grows nothing, so the quota does
        // not refuse it even at the bound.
        ok(
            &router,
            "TagResource",
            json!({ "ResourceARN": arn, "Tags": tag_list([(keys[0].as_str(), "replaced")]) }),
        )
        .await;
        let after_replace = listed_tags(&router, &arn).await;
        assert_eq!(
            after_replace.get(&keys[0]).map(String::as_str),
            Some("replaced"),
            "case \"replace a key at the quota\" on a {kind}: expected the new value, \
             got {after_replace:?}"
        );
        assert_eq!(
            after_replace.len(),
            MAX_TAGS_PER_RESOURCE,
            "case \"replace a key at the quota\" on a {kind}: replacing adds no tag, \
             got {after_replace:?}"
        );

        // Freeing one slot makes room for exactly one more tag.
        ok(
            &router,
            "UntagResource",
            json!({ "ResourceARN": arn, "TagKeys": [keys[0].as_str()] }),
        )
        .await;
        ok(
            &router,
            "TagResource",
            json!({ "ResourceARN": arn, "Tags": tag_list([("after-untag", "x")]) }),
        )
        .await;
        let refilled = listed_tags(&router, &arn).await;
        assert_eq!(
            refilled.get("after-untag").map(String::as_str),
            Some("x"),
            "case \"refill a freed slot\" on a {kind}: expected the new tag, got {refilled:?}"
        );
        assert_eq!(
            refilled.len(),
            MAX_TAGS_PER_RESOURCE,
            "case \"refill a freed slot\" on a {kind}: expected the quota again, \
             got {refilled:?}"
        );
    }

    // A single call that would land one tag past the quota on an untagged resource is
    // refused whole.
    for (kind, arn) in taggable_arns(&router, "quota-one-call").await {
        let over: Vec<String> = (0..=MAX_TAGS_PER_RESOURCE)
            .map(|index| format!("k{index:02}"))
            .collect();
        let (status, code) = failure(
            &router,
            "TagResource",
            json!({
                "ResourceARN": arn,
                "Tags": tag_list(over.iter().map(|key| (key.as_str(), "x"))),
            }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "TooManyTagsException"),
            "case \"one call past the quota\" on a {kind}: expected 400 TooManyTagsException"
        );
        assert!(
            listed_tags(&router, &arn).await.is_empty(),
            "case \"one call past the quota\" on a {kind}: the refused call stored nothing"
        );
    }
}

/// Every tagging operation answers `ResourceNotFoundException` for an ARN that names
/// no stored resource.
#[tokio::test]
async fn an_arn_naming_no_stored_resource_is_not_found() {
    let router = emulator();

    // Each unknown ARN is the real one with its resource id swapped, so the test does
    // not hand-write the ARN format.
    let mut unknown: Vec<(String, String)> = Vec::new();
    for (kind, arn) in taggable_arns(&router, "notfound").await {
        let (prefix, id) = arn
            .rsplit_once('/')
            .expect("a Cloud Map ARN names its resource after a '/'");
        unknown.push((
            format!("{kind} ARN naming nothing"),
            format!("{prefix}/{id}-nosuchresource"),
        ));
    }

    for (case, arn) in unknown {
        let requests = [
            ("TagResource", json!({ "ResourceARN": arn, "Tags": tag_list([("a", "1")]) })),
            ("UntagResource", json!({ "ResourceARN": arn, "TagKeys": ["a"] })),
            ("ListTagsForResource", json!({ "ResourceARN": arn })),
        ];
        for (operation, body) in requests {
            let (status, code) = failure(&router, operation, body.clone()).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "ResourceNotFoundException"),
                "case {case:?} on {operation} with {body}: \
                 expected 400 ResourceNotFoundException"
            );
        }
    }
}

/// A tag key or value outside the modelled lengths is refused, and one at each
/// published bound is accepted.
#[tokio::test]
async fn tag_keys_and_values_outside_the_modelled_lengths_are_refused() {
    let router = emulator();
    let arn = taggable_arns(&router, "lengths").await[0].1.clone();

    // Bounds from the model: Tag$Key 1..128, Tag$Value 0..256.
    let cases: Vec<(&str, String, String, bool)> = vec![
        ("key at the minimum length", "k".to_string(), "v".to_string(), true),
        (
            "key at the maximum length",
            "k".repeat(MAX_TAG_KEY_LEN),
            "v".to_string(),
            true,
        ),
        (
            "key one past the maximum length",
            "k".repeat(MAX_TAG_KEY_LEN + 1),
            "v".to_string(),
            false,
        ),
        (
            "key one below the minimum length",
            String::new(),
            "v".to_string(),
            false,
        ),
        (
            "value at the minimum length",
            "min-value".to_string(),
            String::new(),
            true,
        ),
        (
            "value at the maximum length",
            "max-value".to_string(),
            "v".repeat(MAX_TAG_VALUE_LEN),
            true,
        ),
        (
            "value one past the maximum length",
            "over-value".to_string(),
            "v".repeat(MAX_TAG_VALUE_LEN + 1),
            false,
        ),
    ];

    for (case, key, value, accepted) in cases {
        let body = json!({
            "ResourceARN": arn,
            "Tags": tag_list([(key.as_str(), value.as_str())]),
        });
        let (status, json) = call(&router, "TagResource", body).await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?} (key {} chars, value {} chars): expected success, got {status} {json}",
                key.chars().count(),
                value.chars().count()
            );
            let listed = listed_tags(&router, &arn).await;
            assert_eq!(
                listed.get(&key).map(String::as_str),
                Some(value.as_str()),
                "case {case:?}: the accepted tag is stored, got {listed:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?} (key {} chars, value {} chars): a length outside the \
                 modelled bound is refused, got {status} {json}",
                key.chars().count(),
                value.chars().count()
            );
            assert!(
                !listed_tags(&router, &arn).await.contains_key(&key),
                "case {case:?}: a refused tag is not stored"
            );
        }
    }
}
