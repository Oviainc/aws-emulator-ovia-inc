// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UntagResource`.
//!
//! RULE under test: `UntagResource` REMOVES from the tag set the named resource carries
//! in this service's store exactly the keys the request lists — every other tag stays,
//! values are irrelevant to the removal, and a key the resource does not carry leaves
//! the set alone, which is what makes the `@idempotent` operation answer the same
//! success when it is repeated; the removal touches no other resource and no part of
//! the resource beyond its tag set; and a `ResourceArn` this endpoint holds no resource
//! for is the modelled `ResourceNotFoundException`.
//!
//! The table spans the class the rule generalises over: HOW MANY of the carried keys a
//! request removes (none, one, some, all, and the modelled `TagKeys` maximum of 50),
//! whether a listed key is carried at all, whether it is listed twice, and the FORM and
//! LENGTH of each key (plain, punctuated, upper-case, numeric, spaced, non-ASCII, and
//! `TagKey`'s published maximum of 128). The tag set is read back through the
//! implemented `ListTagsForResource`, so what is asserted is what a client sees.

use std::time::Duration;

use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::state::WORKFLOW_DELETED_AFTER;

use crate::missing_resource_lookups::{ghost_arns, LOOKUP_MISS_STATUS, WORKFLOW_MISSING};
use crate::resource_tag_listing::{generated_tags, list_tags, workflow_with_tags};
use crate::workflow_lifecycle::Harness;

/// `TagKeys`' published entry maximum (`smithy_query(query="shape", shape="TagKeys")`:
/// `@length(min: 1, max: 50)`), which is also `Tags`' entry maximum.
const MAX_TAG_KEYS: usize = 50;

/// `TagKey`'s published length maximum (`@length(min: 1, max: 128)`).
const MAX_TAG_KEY_LEN: usize = 128;

/// One row: a label, the tag set the resource is created with, the `TagKeys` the untag
/// request lists, and the tag set the resource must carry afterwards.
struct Case {
    label: String,
    initial: serde_json::Value,
    remove: Vec<String>,
    expected: serde_json::Value,
}

fn case(
    label: &str,
    initial: serde_json::Value,
    remove: &[&str],
    expected: serde_json::Value,
) -> Case {
    Case {
        label: label.to_string(),
        initial,
        remove: remove.iter().map(|key| (*key).to_string()).collect(),
        expected,
    }
}

/// The keys of a `Tags` map, in the map's own order.
fn keys_of(tags: &serde_json::Value) -> Vec<String> {
    tags.as_object()
        .map(|members| members.keys().cloned().collect())
        .unwrap_or_default()
}

/// A `Tags` map built from owned keys, for the rows whose keys are generated rather
/// than written out.
fn tag_map(entries: &[(String, &str)]) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for (key, value) in entries {
        map.insert(
            key.clone(),
            serde_json::Value::String((*value).to_string()),
        );
    }
    serde_json::Value::Object(map)
}

