// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the Entity Resolution tagging surface —
//! `TagResource`, `ListTagsForResource` and `UntagResource`.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 request signed for the `entityresolution` signing name, through the
//! harness the sibling `schema_mapping_workflow_lifecycle` suite owns — so what is
//! asserted is what an AWS SDK client would see.
//!
//! The rule under test: each tagging operation addresses the resource its
//! `resourceArn` identifies and acts on THAT resource's persisted tag map.
//! `ListTagsForResource` reports exactly the stored map and changes nothing;
//! `TagResource` merges the request's pairs into it (a key the resource does not
//! carry is appended, a key it already carries takes the new value); `UntagResource`
//! removes exactly the named keys and is idempotent for a key that is not there; a
//! merge that would leave the resource carrying more than the modelled `TagMap`
//! maximum of 200 tags is refused whole with HTTP 400 `ValidationException`; and
//! an ARN that names no stored resource is refused with HTTP 404
//! `ResourceNotFoundException` on all three.
//!
//! Each table row differs along what the rule generalises over: the resource TYPE the
//! ARN names (`schemamapping`, `matchingworkflow`, `idmappingworkflow`, `idnamespace`
//! — every type this service persists), whether the resource was created with tags, with an empty
//! tag map or with none, the number of pairs written (one, several, the modelled
//! `TagMap` maximum of 200), the key and value lengths (the modelled `TagKey` 1..128
//! and `TagValue` 0..256 bounds), and whether an untagged key exists. The recorded
//! tag/list/untag round trip is ONE row of these.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use crate::schema_mapping_workflow_lifecycle::{
    account_id, assert_error_shape, glue_table_arn, role_arn, Emulator, Reply, REGION,
};

