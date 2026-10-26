// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `TagResource`.
//!
//! RULE under test: `TagResource` MERGES the key-value pairs the request carries into
//! the tag set the named resource holds in this service's store — a key the resource
//! already carries takes the value just sent, a key it does not carry is added, and
//! every key the request does not name keeps the value it had — so the set a later read
//! reports is the union of what was there and what was sent; the union carries at most
//! the modelled `Tags` entry maximum of 50 tags, and a call that would carry the
//! resource past it is refused with the modelled `ValidationException` and writes
//! nothing; the merge touches no other resource and no part of the resource beyond its
//! tag set; and a `ResourceArn` this endpoint holds no resource for is the modelled
//! `ResourceNotFoundException`.
//!
//! The table spans the class the rule generalises over: HOW MANY pairs a request carries
//! (none, one, several, and a whole set at the modelled maximum), whether each key is
//! carried already (added, overwritten, mixed), the resulting entry COUNT across the
//! published range (up to and at 50, and past it), and the FORM and LENGTH of each key
//! and value (plain, punctuated, upper-case, numeric, spaced, non-ASCII, the empty value
//! `TagValue`'s minimum of 0 admits, and both published length maxima — `TagKey` 128 and
//! `TagValue` 256). Every tag set is read back through the implemented
//! `ListTagsForResource`, so what is asserted is what a client sees.

use std::time::Duration;

use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::state::{
    MAX_TAGS_PER_RESOURCE, WORKFLOW_DELETED_AFTER,
};

use crate::missing_resource_lookups::{ghost_arns, LOOKUP_MISS_STATUS, WORKFLOW_MISSING};
use crate::resource_tag_listing::{generated_tags, list_tags, workflow_with_tags};
use crate::workflow_lifecycle::Harness;

/// `TagKey`'s published length maximum (`smithy_query(query="shape", shape="Tags")`:
/// the key targets `TagKey`, `@length(min: 1, max: 128)`).
const MAX_TAG_KEY_LEN: usize = 128;

/// `TagValue`'s published length maximum (`@length(min: 0, max: 256)`).
const MAX_TAG_VALUE_LEN: usize = 256;

/// One row: a label, the tag set the resource is created with, the `Tags` the tagging
/// request carries, and the tag set the resource must carry afterwards.
struct Case {
    label: String,
    initial: serde_json::Value,
    add: serde_json::Value,
    expected: serde_json::Value,
}

fn case(
    label: &str,
    initial: serde_json::Value,
    add: serde_json::Value,
    expected: serde_json::Value,
) -> Case {
    Case {
        label: label.to_string(),
        initial,
        add,
        expected,
    }
}

/// A `Tags` map of `count` generated entries whose values carry `mark`, so a row can
/// re-send the SAME keys with different values.
fn marked_tags(count: usize, mark: &str) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for index in 0..count {
        map.insert(
            format!("key-{index:03}"),
            serde_json::Value::String(format!("{mark}-{index:03}")),
        );
    }
    serde_json::Value::Object(map)
}

/// The union of two `Tags` maps, with `added` winning on a shared key — what the rule
/// says the resource must carry afterwards.
fn union(carried: &serde_json::Value, added: &serde_json::Value) -> serde_json::Value {
    let mut merged = carried.as_object().cloned().unwrap_or_default();
    for (key, value) in added.as_object().cloned().unwrap_or_default() {
        merged.insert(key, value);
    }
    serde_json::Value::Object(merged)
}