/// Every row of the table.
fn cases() -> Vec<Case> {
    let empty = serde_json::json!({});
    let three = serde_json::json!({"env": "prod", "team": "data", "owner": "alice"});
    let mut cases = vec![
        case(
            "the only key the resource carries",
            serde_json::json!({"env": "prod"}),
            &["env"],
            empty.clone(),
        ),
        case(
            "one key of three",
            three.clone(),
            &["team"],
            serde_json::json!({"env": "prod", "owner": "alice"}),
        ),
        case(
            "two keys of three",
            three.clone(),
            &["env", "owner"],
            serde_json::json!({"team": "data"}),
        ),
        case(
            "every key the resource carries",
            three.clone(),
            &["env", "team", "owner"],
            empty.clone(),
        ),
        case(
            // `@idempotent`: a key that is not there is removed by there being
            // nothing to remove.
            "a key the resource does not carry",
            serde_json::json!({"env": "prod"}),
            &["absent"],
            serde_json::json!({"env": "prod"}),
        ),
        case(
            "a carried key beside one the resource does not carry",
            serde_json::json!({"env": "prod", "team": "data"}),
            &["env", "absent"],
            serde_json::json!({"team": "data"}),
        ),
        case(
            "the same key listed twice in one request",
            serde_json::json!({"env": "prod", "team": "data"}),
            &["env", "env"],
            serde_json::json!({"team": "data"}),
        ),
        case(
            "a resource that carries no tag at all",
            empty.clone(),
            &["env"],
            empty.clone(),
        ),
        case(
            // "Only the keys are required; the values are ignored": a tag whose value
            // is empty (`TagValue`'s published minimum of 0) is removed by its key
            // like any other.
            "a key whose value is empty",
            serde_json::json!({"env": "", "team": "data"}),
            &["env"],
            serde_json::json!({"team": "data"}),
        ),
        case(
            // `TagKey` is `@pattern(^.*$)`, so any form of text is a legal key.
            "keys differing in form: punctuation, case, digits, spaces",
            serde_json::json!({
                "Key.With-Dots_1": "a/b:c",
                "UPPER": "lower",
                "123": "456",
                "spaced key": "spaced value",
            }),
            &["Key.With-Dots_1", "123"],
            serde_json::json!({"UPPER": "lower", "spaced key": "spaced value"}),
        ),
        case(
            "a non-ASCII key",
            serde_json::json!({"проект": "значение", "テスト": "値"}),
            &["проект"],
            serde_json::json!({"テスト": "値"}),
        ),
    ];

    // `TagKey`'s published maximum, exactly at the bound.
    let longest_key = "k".repeat(MAX_TAG_KEY_LEN);
    cases.push(case(
        "a key at TagKey's published maximum of 128 characters",
        tag_map(&[
            (longest_key.clone(), "v"),
            ("short".to_string(), "w"),
        ]),
        &[longest_key.as_str()],
        serde_json::json!({"short": "w"}),
    ));

    // Entry counts across the published range: the maximum the request may list, and
    // one short of it.
    let full = generated_tags(MAX_TAG_KEYS);
    let all_keys = keys_of(&full);
    let all: Vec<&str> = all_keys.iter().map(String::as_str).collect();
    cases.push(case(
        &format!("{MAX_TAG_KEYS} keys, TagKeys' published maximum"),
        full.clone(),
        &all,
        serde_json::json!({}),
    ));
    let last = all_keys
        .last()
        .cloned()
        .expect("the generated tag set has a last key");
    let kept = full
        .get(&last)
        .and_then(serde_json::Value::as_str)
        .expect("the generated tag set holds a value for its last key")
        .to_string();
    cases.push(case(
        &format!("{} keys, one short of the maximum", MAX_TAG_KEYS - 1),
        full.clone(),
        &all[..MAX_TAG_KEYS - 1],
        tag_map(&[(last, kept.as_str())]),
    ));

    cases
}

/// Call `UntagResource` and report its status and body.
async fn untag(harness: &Harness, arn: &str, keys: &[String]) -> (u16, serde_json::Value) {
    harness
        .call(
            "UntagResource",
            serde_json::json!({"ResourceArn": arn, "TagKeys": keys}),
        )
        .await
}

#[tokio::test]
async fn untag_resource_removes_exactly_the_keys_the_request_lists() {
    for Case {
        label,
        initial,
        remove,
        expected,
    } in cases()
    {
        // A fresh emulator per row, so one row's resource cannot answer another's call.
        let harness = Harness::new("us-west-2");
        let arn = workflow_with_tags(&harness, &label, "tagged", Some(initial.clone())).await;

        let (status, answered) = untag(&harness, &arn, &remove).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected UntagResource to succeed, got {status} {answered:?}"
        );
        // `UntagResourceResponse` models no member, so a success carries none.
        assert!(
            answered
                .as_object()
                .is_some_and(|members| members.is_empty()),
            "case {label:?}: expected an empty response body, got {answered:?}"
        );

        let (status, tags) = list_tags(&harness, &arn).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected ListTagsForResource to succeed, got {status} {tags:?}"
        );
        assert_eq!(
            tags, expected,
            "case {label:?}: expected the tag set {expected:?} after removing {remove:?}, \
             got {tags:?}"
        );

        // `@idempotent`: repeating the very same removal answers the same success and
        // leaves the same tag set.
        let (status, again) = untag(&harness, &arn, &remove).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the repeated UntagResource to succeed, got {status} \
             {again:?}"
        );
        let (_, tags_again) = list_tags(&harness, &arn).await;
        assert_eq!(
            tags_again, expected,
            "case {label:?}: a repeated removal must leave the tag set at {expected:?}, got \
             {tags_again:?}"
        );

        // The resource itself survived its tags being removed.
        let (status, fetched) = harness
            .call("GetWorkflow", serde_json::json!({"WorkflowArn": arn}))
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: the resource must survive the removal, got {status} {fetched:?}"
        );
    }
}