/// Percent-encode an ARN for the single (non-greedy) `@httpLabel` segment of
/// `/tags/{resourceArn}`, the way an AWS SDK client does: everything outside the
/// unreserved set is encoded, so the ARN's own `/` cannot be read as a path
/// separator. A `VeniceGlobalArn` is ASCII by its modelled pattern, so a byte-wise
/// encoding is exact.
fn encoded_label(resource_arn: &str) -> String {
    resource_arn
        .bytes()
        .map(|byte| {
            let ch = char::from(byte);
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '~') {
                ch.to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

/// `GET /tags/{resourceArn}` — `ListTagsForResource`.
async fn list_tags(emulator: &Emulator, resource_arn: &str) -> Reply {
    emulator
        .call("GET", &format!("/tags/{}", encoded_label(resource_arn)), None)
        .await
}

/// `POST /tags/{resourceArn}` with a `tags` body — `TagResource`.
async fn tag_resource(emulator: &Emulator, resource_arn: &str, tags: &Value) -> Reply {
    emulator
        .call(
            "POST",
            &format!("/tags/{}", encoded_label(resource_arn)),
            Some(&json!({ "tags": tags })),
        )
        .await
}

/// `DELETE /tags/{resourceArn}?tagKeys=…` — `UntagResource`. `tagKeys` is an
/// `@httpQuery` list, so it goes on the wire as one repeated parameter per key.
async fn untag_resource(emulator: &Emulator, resource_arn: &str, tag_keys: &[String]) -> Reply {
    let query: Vec<String> = tag_keys
        .iter()
        .map(|key| format!("tagKeys={}", encoded_label(key)))
        .collect();
    emulator
        .call(
            "DELETE",
            &format!("/tags/{}?{}", encoded_label(resource_arn), query.join("&")),
            None,
        )
        .await
}

/// The `tags` member of a response body, as a sorted map so two tag sets compare
/// independently of JSON member order.
fn tags_of(reply: &Reply, case: &str) -> BTreeMap<String, String> {
    let tags = reply
        .body
        .get("tags")
        .unwrap_or_else(|| panic!("case {case:?}: response carries no `tags` member: {reply:?}"));
    sorted(tags, case)
}

/// A JSON object of string values, as a sorted map.
fn sorted(tags: &Value, case: &str) -> BTreeMap<String, String> {
    tags.as_object()
        .unwrap_or_else(|| panic!("case {case:?}: `tags` is not an object: {tags:?}"))
        .iter()
        .map(|(key, value)| {
            let text = value
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: tag {key:?} is not a string"));
            (key.clone(), text.to_string())
        })
        .collect()
}

/// Build a JSON tag object from pairs.
fn tag_object(pairs: Vec<(String, String)>) -> Value {
    let mut map = Map::new();
    for (key, value) in pairs {
        map.insert(key, Value::String(value));
    }
    Value::Object(map)
}

/// Which taggable resource type a row runs against. These are the resource types this
/// service persists, and the `VeniceGlobalArn` pattern names each of them
/// (`smithy_query(query="operation",operation="TagResource")`).
#[derive(Clone, Copy, Debug)]
enum Kind {
    SchemaMapping,
    MatchingWorkflow,
    IdMappingWorkflow,
    IdNamespace,
}

impl Kind {
    const ALL: [Self; 4] = [
        Self::SchemaMapping,
        Self::MatchingWorkflow,
        Self::IdMappingWorkflow,
        Self::IdNamespace,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::SchemaMapping => "schemamapping",
            Self::MatchingWorkflow => "matchingworkflow",
            Self::IdMappingWorkflow => "idmappingworkflow",
            Self::IdNamespace => "idnamespace",
        }
    }
}

/// Create a resource of `kind` named `name`, carrying `tags` when the row asks for
/// them, and answer the ARN the service minted for it.
///
/// Both workflow kinds reference a schema mapping by name from their input source, so
/// those rows seed an UNTAGGED schema mapping of the same name first: that also pins
/// that a tag write addressed to one ARN leaves the other resources alone.
async fn create_resource(
    emulator: &Emulator,
    kind: Kind,
    name: &str,
    tags: Option<&Value>,
    case: &str,
) -> String {
    // A workflow's `inputSourceConfig` references a schema mapping BY NAME, so those
    // rows need one of the same name in the store; an ID namespace references none, so
    // it is created on its own.
    if !matches!(kind, Kind::IdNamespace) {
        let mut schema = json!({
            "schemaName": name,
            "mappedInputFields": [
                { "fieldName": "id", "type": "UNIQUE_ID" },
                { "fieldName": "name", "type": "NAME" },
            ],
        });
        if matches!(kind, Kind::SchemaMapping) {
            if let Some(tags) = tags {
                schema["tags"] = tags.clone();
            }
        }
        let created = emulator.create_schema_mapping(&schema).await;
        // For a workflow row the schema mapping is only the reference its input source
        // names, so one another row already created is as good: `CreateSchemaMapping`
        // answers `ConflictException` for a name that is taken.
        let seeded_reference = !matches!(kind, Kind::SchemaMapping)
            && created.status == 400
            && created.error_code.as_deref() == Some("ConflictException");
        assert!(
            created.status == 200 || seeded_reference,
            "case {case:?}: seeding schema mapping {name} failed: {created:?}"
        );
        if matches!(kind, Kind::SchemaMapping) {
            return created
                .body
                .get("schemaArn")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: no schemaArn in {created:?}"))
                .to_string();
        }
    }

    match kind {
        // Answered above, from the create that seeded it.
        Kind::SchemaMapping => unreachable!("the schema-mapping ARN is answered above"),
        Kind::MatchingWorkflow => {
            let mut workflow = json!({
                "workflowName": name,
                "inputSourceConfig": [
                    { "inputSourceARN": glue_table_arn("customers"), "schemaName": name },
                ],
                "outputSourceConfig": [{
                    "outputS3Path": "s3://er-output/tags",
                    "output": [{ "name": "id" }],
                }],
                "resolutionTechniques": { "resolutionType": "ML_MATCHING" },
                "roleArn": role_arn(),
            });
            if let Some(tags) = tags {
                workflow["tags"] = tags.clone();
            }
            let created = emulator.create_matching_workflow(&workflow).await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: seeding matching workflow {name} failed: {created:?}"
            );
            created
                .body
                .get("workflowArn")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: no workflowArn in {created:?}"))
                .to_string()
        }
        Kind::IdMappingWorkflow => {
            // The smallest `CreateIdMappingWorkflow` the model admits: one input
            // source (`length 1..20`) and the @required `idMappingTechniques`, whose
            // enum values come from
            // `smithy_query(query="shape",shape="IdMappingRuleBasedProperties")`.
            let mut workflow = json!({
                "workflowName": name,
                "inputSourceConfig": [
                    {
                        "inputSourceARN": glue_table_arn("customers"),
                        "schemaName": name,
                        "type": "SOURCE",
                    },
                ],
                "idMappingTechniques": {
                    "idMappingType": "RULE_BASED",
                    "ruleBasedProperties": {
                        "ruleDefinitionType": "SOURCE",
                        "attributeMatchingModel": "ONE_TO_ONE",
                        "recordMatchingModel": "ONE_SOURCE_TO_ONE_TARGET",
                        "rules": [{ "ruleName": "by email", "matchingKeys": ["email"] }],
                    },
                },
                "roleArn": role_arn(),
            });
            if let Some(tags) = tags {
                workflow["tags"] = tags.clone();
            }
            let created = emulator
                .call("POST", "/idmappingworkflows", Some(&workflow))
                .await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: seeding ID mapping workflow {name} failed: {created:?}"
            );
            created
                .body
                .get("workflowArn")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: no workflowArn in {created:?}"))
                .to_string()
        }
        Kind::IdNamespace => {
            // The smallest `CreateIdNamespace` the model admits: the name and the
            // @required `type` (`IdNamespaceType`, `SOURCE | TARGET`, from
            // `smithy_query(query="shape",shape="IdNamespaceType")`).
            let mut namespace = json!({ "idNamespaceName": name, "type": "SOURCE" });
            if let Some(tags) = tags {
                namespace["tags"] = tags.clone();
            }
            let created = emulator.create_id_namespace(&namespace).await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: seeding ID namespace {name} failed: {created:?}"
            );
            created
                .body
                .get("idNamespaceArn")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: no idNamespaceArn in {created:?}"))
                .to_string()
        }
    }
}

