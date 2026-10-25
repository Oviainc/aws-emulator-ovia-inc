// SPDX-License-Identifier: CC0-1.0

//! The `TagSpecifications` member of a create request, over both creates that model
//! one.
//!
//! The rule: a create's stored tag set is the union of every caller tag the request
//! carries for the resource it creates — the tags of each `TagSpecification` whose
//! `ResourceType` names that resource's type or names none at all, plus the tags of a
//! top-level `Tags` member where the operation has one — and a specification naming
//! another resource type contributes nothing. A tag key in the reserved namespace is
//! refused whichever specification carried it, and the merged set may never exceed
//! what the modelled `TagList` can report.
//!
//! Every row goes through the router and is read back out of the store —
//! `ListTagsForResource` for an instance, the service's own state for a volume, which
//! no operation reports yet.
//!
//! The wire helpers come from the sibling `workspace_instance_lifecycle` suite.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_workspacesinstances_eucmifrontendapiservice as wsinst;

use crate::workspace_instance_lifecycle::{
    call, create, error_code, harness, list_all_ids, ok, router, stored_tags,
};

/// A `TagList` body from pairs.
fn tags(pairs: &[(&str, &str)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(key, value)| json!({ "Key": key, "Value": value }))
            .collect(),
    )
}

/// The expected tag map from pairs.
fn expected(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// One `CreateWorkspaceInstance` request, and the tag set the instance it creates
/// must report afterwards.
struct Row {
    case: &'static str,
    /// The `ManagedInstance.TagSpecifications` list the call sends, if any.
    specifications: Option<Value>,
    /// The top-level `Tags` list the call sends, if any.
    top_level: Option<Value>,
    expected: BTreeMap<String, String>,
}

/// Rows spanning what the rule generalises over: whether a specification names the
/// created resource's type, another type, or none; how many specifications and how
/// many tags each carries; a key repeated within and across the two members; a value
/// left absent; and a key outside ASCII.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "no tag member at all",
            specifications: None,
            top_level: None,
            expected: BTreeMap::new(),
        },
        Row {
            case: "the top-level member alone, as before",
            specifications: None,
            top_level: Some(tags(&[("env", "dev")])),
            expected: expected(&[("env", "dev")]),
        },
        Row {
            case: "one specification naming the created resource's type",
            specifications: Some(json!([
                { "ResourceType": "instance", "Tags": tags(&[("env", "dev")]) }
            ])),
            top_level: None,
            expected: expected(&[("env", "dev")]),
        },
        Row {
            case: "a specification naming no resource type",
            specifications: Some(json!([{ "Tags": tags(&[("team", "euc")]) }])),
            top_level: None,
            expected: expected(&[("team", "euc")]),
        },
        Row {
            case: "a specification naming another resource type",
            specifications: Some(json!([
                { "ResourceType": "volume", "Tags": tags(&[("env", "dev")]) }
            ])),
            top_level: None,
            expected: BTreeMap::new(),
        },
        Row {
            case: "one specification for the instance beside one for another type",
            specifications: Some(json!([
                { "ResourceType": "volume", "Tags": tags(&[("disk", "root")]) },
                { "ResourceType": "instance", "Tags": tags(&[("env", "prod")]) },
            ])),
            top_level: None,
            expected: expected(&[("env", "prod")]),
        },
        Row {
            case: "several specifications for the instance",
            specifications: Some(json!([
                { "ResourceType": "instance", "Tags": tags(&[("a", "1")]) },
                { "Tags": tags(&[("b", "2"), ("c", "3")]) },
                { "ResourceType": "instance", "Tags": tags(&[("d", "4")]) },
            ])),
            top_level: None,
            expected: expected(&[("a", "1"), ("b", "2"), ("c", "3"), ("d", "4")]),
        },
        Row {
            case: "a key repeated across two specifications",
            specifications: Some(json!([
                { "ResourceType": "instance", "Tags": tags(&[("env", "first")]) },
                { "ResourceType": "instance", "Tags": tags(&[("env", "last")]) },
            ])),
            top_level: None,
            expected: expected(&[("env", "last")]),
        },
        Row {
            case: "a key repeated inside one specification",
            specifications: Some(json!([{
                "ResourceType": "instance",
                "Tags": tags(&[("env", "first"), ("env", "last")]),
            }])),
            top_level: None,
            expected: expected(&[("env", "last")]),
        },
        Row {
            case: "an empty specification list",
            specifications: Some(json!([])),
            top_level: Some(tags(&[("env", "dev")])),
            expected: expected(&[("env", "dev")]),
        },
        Row {
            case: "a specification carrying an empty tag list",
            specifications: Some(json!([{ "ResourceType": "instance", "Tags": [] }])),
            top_level: None,
            expected: BTreeMap::new(),
        },
        Row {
            case: "a specification carrying no tag list",
            specifications: Some(json!([{ "ResourceType": "instance" }])),
            top_level: None,
            expected: BTreeMap::new(),
        },
        Row {
            case: "a specification tag with no value",
            specifications: Some(json!([
                { "ResourceType": "instance", "Tags": [{ "Key": "marker" }] }
            ])),
            top_level: None,
            expected: expected(&[("marker", "")]),
        },
        Row {
            case: "a specification tag with no key",
            specifications: Some(json!([
                { "ResourceType": "instance", "Tags": [{ "Value": "orphan" }] }
            ])),
            top_level: None,
            expected: BTreeMap::new(),
        },
        Row {
            case: "both members, disjoint keys",
            specifications: Some(json!([
                { "ResourceType": "instance", "Tags": tags(&[("from-spec", "1")]) }
            ])),
            top_level: Some(tags(&[("from-tags", "2")])),
            expected: expected(&[("from-spec", "1"), ("from-tags", "2")]),
        },
        Row {
            case: "both members sharing a key: the member naming the instance directly wins",
            specifications: Some(json!([
                { "ResourceType": "instance", "Tags": tags(&[("env", "from-spec")]) }
            ])),
            top_level: Some(tags(&[("env", "from-tags")])),
            expected: expected(&[("env", "from-tags")]),
        },
        Row {
            case: "keys outside ASCII, and a value with spaces",
            specifications: Some(json!([{
                "ResourceType": "instance",
                "Tags": tags(&[("étiquette", "valeur dev"), ("日本語", "テスト")]),
            }])),
            top_level: None,
            expected: expected(&[("étiquette", "valeur dev"), ("日本語", "テスト")]),
        },
    ]
}