#[tokio::test]
async fn untagging_one_resource_leaves_every_other_resource_alone() {
    let harness = Harness::new("eu-west-1");

    // Several resources in ONE store, with tag sets that overlap in their keys: the
    // removal is keyed by the resource the ARN names, never by the key alone.
    let rows: Vec<(&str, serde_json::Value)> = vec![
        ("first", serde_json::json!({"env": "prod", "owner": "alice"})),
        (
            "second",
            serde_json::json!({"env": "staging", "owner": "bob"}),
        ),
        ("third", generated_tags(5)),
    ];
    let mut stored: Vec<(String, String, serde_json::Value)> = Vec::new();
    for (name, tags) in rows {
        let arn = workflow_with_tags(&harness, name, name, Some(tags.clone())).await;
        stored.push((name.to_string(), arn, tags));
    }

    // Remove `env` from the FIRST resource only.
    let (label, target, _) = stored[0].clone();
    let (status, answered) = untag(&harness, &target, &["env".to_string()]).await;
    assert_eq!(
        status, 200,
        "case {label:?}: expected UntagResource to succeed, got {status} {answered:?}"
    );

    for (index, (label, arn, created)) in stored.iter().enumerate() {
        let expected = if index == 0 {
            let mut map = created.as_object().cloned().unwrap_or_default();
            map.remove("env");
            serde_json::Value::Object(map)
        } else {
            created.clone()
        };
        let (status, tags) = list_tags(&harness, arn).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected ListTagsForResource to succeed, got {status} {tags:?}"
        );
        assert_eq!(
            &tags, &expected,
            "case {label:?}: expected {expected:?} after untagging only the first resource, \
             got {tags:?}"
        );
    }

    // And the removal created or destroyed no resource.
    let (status, listed) = harness
        .call("ListWorkflows", serde_json::json!({"MaxResults": 100}))
        .await;
    assert_eq!(
        status, 200,
        "expected ListWorkflows to succeed, got {status} {listed:?}"
    );
    assert_eq!(
        listed["Workflows"].as_array().map(Vec::len),
        Some(stored.len()),
        "removing a tag must not create or remove a resource, got {:?}",
        listed["Workflows"]
    );
}

#[tokio::test]
async fn untagging_changes_nothing_but_the_tag_set() {
    let harness = Harness::new("us-east-1");
    let arn = workflow_with_tags(
        &harness,
        "configuration",
        "configured",
        Some(serde_json::json!({"env": "prod", "team": "data"})),
    )
    .await;

    let (status, before) = harness
        .call("GetWorkflow", serde_json::json!({"WorkflowArn": arn}))
        .await;
    assert_eq!(
        status, 200,
        "setup: expected GetWorkflow to succeed, got {status} {before:?}"
    );

    let (status, answered) = untag(&harness, &arn, &["env".to_string()]).await;
    assert_eq!(
        status, 200,
        "expected UntagResource to succeed, got {status} {answered:?}"
    );

    let (status, after) = harness
        .call("GetWorkflow", serde_json::json!({"WorkflowArn": arn}))
        .await;
    assert_eq!(
        status, 200,
        "expected GetWorkflow to succeed after the removal, got {status} {after:?}"
    );
    // The tag set is resource metadata: removing a tag publishes no version and moves
    // no part of the resource's configuration or its timestamps.
    for member in [
        "Name",
        "WorkflowArn",
        "Status",
        "WorkflowVersion",
        "RevisionId",
        "RoleArn",
        "DefinitionS3Location",
        "CreatedAt",
        "ModifiedAt",
    ] {
        assert_eq!(
            after[member], before[member],
            "case {member:?}: removing a tag must leave this member where it was, got {:?} \
             after {:?} before",
            after[member], before[member]
        );
    }

    let (status, versions) = harness
        .call(
            "ListWorkflowVersions",
            serde_json::json!({"WorkflowArn": arn}),
        )
        .await;
    assert_eq!(
        status, 200,
        "expected ListWorkflowVersions to succeed, got {status} {versions:?}"
    );
    assert_eq!(
        versions["WorkflowVersions"].as_array().map(Vec::len),
        Some(1),
        "removing a tag must publish no new version, got {:?}",
        versions["WorkflowVersions"]
    );
}