/// Read the resource back through its own Get operation, so a tag write is proved to
/// have landed on the ONE stored record both surfaces serve.
async fn read_resource(emulator: &Emulator, kind: Kind, name: &str) -> Reply {
    match kind {
        Kind::SchemaMapping => emulator.get_schema_mapping(name).await,
        Kind::MatchingWorkflow => emulator.get_matching_workflow(name).await,
        Kind::IdMappingWorkflow => {
            emulator
                .call("GET", &format!("/idmappingworkflows/{name}"), None)
                .await
        }
        Kind::IdNamespace => emulator.get_id_namespace(name).await,
    }
}

/// One tag/list/untag round trip: what the resource was created with, what is merged
/// onto it, what must then be stored, which keys are removed, and what must remain.
struct TagCase {
    case: &'static str,
    created_tags: Option<Value>,
    merged: Value,
    after_merge: Value,
    removed: Vec<String>,
    after_removal: Value,
}

/// The round-trip table.
///
/// The bounds come from the model
/// (`smithy_query(query="shape",shape="TagMap,TagKey,TagValue")`): `TagMap` is
/// `length 0..200`, `TagKey` `length 1..128`, `TagValue` `length 0..256`.
fn tag_cases() -> Vec<TagCase> {
    // The modelled 200-pair maximum, as 200 distinct pairs rather than one repeated.
    let widest: Vec<(String, String)> = (0..200)
        .map(|index| (format!("key-{index:03}"), format!("value-{index:03}")))
        .collect();
    // Removing half of the widest set leaves the other half, key for key.
    let widest_removed: Vec<String> = widest
        .iter()
        .take(100)
        .map(|(key, _)| key.clone())
        .collect();
    let widest_remaining: Vec<(String, String)> = widest.iter().skip(100).cloned().collect();

    vec![
        TagCase {
            case: "created with no tags at all, one pair added then removed",
            created_tags: None,
            merged: json!({ "env": "test" }),
            after_merge: json!({ "env": "test" }),
            removed: vec!["env".to_string()],
            after_removal: json!({}),
        },
        TagCase {
            case: "created with tags: a new key is appended and an existing key \
                   takes the new value",
            created_tags: Some(json!({ "team": "identity", "env": "test" })),
            merged: json!({ "env": "prod", "owner": "ana" }),
            after_merge: json!({ "team": "identity", "env": "prod", "owner": "ana" }),
            // The second key is not on the resource: an untag of an absent key is a
            // no-op, not a failure.
            removed: vec!["team".to_string(), "never-set".to_string()],
            after_removal: json!({ "env": "prod", "owner": "ana" }),
        },
        TagCase {
            case: "created with an empty tag map, and an untag of a key that was \
                   never set",
            created_tags: Some(json!({})),
            merged: json!({ "only": "one" }),
            after_merge: json!({ "only": "one" }),
            removed: vec!["absent".to_string()],
            after_removal: json!({ "only": "one" }),
        },
        TagCase {
            case: "key and value at the modelled TagKey/TagValue length bounds",
            created_tags: None,
            merged: tag_object(vec![
                // `TagKey` maximum length 128 with `TagValue` maximum length 256.
                ("k".repeat(128), "v".repeat(256)),
                // `TagKey` minimum length 1 with the modelled EMPTY `TagValue`.
                ("a".to_string(), String::new()),
            ]),
            after_merge: tag_object(vec![
                ("k".repeat(128), "v".repeat(256)),
                ("a".to_string(), String::new()),
            ]),
            removed: vec!["k".repeat(128)],
            after_removal: tag_object(vec![("a".to_string(), String::new())]),
        },
        TagCase {
            case: "the modelled TagMap maximum of 200 pairs, half of them removed",
            created_tags: None,
            merged: tag_object(widest.clone()),
            after_merge: tag_object(widest),
            removed: widest_removed,
            after_removal: tag_object(widest_remaining),
        },
    ]
}