#[tokio::test]
async fn tag_specifications_tag_the_instance_the_call_creates() {
    let router = router();

    for row in rows() {
        let mut managed_instance = json!({ "InstanceType": "t3.medium" });
        if let Some(specifications) = &row.specifications {
            managed_instance["TagSpecifications"] = specifications.clone();
        }
        let mut body = json!({ "ManagedInstance": managed_instance });
        if let Some(top_level) = &row.top_level {
            body["Tags"] = top_level.clone();
        }

        let id = create(&router, body).await;
        let actual = stored_tags(&router, &id).await;
        assert_eq!(
            actual, row.expected,
            "case {:?}: expected the instance to report {:?}, got {actual:?}",
            row.case, row.expected
        );
    }
}

#[tokio::test]
async fn a_specification_tags_only_the_resource_type_its_call_creates() {
    // The catalog comes from the model: every `ResourceTypeEnum` value a
    // `TagSpecification` can name (`wsinst::ops::types::RESOURCE_TYPE_ENUM_VALUES`,
    // `smithy_query(query="shape",shape="ResourceTypeEnum")`), plus a specification
    // that names none. A create applies a specification's tags only when it names the
    // resource type that call creates, or no type at all — so the same list of rows
    // tags an instance on one create and a volume on the other.
    let kinds = wsinst::ops::types::RESOURCE_TYPE_ENUM_VALUES;
    assert!(
        kinds.contains(&"instance") && kinds.contains(&"volume"),
        "case catalog: the generated ResourceTypeEnum list must carry the modelled values, got {kinds:?}"
    );
    let mut specification_kinds: Vec<Option<&str>> = vec![None];
    specification_kinds.extend(kinds.iter().map(|kind| Some(*kind)));

    let (router, engine) = harness();
    let store = wsinst::state::service_state(&*engine).expect("the service's own store");

    for kind in &specification_kinds {
        let mut specification = json!({ "Tags": tags(&[("env", "dev")]) });
        if let Some(kind) = kind {
            specification["ResourceType"] = json!(kind);
        }

        let instance = create(
            &router,
            json!({
                "ManagedInstance": {
                    "InstanceType": "t3.medium",
                    "TagSpecifications": [specification.clone()],
                },
            }),
        )
        .await;
        let instance_tags = stored_tags(&router, &instance).await;
        let tags_the_instance = kind.is_none_or(|kind| kind == "instance");
        assert_eq!(
            !instance_tags.is_empty(),
            tags_the_instance,
            "case a specification for {kind:?} on CreateWorkspaceInstance: expected tagged={tags_the_instance}, got {instance_tags:?}"
        );

        let payload = ok(
            &router,
            "CreateVolume",
            json!({
                "AvailabilityZone": "us-east-1a",
                "SizeInGB": 4,
                "TagSpecifications": [specification],
            }),
        )
        .await;
        let volume = payload["VolumeId"]
            .as_str()
            .unwrap_or_else(|| panic!("CreateVolume returned no VolumeId: {payload}"))
            .to_string();
        let volume_tags = store
            .lock()
            .volumes
            .get(&volume)
            .unwrap_or_else(|| panic!("the create stored no volume {volume:?}"))
            .tags
            .clone();
        let tags_the_volume = kind.is_none_or(|kind| kind == "volume");
        assert_eq!(
            !volume_tags.is_empty(),
            tags_the_volume,
            "case a specification for {kind:?} on CreateVolume: expected tagged={tags_the_volume}, got {volume_tags:?}"
        );
    }
}