#[tokio::test]
async fn an_arn_this_endpoint_holds_no_resource_for_is_a_lookup_miss() {
    let harness = Harness::new("us-west-2");

    // Pattern-valid ARNs of resources that were never created: rows differ in region,
    // account and the form and length of the resource name.
    for arn in ghost_arns() {
        let (status, answered) = untag(&harness, &arn, &["env".to_string()]).await;
        assert_eq!(
            status, LOOKUP_MISS_STATUS,
            "case {arn:?}: expected HTTP {LOOKUP_MISS_STATUS}, got {status} {answered:?}"
        );
        assert!(
            answered["__type"]
                .as_str()
                .is_some_and(|kind| kind.contains("ResourceNotFoundException")),
            "case {arn:?}: expected a ResourceNotFoundException, got {:?}",
            answered["__type"]
        );
        assert_eq!(
            answered["Message"].as_str(),
            Some(WORKFLOW_MISSING),
            "case {arn:?}: expected the workflow-level message, got {:?}",
            answered["Message"]
        );
        assert_eq!(
            answered["ResourceId"].as_str(),
            Some(arn.as_str()),
            "case {arn:?}: expected ResourceId to name what was looked up, got {:?}",
            answered["ResourceId"]
        );
        assert_eq!(
            answered["ResourceType"].as_str(),
            Some("workflow"),
            "case {arn:?}: expected ResourceType \"workflow\", got {:?}",
            answered["ResourceType"]
        );
    }

    // A refused removal persisted nothing.
    let (status, listed) = harness
        .call("ListWorkflows", serde_json::json!({"MaxResults": 100}))
        .await;
    assert_eq!(
        status, 200,
        "expected ListWorkflows to succeed, got {status} {listed:?}"
    );
    assert_eq!(
        listed["Workflows"].as_array().map(Vec::len),
        Some(0),
        "a refused removal must create no resource, got {:?}",
        listed["Workflows"]
    );

    // The tags live with the resource: once it is gone, its ARN is a lookup miss here
    // too. `DeleteWorkflow` starts an asynchronous teardown, so "gone" is what the
    // store holds once the removal the clock owes has fallen due.
    let arn = workflow_with_tags(
        &harness,
        "deleted resource",
        "deletable",
        Some(serde_json::json!({"keep": "until-deleted"})),
    )
    .await;
    let (status, deleted) = harness
        .call("DeleteWorkflow", serde_json::json!({"WorkflowArn": arn}))
        .await;
    assert_eq!(
        status, 200,
        "case after delete: expected DeleteWorkflow to succeed, got {status} {deleted:?}"
    );
    tokio::time::sleep(WORKFLOW_DELETED_AFTER + Duration::from_millis(200)).await;
    let (status, answered) = untag(&harness, &arn, &["keep".to_string()]).await;
    assert_eq!(
        status, LOOKUP_MISS_STATUS,
        "case after delete: expected HTTP {LOOKUP_MISS_STATUS}, got {status} {answered:?}"
    );
    assert_eq!(
        answered["Message"].as_str(),
        Some(WORKFLOW_MISSING),
        "case after delete: expected the workflow-level message, got {:?}",
        answered["Message"]
    );
}