#[tokio::test]
async fn tag_list_and_untag_round_trip_over_every_resource_type_and_tag_shape() {
    let emulator = Emulator::new();

    for (index, row) in tag_cases().into_iter().enumerate() {
        for kind in Kind::ALL {
            let case = format!("{} [{}]", row.case, kind.label());
            let name = format!("{}_{index}", kind.label());
            let resource_arn =
                create_resource(&emulator, kind, &name, row.created_tags.as_ref(), &case).await;

            // 1. A create's own `tags` are what the tag reader reports, with no
            //    tagging call in between; a resource created without the member
            //    reads back as the EMPTY map, because `ListTagsForResourceOutput$tags`
            //    is modelled @required.
            let listed = list_tags(&emulator, &resource_arn).await;
            assert_eq!(
                listed.status, 200,
                "case {case:?}: ListTagsForResource must answer 200, got {listed:?}"
            );
            let expected_at_create = row.created_tags.clone().unwrap_or_else(|| json!({}));
            assert_eq!(
                tags_of(&listed, &case),
                sorted(&expected_at_create, &case),
                "case {case:?}: ListTagsForResource must report the tags the create \
                 persisted, got {:?}",
                listed.body
            );

            // 2. A read does not change what the next reader sees.
            let again = list_tags(&emulator, &resource_arn).await;
            assert_eq!(
                again.body, listed.body,
                "case {case:?}: ListTagsForResource must not change the stored tags"
            );

            // 3. TagResource merges: new keys appended, existing keys overwritten.
            let tagged = tag_resource(&emulator, &resource_arn, &row.merged).await;
            assert_eq!(
                tagged.status, 200,
                "case {case:?}: TagResource must answer 200, got {tagged:?}"
            );
            let listed = list_tags(&emulator, &resource_arn).await;
            assert_eq!(
                tags_of(&listed, &case),
                sorted(&row.after_merge, &case),
                "case {case:?}: after TagResource the stored tags must be the merge, \
                 got {:?}",
                listed.body
            );

            // 4. The merge landed on the resource's own record, which is what its
            //    Get operation serves.
            let read = read_resource(&emulator, kind, &name).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: the resource must still read back, got {read:?}"
            );
            assert_eq!(
                tags_of(&read, &case),
                sorted(&row.after_merge, &case),
                "case {case:?}: the resource's own reader must serve the merged tags, \
                 got {:?}",
                read.body
            );

            // 5. UntagResource removes exactly the named keys, and is a no-op for a
            //    key the resource does not carry.
            let untagged = untag_resource(&emulator, &resource_arn, &row.removed).await;
            assert_eq!(
                untagged.status, 200,
                "case {case:?}: UntagResource must answer 200, got {untagged:?}"
            );
            let listed = list_tags(&emulator, &resource_arn).await;
            assert_eq!(
                tags_of(&listed, &case),
                sorted(&row.after_removal, &case),
                "case {case:?}: after UntagResource exactly the named keys must be \
                 gone, got {:?}",
                listed.body
            );

            // 6. Repeating the untag changes nothing (@idempotent on the model).
            let repeated = untag_resource(&emulator, &resource_arn, &row.removed).await;
            assert_eq!(
                repeated.status, 200,
                "case {case:?}: a repeated UntagResource must answer 200, got {repeated:?}"
            );
            let after_repeat = list_tags(&emulator, &resource_arn).await;
            assert_eq!(
                after_repeat.body, listed.body,
                "case {case:?}: a repeated UntagResource must leave the tags as they were"
            );
        }
    }
}