#[tokio::test]
async fn a_reserved_key_in_a_specification_is_refused_and_creates_no_instance() {
    let router = router();
    let existing = create(&router, json!({ "ManagedInstance": {} })).await;

    // Rows from the same namespace rule the sibling `reserved_tag_keys` suite pins,
    // crossed with the resource type the specification names: the prefix is reserved
    // whichever specification carried the key, including one whose tags this call
    // would not apply at all.
    for (case, key) in [
        ("a system tag key", "aws:cloudformation:stack-name"),
        ("the prefix alone", "aws:"),
        ("the prefix upper case", "AWS:Region"),
        ("the prefix in mixed case", "Aws:CreatedBy"),
    ] {
        for kind in [None, Some("instance"), Some("volume")] {
            let mut specification = json!({ "Tags": tags(&[(key, "mine")]) });
            if let Some(kind) = kind {
                specification["ResourceType"] = json!(kind);
            }
            let (status, payload) = call(
                &router,
                "CreateWorkspaceInstance",
                json!({
                    "ManagedInstance": {
                        "InstanceType": "t3.medium",
                        "TagSpecifications": [specification],
                    },
                }),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?} for resource type {kind:?}: expected HTTP 400, got {status} with {payload}"
            );
            assert_eq!(
                error_code(&payload),
                "ValidationException",
                "case {case:?} for resource type {kind:?}: expected ValidationException, got {payload}"
            );
            let named: Vec<&str> = payload["FieldList"]
                .as_array()
                .map(|fields| {
                    fields
                        .iter()
                        .filter_map(|field| field["Name"].as_str())
                        .collect()
                })
                .unwrap_or_default();
            assert!(
                named.contains(&"TagSpecifications"),
                "case {case:?} for resource type {kind:?}: expected FieldList to name TagSpecifications, got {named:?} in {payload}"
            );

            let listed = list_all_ids(&router, json!({})).await;
            assert_eq!(
                listed,
                vec![existing.clone()],
                "case {case:?} for resource type {kind:?}: a refused create left an instance behind, got {listed:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_tag_set_a_specification_builds_cannot_exceed_what_the_model_reports() {
    let router = router();
    let limit = wsinst::state::MAX_TAGS_PER_INSTANCE;
    let existing = create(&router, json!({ "ManagedInstance": {} })).await;

    // Specifications carrying exactly the modelled maximum between them, split over
    // two specifications: stored whole and reported whole.
    let half: Vec<(String, String)> = (0..limit)
        .map(|i| (format!("key-{i:02}"), format!("value-{i:02}")))
        .collect();
    let (first, second) = half.split_at(limit / 2);
    let as_list = |pairs: &[(String, String)]| {
        Value::Array(
            pairs
                .iter()
                .map(|(key, value)| json!({ "Key": key, "Value": value }))
                .collect(),
        )
    };
    let id = create(
        &router,
        json!({
            "ManagedInstance": {
                "TagSpecifications": [
                    { "ResourceType": "instance", "Tags": as_list(first) },
                    { "Tags": as_list(second) },
                ],
            },
        }),
    )
    .await;
    let actual = stored_tags(&router, &id).await;
    assert_eq!(
        actual.len(),
        limit,
        "case filled to the limit from two specifications: expected {limit} tags, got {actual:?}"
    );

    // One key past the limit, reached three ways: all from specifications, from the
    // top-level member plus a specification, and far past it.
    let over_rows: Vec<(&str, Value)> = vec![
        (
            "one key past the limit, all from specifications",
            json!({
                "ManagedInstance": {
                    "TagSpecifications": [
                        { "ResourceType": "instance", "Tags": as_list(&half) },
                        { "ResourceType": "instance", "Tags": tags(&[("one-too-many", "x")]) },
                    ],
                },
            }),
        ),
        (
            "one key past the limit, split across both members",
            json!({
                "ManagedInstance": {
                    "TagSpecifications": [
                        { "ResourceType": "instance", "Tags": tags(&[("one-too-many", "x")]) }
                    ],
                },
                "Tags": as_list(&half),
            }),
        ),
        (
            "far past the limit, from several specifications",
            json!({
                "ManagedInstance": {
                    "TagSpecifications": [
                        { "ResourceType": "instance", "Tags": as_list(&half) },
                        { "Tags": as_list(&half.iter().map(|(k, v)| (format!("other-{k}"), v.clone())).collect::<Vec<_>>()) },
                    ],
                },
            }),
        ),
    ];
    for (case, body) in over_rows {
        let (status, payload) = call(&router, "CreateWorkspaceInstance", body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {payload}"
        );
        assert_eq!(
            error_code(&payload),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {payload}"
        );
        let listed = list_all_ids(&router, json!({})).await;
        assert_eq!(
            listed.len(),
            2,
            "case {case:?}: a refused create left an instance behind, got {listed:?}"
        );
        assert!(
            listed.contains(&existing) && listed.contains(&id),
            "case {case:?}: expected only the two accepted instances, got {listed:?}"
        );
    }
}