/// Every row of the table.
fn cases() -> Vec<Case> {
    let empty = serde_json::json!({});
    let mut cases = vec![
        case(
            "one tag onto a resource carrying none",
            empty.clone(),
            serde_json::json!({"env": "prod"}),
            serde_json::json!({"env": "prod"}),
        ),
        case(
            "a new key beside the one the resource carries",
            serde_json::json!({"env": "prod"}),
            serde_json::json!({"team": "data"}),
            serde_json::json!({"env": "prod", "team": "data"}),
        ),
        case(
            // A key already carried takes the value just sent: the pair is assigned,
            // not duplicated.
            "overwriting the value of a carried key",
            serde_json::json!({"env": "prod", "team": "data"}),
            serde_json::json!({"env": "staging"}),
            serde_json::json!({"env": "staging", "team": "data"}),
        ),
        case(
            "overwriting one key and adding another in one call",
            serde_json::json!({"env": "prod", "team": "data"}),
            serde_json::json!({"env": "dev", "owner": "alice"}),
            serde_json::json!({"env": "dev", "team": "data", "owner": "alice"}),
        ),
        case(
            // `Tags` is `@length(min: 0, …)`: a request carrying no pair is legal and
            // adds nothing.
            "an empty Tags map, the published minimum of 0 entries",
            serde_json::json!({"env": "prod"}),
            empty.clone(),
            serde_json::json!({"env": "prod"}),
        ),
        case(
            "an empty Tags map onto a resource carrying no tag",
            empty.clone(),
            empty.clone(),
            empty.clone(),
        ),
        case(
            // `TagValue` is `@length(min: 0, …)`: the empty value is legal and must be
            // stored as an entry rather than dropped.
            "a value that is empty, TagValue's published minimum of 0",
            serde_json::json!({"env": "prod"}),
            serde_json::json!({"env": "", "team": ""}),
            serde_json::json!({"env": "", "team": ""}),
        ),
        case(
            // `TagKey`/`TagValue` are both `@pattern(^.*$)`, so any form of text is a
            // legal key or value.
            "keys and values differing in form: punctuation, case, digits, spaces",
            serde_json::json!({"carried": "kept"}),
            serde_json::json!({
                "Key.With-Dots_1": "a/b:c",
                "UPPER": "lower",
                "123": "456",
                "spaced key": "spaced value",
            }),
            serde_json::json!({
                "carried": "kept",
                "Key.With-Dots_1": "a/b:c",
                "UPPER": "lower",
                "123": "456",
                "spaced key": "spaced value",
            }),
        ),
        case(
            "non-ASCII keys and values",
            serde_json::json!({"проект": "старое"}),
            serde_json::json!({"проект": "значение", "テスト": "値"}),
            serde_json::json!({"проект": "значение", "テスト": "値"}),
        ),
    ];

    // Both published length maxima, exactly at the bound.
    let longest_key = "k".repeat(MAX_TAG_KEY_LEN);
    let longest_value = "v".repeat(MAX_TAG_VALUE_LEN);
    let mut longest = serde_json::Map::new();
    longest.insert(
        longest_key,
        serde_json::Value::String(longest_value.clone()),
    );
    let longest = serde_json::Value::Object(longest);
    cases.push(case(
        "a key at TagKey's maximum of 128 and a value at TagValue's maximum of 256",
        serde_json::json!({"carried": "kept"}),
        longest.clone(),
        union(&serde_json::json!({"carried": "kept"}), &longest),
    ));

    // Entry counts across the published range: a whole set at the maximum in one call,
    // the same keys re-sent with new values at the maximum (the union does not grow),
    // and one short of the maximum topped up to it.
    let full = generated_tags(MAX_TAGS_PER_RESOURCE);
    cases.push(case(
        &format!("{MAX_TAGS_PER_RESOURCE} pairs, Tags' published entry maximum, in one call"),
        serde_json::json!({}),
        full.clone(),
        full.clone(),
    ));
    let recoloured = marked_tags(MAX_TAGS_PER_RESOURCE, "again");
    cases.push(case(
        &format!("the same {MAX_TAGS_PER_RESOURCE} keys re-sent with new values"),
        full.clone(),
        recoloured.clone(),
        recoloured,
    ));
    let almost = generated_tags(MAX_TAGS_PER_RESOURCE - 1);
    let last_key = format!("key-{:03}", MAX_TAGS_PER_RESOURCE - 1);
    let top_up = serde_json::json!({last_key.clone(): "topped-up"});
    cases.push(case(
        &format!(
            "{} carried, one more makes exactly {MAX_TAGS_PER_RESOURCE}",
            MAX_TAGS_PER_RESOURCE - 1
        ),
        almost.clone(),
        top_up.clone(),
        union(&almost, &top_up),
    ));

    cases
}

/// Call `TagResource` and report its status and body.
async fn tag(harness: &Harness, arn: &str, tags: &serde_json::Value) -> (u16, serde_json::Value) {
    harness
        .call(
            "TagResource",
            serde_json::json!({"ResourceArn": arn, "Tags": tags}),
        )
        .await
}