#[tokio::test]
async fn a_tagging_call_touches_only_the_resource_its_arn_names() {
    let emulator = Emulator::new();

    // One name, one resource of EVERY taggable type: `shared`. Their ARNs differ only
    // in the resource TYPE segment, so a tag write that resolved by name alone would
    // hit all three.
    let case = "one name, every taggable resource type";
    let mut arns = Vec::new();
    for kind in Kind::ALL {
        arns.push((
            kind,
            create_resource(&emulator, kind, "shared", None, case).await,
        ));
    }
    let distinct: std::collections::BTreeSet<&String> =
        arns.iter().map(|(_, resource_arn)| resource_arn).collect();
    assert_eq!(
        distinct.len(),
        arns.len(),
        "case {case:?}: the resources must have distinct ARNs, got {arns:?}"
    );

    // Tag exactly one of them, then every other must still report no tags.
    for (tagged_kind, tagged_arn) in &arns {
        let tag = json!({ "on": tagged_kind.label() });
        assert_eq!(
            tag_resource(&emulator, tagged_arn, &tag).await.status,
            200,
            "case {case:?}: tagging the {} must succeed",
            tagged_kind.label()
        );
        for (other_kind, other_arn) in &arns {
            let listed = list_tags(&emulator, other_arn).await;
            let expected = if other_arn == tagged_arn {
                sorted(&tag, case)
            } else {
                BTreeMap::new()
            };
            assert_eq!(
                tags_of(&listed, case),
                expected,
                "case {case:?}: tagging the {} must not change the {}'s tags, got {:?}",
                tagged_kind.label(),
                other_kind.label(),
                listed.body
            );
        }
        // Untagging the one that was tagged puts every resource back to no tags, so
        // the next iteration starts from the same state.
        assert_eq!(
            untag_resource(&emulator, tagged_arn, &["on".to_string()])
                .await
                .status,
            200,
            "case {case:?}: untagging the {} must succeed",
            tagged_kind.label()
        );
    }
}

#[tokio::test]
async fn an_empty_merge_or_an_untag_of_absent_keys_leaves_the_record_untouched() {
    let emulator = Emulator::new();

    // The class: a write that asks for nothing. `TagMap` admits the empty map
    // (`length 0..200`), and an untag may name keys the resource does not carry.
    let case = "a write that asks for nothing";
    let resource_arn =
        create_resource(&emulator, Kind::SchemaMapping, "untouched", None, case).await;
    let before = read_resource(&emulator, Kind::SchemaMapping, "untouched").await;

    for (label, reply) in [
        (
            "TagResource with an empty tag map",
            tag_resource(&emulator, &resource_arn, &json!({})).await,
        ),
        (
            "UntagResource naming keys that were never set",
            untag_resource(
                &emulator,
                &resource_arn,
                &["nope".to_string(), "still-nope".to_string()],
            )
            .await,
        ),
    ] {
        assert_eq!(
            reply.status, 200,
            "case {case:?}: {label} must answer 200, got {reply:?}"
        );
    }

    let after = read_resource(&emulator, Kind::SchemaMapping, "untouched").await;
    assert_eq!(
        after.body, before.body,
        "case {case:?}: a write that asks for nothing must leave the record exactly \
         as it was, got {:?}",
        after.body
    );
    let listed = list_tags(&emulator, &resource_arn).await;
    assert_eq!(
        tags_of(&listed, case),
        BTreeMap::new(),
        "case {case:?}: the resource must still report no tags, got {:?}",
        listed.body
    );
}

