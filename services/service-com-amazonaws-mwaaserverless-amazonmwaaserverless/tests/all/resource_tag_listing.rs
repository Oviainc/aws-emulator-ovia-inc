// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListTagsForResource`.
//!
//! RULE under test: `ListTagsForResource` REPORTS the tag set the named resource
//! currently carries in this service's store — every key-value pair that was persisted
//! on it and nothing else — as the modelled `Tags` map, which is
//! `@length(min: 0, max: 50)` so a resource carrying no tag answers an EMPTY map rather
//! than an absent member; the operation is `@readonly`, so it changes nothing a later
//! caller sees; and a `ResourceArn` no stored resource has is the modelled
//! `ResourceNotFoundException`.
//!
//! The table spans the class the rule generalises over: the NUMBER of tags (none, one,
//! several, the modelled maximum of 50), and the FORM and LENGTH of each key and value
//! (plain, punctuated, upper-case, numeric, spaced, non-ASCII, the empty value
//! `TagValue`'s minimum of 0 admits, and both published length maxima — `TagKey` 128 and
//! `TagValue` 256). Every case drives the real wire path through the shared `Harness`
//! (see `workflow_lifecycle`), so what is asserted is what a client sees.

use crate::missing_resource_lookups::{ghost_arns, LOOKUP_MISS_STATUS, WORKFLOW_MISSING};
use crate::workflow_lifecycle::{role_arn, Harness};

/// `Tags`' published entry maximum (`smithy_query(query="shape", shape="Tags")`:
/// `@length(min: 0, max: 50)`).
const MAX_TAGS: usize = 50;

/// `TagKey`'s published length maximum (`@length(min: 1, max: 128)` on the `Tags` key).
const MAX_TAG_KEY_LEN: usize = 128;

/// `TagValue`'s published length maximum (`@length(min: 0, max: 256)`).
const MAX_TAG_VALUE_LEN: usize = 256;

/// One row: a label, the `Tags` the create request carries (absent when `None`), and
/// the map `ListTagsForResource` must then report.
struct Case {
    label: String,
    create_tags: Option<serde_json::Value>,
    expected: serde_json::Value,
}

/// A `Tags` map of `count` generated entries, used for the entry-count rows.
fn generated_tags(count: usize) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for index in 0..count {
        map.insert(
            format!("key-{index:03}"),
            serde_json::Value::String(format!("value-{index:03}")),
        );
    }
    serde_json::Value::Object(map)
}

fn case(label: &str, create_tags: Option<serde_json::Value>, expected: serde_json::Value) -> Case {
    Case {
        label: label.to_string(),
        create_tags,
        expected,
    }
}

/// Every row of the table.
fn cases() -> Vec<Case> {
    let empty = serde_json::json!({});
    let mut cases = vec![
        // No `Tags` member at all, and an explicitly empty map: both are "no tags",
        // which the minimum length of 0 makes an empty map rather than an absence.
        case("the create request carries no Tags member", None, empty.clone()),
        case(
            "an explicitly empty Tags map",
            Some(empty.clone()),
            empty.clone(),
        ),
        case(
            "one tag",
            Some(serde_json::json!({"env": "prod"})),
            serde_json::json!({"env": "prod"}),
        ),
        case(
            // `TagValue` is @length(min: 0, …): the empty value is legal and must
            // come back as an entry, not be dropped.
            "a tag whose value is empty, TagValue's published minimum of 0",
            Some(serde_json::json!({"env": "", "team": "data"})),
            serde_json::json!({"env": "", "team": "data"}),
        ),
        case(
            // `TagKey`/`TagValue` are both @pattern(^.*$), so any form of text is a
            // legal key or value.
            "keys and values differing in form: punctuation, case, digits, spaces",
            Some(serde_json::json!({
                "Key.With-Dots_1": "a/b:c",
                "UPPER": "lower",
                "123": "456",
                "spaced key": "spaced value",
            })),
            serde_json::json!({
                "Key.With-Dots_1": "a/b:c",
                "UPPER": "lower",
                "123": "456",
                "spaced key": "spaced value",
            }),
        ),
        case(
            "non-ASCII keys and values",
            Some(serde_json::json!({"проект": "значение", "テスト": "値"})),
            serde_json::json!({"проект": "значение", "テスト": "値"}),
        ),
    ];

    // Both published length maxima, exactly at the bound.
    let mut longest = serde_json::Map::new();
    longest.insert(
        "k".repeat(MAX_TAG_KEY_LEN),
        serde_json::Value::String("v".repeat(MAX_TAG_VALUE_LEN)),
    );
    let longest = serde_json::Value::Object(longest);
    cases.push(case(
        "a key at TagKey's maximum of 128 and a value at TagValue's maximum of 256",
        Some(longest.clone()),
        longest,
    ));

    // Entry counts across the published range: one short of the maximum and the
    // maximum itself.
    for count in [MAX_TAGS - 1, MAX_TAGS] {
        let tags = generated_tags(count);
        cases.push(case(
            &format!("{count} tags, Tags' published entry range"),
            Some(tags.clone()),
            tags,
        ));
    }

    cases
}