/// How many entries a `Tags` map holds.
fn entry_count(tags: &serde_json::Value) -> usize {
    tags.as_object().map_or(0, serde_json::Map::len)
}

#[tokio::test]
async fn tag_resource_merges_the_requested_pairs_into_the_tag_set() {
    for Case {
        label,
        initial,
        add,
        expected,
    } in cases()
    {
        // A fresh emulator per row, so one row's resource cannot answer another's call.
        let harness = Harness::new("us-west-2");
        let arn = workflow_with_tags(&harness, &label, "tagged", Some(initial.clone())).await;

        let (status, answered) = tag(&harness, &arn, &add).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected TagResource to succeed, got {status} {answered:?}"
        );
        // `TagResourceResponse` models no member, so a success carries none.
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
            "case {label:?}: expected the tag set {expected:?} after adding {add:?} to \
             {initial:?}, got {tags:?}"
        );

        // Sending the very same pairs again assigns the same values, so the set stays
        // where it is.
        let (status, again) = tag(&harness, &arn, &add).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the repeated TagResource to succeed, got {status} \
             {again:?}"
        );
        let (_, tags_again) = list_tags(&harness, &arn).await;
        assert_eq!(
            tags_again, expected,
            "case {label:?}: a repeated merge must leave the tag set at {expected:?}, got \
             {tags_again:?}"
        );

        // The resource itself survived being tagged.
        let (status, fetched) = harness
            .call("GetWorkflow", serde_json::json!({"WorkflowArn": arn}))
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: the resource must survive the merge, got {status} {fetched:?}"
        );
    }
}

#[tokio::test]
async fn tagging_one_resource_leaves_every_other_resource_alone() {
    let harness = Harness::new("eu-west-1");

    // Several resources in ONE store, with tag sets that overlap in their keys: the
    // merge is keyed by the resource the ARN names, never by the key alone.
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

    // Tag the FIRST resource only, overwriting one of its keys and adding one.
    let added = serde_json::json!({"env": "qa", "team": "data"});
    let (label, target, carried) = stored[0].clone();
    let (status, answered) = tag(&harness, &target, &added).await;
    assert_eq!(
        status, 200,
        "case {label:?}: expected TagResource to succeed, got {status} {answered:?}"
    );

    for (index, (label, arn, created)) in stored.iter().enumerate() {
        let expected = if index == 0 {
            union(&carried, &added)
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
            "case {label:?}: expected {expected:?} after tagging only the first resource, \
             got {tags:?}"
        );
    }

    // And the merge created or destroyed no resource.
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
        "adding a tag must not create or remove a resource, got {:?}",
        listed["Workflows"]
    );
}