/// The most tags one resource may carry, from the modelled `TagMap`
/// (`length 0..200`, `smithy_query(query="shape",shape="TagMap")`) — the figure
/// `crate::state::MAX_TAGS_PER_RESOURCE` enforces on a merge.
const MAX_TAGS_PER_RESOURCE: usize = 200;

#[tokio::test]
async fn a_merge_past_the_tag_maximum_is_refused_whole_and_the_bound_is_self_healing() {
    let emulator = Emulator::new();

    for kind in Kind::ALL {
        let case = format!("the tag maximum [{}]", kind.label());
        let resource_arn = create_resource(&emulator, kind, "bounded", None, &case).await;

        // AT the bound: a merge that leaves exactly the maximum is accepted.
        let full: Vec<(String, String)> = (0..MAX_TAGS_PER_RESOURCE)
            .map(|index| (format!("key-{index:03}"), format!("value-{index:03}")))
            .collect();
        let at_bound = tag_resource(&emulator, &resource_arn, &tag_object(full.clone())).await;
        assert_eq!(
            at_bound.status, 200,
            "case {case:?}: a merge up to the maximum of {MAX_TAGS_PER_RESOURCE} tags must \
             be accepted, got {at_bound:?}"
        );

        // At the bound, a merge that only OVERWRITES existing keys adds no entry, so
        // it is still accepted.
        let overwritten: Vec<(String, String)> = full
            .iter()
            .map(|(key, _)| (key.clone(), "rewritten".to_string()))
            .collect();
        let rewrite =
            tag_resource(&emulator, &resource_arn, &tag_object(overwritten.clone())).await;
        assert_eq!(
            rewrite.status, 200,
            "case {case:?}: overwriting existing keys at the maximum must be accepted, \
             got {rewrite:?}"
        );
        let listed = list_tags(&emulator, &resource_arn).await;
        assert_eq!(
            tags_of(&listed, &case),
            sorted(&tag_object(overwritten.clone()), &case),
            "case {case:?}: the overwrite must have taken effect, got {} tag(s)",
            tags_of(&listed, &case).len()
        );

        // ONE PAST the bound: a merge carrying a key the resource does not have is
        // refused whole, and the stored map is untouched.
        let past = tag_resource(&emulator, &resource_arn, &json!({ "one-too-many": "x" })).await;
        assert_error_shape(
            &format!("{case} [a merge past the maximum]"),
            &past,
            400,
            "ValidationException",
        );
        let after_refusal = list_tags(&emulator, &resource_arn).await;
        assert_eq!(
            after_refusal.body, listed.body,
            "case {case:?}: a refused merge must leave the stored tags exactly as they \
             were, got {:?}",
            after_refusal.body
        );

        // The bound is self-healing: releasing a key makes room for the refused one.
        let (released, _) = &full[0];
        assert_eq!(
            untag_resource(&emulator, &resource_arn, std::slice::from_ref(released))
                .await
                .status,
            200,
            "case {case:?}: releasing a key must succeed"
        );
        let retried = tag_resource(&emulator, &resource_arn, &json!({ "one-too-many": "x" })).await;
        assert_eq!(
            retried.status, 200,
            "case {case:?}: with room released the same merge must be accepted, got {retried:?}"
        );
        let mut expected: Vec<(String, String)> = overwritten
            .into_iter()
            .filter(|(key, _)| key != released)
            .collect();
        expected.push(("one-too-many".to_string(), "x".to_string()));
        let listed = list_tags(&emulator, &resource_arn).await;
        assert_eq!(
            tags_of(&listed, &case),
            sorted(&tag_object(expected), &case),
            "case {case:?}: the resource must carry the released-for key, got {} tag(s)",
            tags_of(&listed, &case).len()
        );
    }
}

/// A well-formed `VeniceGlobalArn` that names nothing this service holds.
fn arn(partition: &str, region: &str, account: &str, kind: &str, name: &str) -> String {
    format!("arn:{partition}:entityresolution:{region}:{account}:{kind}/{name}")
}