/// Create a workflow carrying `tags` and report its ARN.
async fn workflow_with_tags(
    harness: &Harness,
    label: &str,
    name: &str,
    tags: Option<serde_json::Value>,
) -> String {
    let mut body = serde_json::json!({
        "Name": name,
        "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/tagged.yaml"},
        "RoleArn": role_arn(),
    });
    if let Some(tags) = tags {
        body["Tags"] = tags;
    }
    let (status, created) = harness.call("CreateWorkflow", body).await;
    assert_eq!(
        status, 200,
        "case {label:?}: expected CreateWorkflow to succeed, got {status} {created:?}"
    );
    created["WorkflowArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label:?}: a created workflow has an ARN: {created:?}"))
        .to_string()
}

/// Call `ListTagsForResource` and report its status and `Tags` member.
async fn list_tags(harness: &Harness, arn: &str) -> (u16, serde_json::Value) {
    let (status, answered) = harness
        .call(
            "ListTagsForResource",
            serde_json::json!({"ResourceArn": arn}),
        )
        .await;
    (status, answered["Tags"].clone())
}

#[tokio::test]
async fn list_tags_for_resource_reports_the_tag_set_the_resource_carries() {
    for Case {
        label,
        create_tags,
        expected,
    } in cases()
    {
        // A fresh emulator per row, so one row's workflow cannot answer another's read.
        let harness = Harness::new("us-west-2");
        let arn = workflow_with_tags(&harness, &label, "tagged", create_tags).await;

        let (status, tags) = list_tags(&harness, &arn).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected ListTagsForResource to succeed, got {status} {tags:?}"
        );
        assert_eq!(
            tags, expected,
            "case {label:?}: expected the persisted tag set {expected:?}, got {tags:?}"
        );

        // `@readonly`: the answer is stable, and the reporting call changed nothing a
        // later caller sees.
        let (status_again, again) = list_tags(&harness, &arn).await;
        assert_eq!(
            status_again, 200,
            "case {label:?}: expected the second read to succeed, got {status_again} {again:?}"
        );
        assert_eq!(
            again, expected,
            "case {label:?}: a read must not change what a later caller sees, got {again:?}"
        );
    }
}

#[tokio::test]
async fn each_resource_reports_only_its_own_tags() {
    let harness = Harness::new("eu-west-1");

    // Several workflows in ONE store, with tag sets differing in size and content:
    // the answer is keyed by the resource, never shared between them.
    let rows = vec![
        ("untagged", None),
        ("single", Some(serde_json::json!({"owner": "alice"}))),
        (
            "double",
            Some(serde_json::json!({"owner": "bob", "env": "staging"})),
        ),
        ("bulk", Some(generated_tags(7))),
    ];

    let mut stored: Vec<(String, String, serde_json::Value)> = Vec::new();
    for (name, tags) in rows {
        let expected = tags.clone().unwrap_or_else(|| serde_json::json!({}));
        let arn = workflow_with_tags(&harness, name, name, tags).await;
        stored.push((name.to_string(), arn, expected));
    }

    for (label, arn, expected) in &stored {
        let (status, tags) = list_tags(&harness, arn).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected ListTagsForResource to succeed, got {status} {tags:?}"
        );
        assert_eq!(
            &tags, expected,
            "case {label:?}: expected only this resource's tags {expected:?}, got {tags:?}"
        );
    }

    // And the reads left every resource exactly where it was: the whole set still
    // reports the same tags, and the store still holds every workflow.
    for (label, arn, expected) in &stored {
        let (_, tags) = list_tags(&harness, arn).await;
        assert_eq!(
            &tags, expected,
            "case {label:?}: repeated reads must stay identical, got {tags:?}"
        );
    }
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
        "listing tags must not create or remove a workflow, got {:?}",
        listed["Workflows"]
    );
}

#[tokio::test]
async fn an_arn_no_resource_has_is_a_lookup_miss() {
    let harness = Harness::new("us-west-2");

    // Pattern-valid ARNs of workflows that were never created: rows differ in region,
    // account and the form and length of the resource name.
    for arn in ghost_arns() {
        let (status, answered) = harness
            .call(
                "ListTagsForResource",
                serde_json::json!({"ResourceArn": arn}),
            )
            .await;
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

    // A resource that WAS stored answers its tags, and the same ARN becomes a lookup
    // miss once the resource is gone — the tags live with the resource, not beside it.
    let arn = workflow_with_tags(
        &harness,
        "deleted resource",
        "deletable",
        Some(serde_json::json!({"keep": "until-deleted"})),
    )
    .await;
    let (status, tags) = list_tags(&harness, &arn).await;
    assert_eq!(
        status, 200,
        "case stored resource: expected ListTagsForResource to succeed, got {status} {tags:?}"
    );
    assert_eq!(
        tags,
        serde_json::json!({"keep": "until-deleted"}),
        "case stored resource: expected the persisted tag, got {tags:?}"
    );

    let (status, deleted) = harness
        .call("DeleteWorkflow", serde_json::json!({"WorkflowArn": arn}))
        .await;
    assert_eq!(
        status, 200,
        "case stored resource: expected DeleteWorkflow to succeed, got {status} {deleted:?}"
    );

    let (status, answered) = harness
        .call(
            "ListTagsForResource",
            serde_json::json!({"ResourceArn": arn}),
        )
        .await;
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