#[tokio::test]
async fn a_resource_of_another_region_cannot_be_untagged() {
    // ONE store, reached through two regional endpoints: a regional endpoint serves
    // only the resources created through it.
    let harness = Harness::new("us-west-2");
    let created = serde_json::json!({"env": "prod", "owner": "alice"});
    let arn = workflow_with_tags(&harness, "home region", "shared", Some(created.clone())).await;

    for foreign in ["eu-central-1", "ap-south-1"] {
        let (status, answered) = harness
            .call_in(
                foreign,
                "UntagResource",
                serde_json::json!({"ResourceArn": arn, "TagKeys": ["env"]}),
            )
            .await;
        assert_eq!(
            status, LOOKUP_MISS_STATUS,
            "case {foreign:?}: expected HTTP {LOOKUP_MISS_STATUS} for a resource of another \
             region, got {status} {answered:?}"
        );
        assert!(
            answered["__type"]
                .as_str()
                .is_some_and(|kind| kind.contains("ResourceNotFoundException")),
            "case {foreign:?}: expected a ResourceNotFoundException, got {:?}",
            answered["__type"]
        );

        let (_, tags) = list_tags(&harness, &arn).await;
        assert_eq!(
            tags, created,
            "case {foreign:?}: a refused removal must leave the tag set at {created:?}, got \
             {tags:?}"
        );
    }

    // The same request at the endpoint that holds the resource is served.
    let (status, answered) = untag(&harness, &arn, &["env".to_string()]).await;
    assert_eq!(
        status, 200,
        "case home endpoint: expected UntagResource to succeed, got {status} {answered:?}"
    );
    let (_, tags) = list_tags(&harness, &arn).await;
    assert_eq!(
        tags,
        serde_json::json!({"owner": "alice"}),
        "case home endpoint: expected only the untouched tag to remain, got {tags:?}"
    );
}

#[tokio::test]
async fn a_tag_keys_value_outside_its_modelled_constraint_removes_nothing() {
    let harness = Harness::new("us-east-1");
    let created = serde_json::json!({"env": "prod", "team": "data"});
    let arn = workflow_with_tags(&harness, "constraints", "constrained", Some(created.clone())).await;

    // Rows at and past each published bound of `TagKeys` (1..50 entries) and `TagKey`
    // (1..128 characters). `@pattern(^.*$)` admits every key form, so length and
    // entry count are the constraints a value can break.
    let too_many: Vec<String> = (0..=MAX_TAG_KEYS).map(|i| format!("key-{i:03}")).collect();
    let refused: Vec<(String, Vec<String>)> = vec![
        ("TagKeys is empty, below its minimum of 1".to_string(), vec![]),
        (
            format!(
                "{} keys, one past TagKeys' maximum of {MAX_TAG_KEYS}",
                too_many.len()
            ),
            too_many,
        ),
        (
            "a key below TagKey's minimum of 1 character".to_string(),
            vec![String::new()],
        ),
        (
            format!(
                "a key one past TagKey's maximum of {MAX_TAG_KEY_LEN} characters"
            ),
            vec!["k".repeat(MAX_TAG_KEY_LEN + 1)],
        ),
    ];

    for (label, keys) in refused {
        let (status, answered) = untag(&harness, &arn, &keys).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected HTTP 400, got {status} {answered:?}"
        );
        assert_eq!(
            answered["__type"].as_str(),
            Some("ValidationException"),
            "case {label:?}: expected a ValidationException, got {:?}",
            answered["__type"]
        );
        let field_list = answered["FieldList"].as_array().cloned().unwrap_or_default();
        assert!(
            field_list.iter().any(|entry| entry["Name"]
                .as_str()
                .is_some_and(|name| name.starts_with("TagKeys"))),
            "case {label:?}: expected a FieldList entry naming TagKeys, got {field_list:?}"
        );

        let (_, tags) = list_tags(&harness, &arn).await;
        assert_eq!(
            tags, created,
            "case {label:?}: a refused request must remove nothing, got {tags:?}"
        );
    }

    // And a request INSIDE the bounds still removes what it names: the refusals are
    // not a blanket one.
    let at_the_bound = vec!["k".repeat(MAX_TAG_KEY_LEN), "env".to_string()];
    let (status, answered) = untag(&harness, &arn, &at_the_bound).await;
    assert_eq!(
        status, 200,
        "case keys at TagKey's maximum: expected UntagResource to succeed, got {status} \
         {answered:?}"
    );
    let (_, tags) = list_tags(&harness, &arn).await;
    assert_eq!(
        tags,
        serde_json::json!({"team": "data"}),
        "case keys at TagKey's maximum: expected only the unnamed tag to remain, got {tags:?}"
    );
}