#[tokio::test]
async fn an_arn_that_names_no_stored_resource_is_refused_on_every_tagging_operation() {
    let emulator = Emulator::new();

    // A resource that exists and is then deleted is part of the class too.
    let gone = create_resource(&emulator, Kind::SchemaMapping, "gone", None, "deleted").await;
    assert_eq!(
        emulator.delete_schema_mapping("gone").await.status,
        200,
        "the fixture schema mapping must delete cleanly"
    );
    // Resources that DO exist, so the refusals below are about the ARN and not about
    // an empty store: a schema mapping called `present` and an ID namespace called
    // `namespace`.
    let present =
        create_resource(&emulator, Kind::SchemaMapping, "present", None, "present").await;
    let namespace =
        create_resource(&emulator, Kind::IdNamespace, "namespace", None, "namespace").await;

    let absent = [
        (
            "a schemamapping that was never created",
            arn(
                "aws",
                REGION,
                account_id(),
                "schemamapping",
                "never-created",
            ),
        ),
        (
            "a matchingworkflow that was never created",
            arn(
                "aws",
                REGION,
                account_id(),
                "matchingworkflow",
                "never-created",
            ),
        ),
        (
            "an idmappingworkflow that was never created",
            arn(
                "aws",
                REGION,
                account_id(),
                "idmappingworkflow",
                "never-created",
            ),
        ),
        ("a schemamapping that was created and deleted", gone),
        // An `idnamespace` ARN naming a namespace that was never created. The store
        // DOES hold an ID namespace (called `namespace`, created above), so this row
        // is about the NAME, not about an empty namespace map.
        (
            "an idnamespace that was never created",
            arn("aws", REGION, account_id(), "idnamespace", "never-created"),
        ),
        // The stored names under the WRONG resource type: each of these resources
        // exists, but under another type's ARN, so the ARN names nothing.
        (
            "the stored schema-mapping name as an idnamespace ARN",
            arn("aws", REGION, account_id(), "idnamespace", "present"),
        ),
        (
            "the stored id-namespace name as a schemamapping ARN",
            arn("aws", REGION, account_id(), "schemamapping", "namespace"),
        ),
        // A resource is region- and account-scoped, so the same NAME in another
        // region, another account or another partition is another resource.
        (
            "the stored name in another region",
            arn(
                "aws",
                "us-west-2",
                account_id(),
                "schemamapping",
                "present",
            ),
        ),
        (
            "the stored name in another account",
            arn("aws", REGION, "999999999999", "schemamapping", "present"),
        ),
        (
            "the stored name in another partition",
            arn(
                "aws-cn",
                "cn-north-1",
                account_id(),
                "schemamapping",
                "present",
            ),
        ),
        // The name at the modelled `EntityName` maximum of 255, which the issue never
        // cited.
        (
            "a 255-character name that was never created",
            arn(
                "aws",
                REGION,
                account_id(),
                "matchingworkflow",
                &"z".repeat(255),
            ),
        ),
    ];

    for (case, resource_arn) in absent {
        let replies = [
            (
                "ListTagsForResource",
                list_tags(&emulator, &resource_arn).await,
            ),
            (
                "TagResource",
                tag_resource(&emulator, &resource_arn, &json!({ "env": "test" })).await,
            ),
            (
                "UntagResource",
                untag_resource(&emulator, &resource_arn, &["env".to_string()]).await,
            ),
        ];
        for (operation, reply) in replies {
            // The harness's shared refusal check: status plus wire error code, which is
            // what a client branches on.
            assert_error_shape(
                &format!("{case} [{operation}]"),
                &reply,
                404,
                "ResourceNotFoundException",
            );
        }
    }

    // And the refused calls persisted nothing: both resources that do exist still
    // carry no tags.
    for (label, resource_arn) in [("present", &present), ("namespace", &namespace)] {
        let listed = list_tags(&emulator, resource_arn).await;
        assert_eq!(
            tags_of(&listed, label),
            BTreeMap::new(),
            "case {label:?}: a refused tagging call must persist nothing, got {:?}",
            listed.body
        );
    }
}