#[tokio::test]
async fn tagging_changes_nothing_but_the_tag_set() {
    let harness = Harness::new("us-east-1");
    let arn = workflow_with_tags(
        &harness,
        "configuration",
        "configured",
        Some(serde_json::json!({"env": "prod"})),
    )
    .await;

    let (status, before) = harness
        .call("GetWorkflow", serde_json::json!({"WorkflowArn": arn}))
        .await;
    assert_eq!(
        status, 200,
        "setup: expected GetWorkflow to succeed, got {status} {before:?}"
    );

    let (status, answered) = tag(
        &harness,
        &arn,
        &serde_json::json!({"env": "staging", "team": "data"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected TagResource to succeed, got {status} {answered:?}"
    );

    let (status, after) = harness
        .call("GetWorkflow", serde_json::json!({"WorkflowArn": arn}))
        .await;
    assert_eq!(
        status, 200,
        "expected GetWorkflow to succeed after the merge, got {status} {after:?}"
    );
    // The tag set is resource metadata: adding a tag publishes no version and moves no
    // part of the resource's configuration or its timestamps.
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
            "case {member:?}: adding a tag must leave this member where it was, got {:?} \
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
        "adding a tag must publish no new version, got {:?}",
        versions["WorkflowVersions"]
    );
}

#[tokio::test]
async fn an_arn_this_endpoint_holds_no_resource_for_is_a_lookup_miss() {
    let harness = Harness::new("us-west-2");

    // Pattern-valid ARNs of resources that were never created: rows differ in region,
    // account and the form and length of the resource name.
    for arn in ghost_arns() {
        let (status, answered) = tag(&harness, &arn, &serde_json::json!({"env": "prod"})).await;
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

    // A refused merge persisted nothing.
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
        "a refused merge must create no resource, got {:?}",
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
    let (status, answered) = tag(&harness, &arn, &serde_json::json!({"env": "prod"})).await;
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
async fn a_resource_of_another_region_cannot_be_tagged() {
    // ONE store, reached through two regional endpoints: a regional endpoint serves
    // only the resources created through it.
    let harness = Harness::new("us-west-2");
    let created = serde_json::json!({"env": "prod"});
    let arn = workflow_with_tags(&harness, "home region", "shared", Some(created.clone())).await;

    for foreign in ["eu-central-1", "ap-south-1"] {
        let (status, answered) = harness
            .call_in(
                foreign,
                "TagResource",
                serde_json::json!({"ResourceArn": arn, "Tags": {"env": "foreign"}}),
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
            "case {foreign:?}: a refused merge must leave the tag set at {created:?}, got \
             {tags:?}"
        );
    }

    // The same request at the endpoint that holds the resource is served.
    let (status, answered) = tag(&harness, &arn, &serde_json::json!({"env": "home"})).await;
    assert_eq!(
        status, 200,
        "case home endpoint: expected TagResource to succeed, got {status} {answered:?}"
    );
    let (_, tags) = list_tags(&harness, &arn).await;
    assert_eq!(
        tags,
        serde_json::json!({"env": "home"}),
        "case home endpoint: expected the merged value, got {tags:?}"
    );
}

#[tokio::test]
async fn a_tags_value_outside_its_modelled_constraint_stores_nothing() {
    let harness = Harness::new("us-east-1");
    let created = serde_json::json!({"env": "prod"});
    let arn =
        workflow_with_tags(&harness, "constraints", "constrained", Some(created.clone())).await;

    // Rows at and past each published bound the walk checks on `Tags`: its entry count
    // (0..50) and each VALUE's `TagValue` length (0..256). A map KEY is not walked,
    // which is the framework's own rule for a map shape, so no row asserts a key
    // refusal here.
    let refused: Vec<(String, serde_json::Value)> = vec![
        (
            format!(
                "{} pairs, one past Tags' maximum of {MAX_TAGS_PER_RESOURCE}",
                MAX_TAGS_PER_RESOURCE + 1
            ),
            generated_tags(MAX_TAGS_PER_RESOURCE + 1),
        ),
        (
            format!(
                "a value one past TagValue's maximum of {MAX_TAG_VALUE_LEN} characters"
            ),
            serde_json::json!({"env": "v".repeat(MAX_TAG_VALUE_LEN + 1)}),
        ),
        (
            "one legal pair beside a value past TagValue's maximum".to_string(),
            serde_json::json!({
                "team": "data",
                "env": "v".repeat(MAX_TAG_VALUE_LEN + 1),
            }),
        ),
    ];

    for (label, tags) in refused {
        let (status, answered) = tag(&harness, &arn, &tags).await;
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
                .is_some_and(|name| name.starts_with("Tags"))),
            "case {label:?}: expected a FieldList entry naming Tags, got {field_list:?}"
        );

        let (_, held) = list_tags(&harness, &arn).await;
        assert_eq!(
            held, created,
            "case {label:?}: a refused request must store nothing, got {held:?}"
        );
    }

    // And a request INSIDE the bounds still stores what it carries: the refusals are
    // not a blanket one. A value exactly at the published maximum is legal.
    let at_the_bound = serde_json::json!({"env": "v".repeat(MAX_TAG_VALUE_LEN)});
    let (status, answered) = tag(&harness, &arn, &at_the_bound).await;
    assert_eq!(
        status, 200,
        "case a value at TagValue's maximum: expected TagResource to succeed, got {status} \
         {answered:?}"
    );
    let (_, tags) = list_tags(&harness, &arn).await;
    assert_eq!(
        tags, at_the_bound,
        "case a value at TagValue's maximum: expected the stored value, got {tags:?}"
    );
}

#[tokio::test]
async fn a_resource_carries_at_most_the_modelled_maximum_of_tags() {
    let harness = Harness::new("us-west-2");

    // Filled to the ceiling across TWO calls, so the bound is counted over what the
    // resource ends up carrying rather than over one request's map.
    let half = MAX_TAGS_PER_RESOURCE / 2;
    let first = marked_tags(half, "first");
    let arn = workflow_with_tags(&harness, "ceiling", "ceiling", Some(first.clone())).await;

    let mut rest = serde_json::Map::new();
    for index in half..MAX_TAGS_PER_RESOURCE {
        rest.insert(
            format!("key-{index:03}"),
            serde_json::Value::String(format!("second-{index:03}")),
        );
    }
    let rest = serde_json::Value::Object(rest);
    let (status, answered) = tag(&harness, &arn, &rest).await;
    assert_eq!(
        status, 200,
        "case filling to the ceiling: expected TagResource to succeed, got {status} \
         {answered:?}"
    );
    let at_ceiling = union(&first, &rest);
    let (_, held) = list_tags(&harness, &arn).await;
    assert_eq!(
        held, at_ceiling,
        "case filling to the ceiling: expected {MAX_TAGS_PER_RESOURCE} tags, got {held:?}"
    );
    assert_eq!(
        entry_count(&held),
        MAX_TAGS_PER_RESOURCE,
        "case filling to the ceiling: expected {MAX_TAGS_PER_RESOURCE} entries, got {}",
        entry_count(&held)
    );

    // One key more than the ceiling admits, in requests of different sizes: each is
    // refused and stores NOTHING, not even the pairs that would have fitted.
    let overflowing: Vec<(String, serde_json::Value)> = vec![
        (
            "one new key past the ceiling".to_string(),
            serde_json::json!({"overflow": "one"}),
        ),
        (
            "a carried key re-sent beside a new one past the ceiling".to_string(),
            serde_json::json!({"key-000": "rewritten", "overflow": "two"}),
        ),
        (
            "several new keys past the ceiling".to_string(),
            serde_json::json!({"a": "1", "b": "2", "c": "3"}),
        ),
    ];
    for (label, tags) in overflowing {
        let (status, answered) = tag(&harness, &arn, &tags).await;
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
        assert!(
            answered["Message"]
                .as_str()
                .is_some_and(|message| message.contains(&MAX_TAGS_PER_RESOURCE.to_string())
                    && message.contains(&arn)),
            "case {label:?}: expected the message to name the ceiling and the resource, got \
             {:?}",
            answered["Message"]
        );
        let field_list = answered["FieldList"].as_array().cloned().unwrap_or_default();
        assert!(
            field_list
                .iter()
                .any(|entry| entry["Name"].as_str() == Some("Tags")),
            "case {label:?}: expected a FieldList entry naming Tags, got {field_list:?}"
        );

        let (_, held) = list_tags(&harness, &arn).await;
        assert_eq!(
            held, at_ceiling,
            "case {label:?}: a refused merge must leave the tag set untouched, got {held:?}"
        );
    }

    // At the ceiling, a call that only REWRITES carried keys is still served: the bound
    // is on the size of the union, which such a call does not grow.
    let rewrite = marked_tags(MAX_TAGS_PER_RESOURCE, "rewritten");
    let (status, answered) = tag(&harness, &arn, &rewrite).await;
    assert_eq!(
        status, 200,
        "case rewriting at the ceiling: expected TagResource to succeed, got {status} \
         {answered:?}"
    );
    let (_, held) = list_tags(&harness, &arn).await;
    assert_eq!(
        held, rewrite,
        "case rewriting at the ceiling: expected the rewritten values, got {held:?}"
    );

    // And once a key is removed, the key that did not fit does.
    let (status, answered) = harness
        .call(
            "UntagResource",
            serde_json::json!({"ResourceArn": arn, "TagKeys": ["key-000"]}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case making room: expected UntagResource to succeed, got {status} {answered:?}"
    );
    let (status, answered) = tag(&harness, &arn, &serde_json::json!({"overflow": "fits"})).await;
    assert_eq!(
        status, 200,
        "case after making room: expected TagResource to succeed, got {status} {answered:?}"
    );
    let (_, held) = list_tags(&harness, &arn).await;
    assert_eq!(
        entry_count(&held),
        MAX_TAGS_PER_RESOURCE,
        "case after making room: expected {MAX_TAGS_PER_RESOURCE} entries, got {}",
        entry_count(&held)
    );
    assert_eq!(
        held["overflow"].as_str(),
        Some("fits"),
        "case after making room: expected the new tag to be stored, got {:?}",
        held["overflow"]
    );
}
