// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the tag set `CreateTask` records on the task it creates,
//! driven over HTTP through the same `MultiServiceRouter` the emulator boots
//! with, so every assertion is about the wire behaviour a real client sees.
//!
//! The rule these suites pin: a create that carries `tags` writes them onto the
//! resource it creates, so the ONE tag set that resource holds — the set
//! `ListTagsForResource` reports and `TagResource` / `UntagResource` edit — is
//! reached by the ARN the create answered with, while a create that carries no
//! `tags` leaves that resource an empty set rather than none.
//!
//! `DescribeTask` models no `tags` member (smithy_query query="operation"
//! operation="DescribeTask"), so the tag reader is the only way back to what a
//! task create recorded: the task slice's own suite cannot observe it, and the
//! tagging suite's resource table reaches no workspace-scoped resource. A task's
//! name is unique only within its workspace, so its ARN carries both — which is
//! what keeps two tasks of one name in two workspaces on two tag sets.
//!
//! The HTTP harness is `asset_lifecycle`'s, the task routes are
//! `task_lifecycle`'s and the tagging routes and their comparison helpers are
//! `resource_tags`': this suite reuses all three rather than growing a second
//! copy of any of them.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{
    MAX_TAGS_PER_RESOURCE, TAG_KEY_MAX_LENGTH, TAG_KEY_MIN_LENGTH,
};

use crate::asset_lifecycle::router;
use crate::resource_tags::{
    expected, generated_pairs, keys, pairs, stored_tags, tag_resource, tags_json, untag_resource,
};
use crate::task_lifecycle::{create, describe, minimal_container, PROCESSING_TYPES, PROCESSING_UNITS};

/// The workspace these suites create their tasks in.
const WORKSPACE: &str = "tagged-floor";

/// A second workspace, for the ARN-scoping case.
const OTHER_WORKSPACE: &str = "tagged-annex";

/// `TagValue`'s published maximum length (smithy_query query="shape"
/// shape="TagValue": `@length(min: 0, max: 256)`).
const TAG_VALUE_MAX_LENGTH: usize = 256;

/// One row of the create-time table: a label and the `tags` member the create
/// carries, `None` for a create that sends no `tags` at all.
#[derive(Debug)]
struct TagCase {
    label: &'static str,
    tags: Option<Vec<(String, String)>>,
}

/// The rows, differing along what the rule generalises over: whether the create
/// carries `tags` at all, HOW MANY pairs it carries — up to `TagMap`'s published
/// maximum — and the FORM of key and value, including each published length
/// bound and a multi-byte pair, since `@length` counts characters.
fn create_cases() -> Vec<TagCase> {
    let longest_key = "k".repeat(TAG_KEY_MAX_LENGTH);
    let shortest_key = "k".repeat(TAG_KEY_MIN_LENGTH);
    let longest_value = "v".repeat(TAG_VALUE_MAX_LENGTH);
    vec![
        TagCase {
            label: "no tags member at all",
            tags: None,
        },
        TagCase {
            label: "one pair",
            tags: Some(pairs(&[("Stage", "production")])),
        },
        TagCase {
            label: "three pairs",
            tags: Some(pairs(&[
                ("Stage", "test"),
                ("Owner", "plant-1"),
                ("Line", "a"),
            ])),
        },
        TagCase {
            label: "empty value at TagValue's minimum length",
            tags: Some(pairs(&[("Empty", "")])),
        },
        TagCase {
            label: "multi-byte key and value",
            tags: Some(pairs(&[("温度", "25 °C — 施設")])),
        },
        TagCase {
            label: "key and value at their maximum lengths",
            tags: Some(vec![
                (longest_key, longest_value),
                (shortest_key, "one".to_string()),
            ]),
        },
        TagCase {
            label: "the whole per-resource ceiling in one create",
            tags: Some(generated_pairs(MAX_TAGS_PER_RESOURCE)),
        },
    ]
}

/// The body one row sends: the members `CreateTask` requires, plus the row's
/// `tags` when it has any.
fn create_body(case: &TagCase, name: &str) -> Value {
    let mut body = json!({
        "taskName": name,
        "taskConfiguration": minimal_container(PROCESSING_TYPES[0], PROCESSING_UNITS[0]),
    });
    if let Some(rows) = case.tags.as_deref() {
        body.as_object_mut()
            .expect("an object body")
            .insert("tags".to_string(), tags_json(rows));
    }
    body
}

/// `CreateTask` in `workspace`, answering the created task's ARN.
async fn created_task_arn(
    router: &MultiServiceRouter,
    workspace: &str,
    body: Value,
    case: &str,
) -> String {
    let (status, created) = create(router, workspace, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateTask must answer 200, got {status} {created}"
    );
    created["taskArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no taskArn in {created}"))
        .to_string()
}

/// Read the task back through `DescribeTask`, asserting the create persisted it
/// under the name and the ARN the create answered with.
async fn assert_persisted(
    router: &MultiServiceRouter,
    workspace: &str,
    name: &str,
    arn: &str,
    case: &str,
) {
    let (status, described) = describe(router, workspace, name, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeTask must answer 200, got {status} {described}"
    );
    assert_eq!(
        described["taskName"].as_str().unwrap_or(""),
        name,
        "case {case:?}: the stored task must carry the created name, got {described}"
    );
    assert_eq!(
        described["taskArn"].as_str().unwrap_or(""),
        arn,
        "case {case:?}: the stored task must carry the ARN the create answered, got {described}"
    );
}

// =============================================================================
// 1. The tags a create carries are the tag set the reader reports for it
// =============================================================================

#[tokio::test]
async fn a_created_task_reports_exactly_the_tags_its_create_carried() {
    let router = router();

    for (at, case) in create_cases().into_iter().enumerate() {
        let name = format!("tagged-task-{at}");
        let label = case.label;
        let arn = created_task_arn(&router, WORKSPACE, create_body(&case, &name), label).await;

        assert_persisted(&router, WORKSPACE, &name, &arn, label).await;

        let got = stored_tags(&router, &arn, label).await;
        let want = case.tags.as_deref().map(expected).unwrap_or_default();
        assert_eq!(
            got, want,
            "case {label:?}: the listed tags must be exactly the ones the create carried, \
             got {got:?}"
        );

        // The read is a report, not a mutation: a second read answers the same.
        let again = stored_tags(&router, &arn, label).await;
        assert_eq!(
            again, got,
            "case {label:?}: reading the tags must not change what a later caller sees, \
             got {again:?} then {got:?}"
        );
    }
}

// =============================================================================
// 2. One tag set per task: the writers edit the set the create seeded
// =============================================================================

#[tokio::test]
async fn tags_written_after_the_create_edit_the_set_the_create_seeded() {
    let router = router();
    let case = "a task created with tags";
    let name = "seeded-task";

    let seeded = pairs(&[("Seeded", "at-create"), ("Stage", "test")]);
    let body = create_body(
        &TagCase {
            label: case,
            tags: Some(seeded.clone()),
        },
        name,
    );
    let arn = created_task_arn(&router, WORKSPACE, body, case).await;

    let got = stored_tags(&router, &arn, case).await;
    assert_eq!(
        got,
        expected(&seeded),
        "case {case:?}: the create-time tags must be the ones listed, got {got:?}"
    );

    // An upsert over them: one key edited, one added, one untouched.
    let (status, written) =
        tag_resource(&router, &arn, json!({ "Stage": "production", "Owner": "ops" })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: TagResource must answer 200, got {status} {written}"
    );
    let got = stored_tags(&router, &arn, case).await;
    assert_eq!(
        got,
        expected(&pairs(&[
            ("Seeded", "at-create"),
            ("Stage", "production"),
            ("Owner", "ops"),
        ])),
        "case {case:?}: an edited key takes the new value, a new key is added and an \
         unnamed key is kept, got {got:?}"
    );

    // `UntagResource` removes exactly the keys it names — a key the task does not
    // hold removes nothing — and leaves the task itself readable.
    let (status, removed) = untag_resource(&router, &arn, &keys(&["Stage", "NeverSet"])).await;
    assert_eq!(
        status, 200,
        "case {case:?}: UntagResource must answer 200, got {status} {removed}"
    );
    let got = stored_tags(&router, &arn, case).await;
    assert_eq!(
        got,
        expected(&pairs(&[("Seeded", "at-create"), ("Owner", "ops")])),
        "case {case:?}: only the named keys the task held are removed, got {got:?}"
    );
    assert_persisted(&router, WORKSPACE, name, &arn, case).await;
}

// =============================================================================
// 3. A task's ARN carries its workspace, so one name is two tag sets
// =============================================================================

#[tokio::test]
async fn two_tasks_of_one_name_in_two_workspaces_hold_two_tag_sets() {
    let router = router();
    let name = "shared-name";

    let here = pairs(&[("Where", "here")]);
    let here_case = "the task in the first workspace";
    let there_case = "the task in the second workspace";
    let here_arn = created_task_arn(
        &router,
        WORKSPACE,
        create_body(
            &TagCase {
                label: here_case,
                tags: Some(here.clone()),
            },
            name,
        ),
        here_case,
    )
    .await;
    let there_arn = created_task_arn(
        &router,
        OTHER_WORKSPACE,
        create_body(
            &TagCase {
                label: there_case,
                tags: None,
            },
            name,
        ),
        there_case,
    )
    .await;
    assert_ne!(
        here_arn, there_arn,
        "one task name in two workspaces must be two ARNs, got {here_arn} twice"
    );

    let got = stored_tags(&router, &here_arn, here_case).await;
    assert_eq!(
        got,
        expected(&here),
        "case {here_case:?}: it reports its own create-time tags, got {got:?}"
    );
    let got = stored_tags(&router, &there_arn, there_case).await;
    assert!(
        got.is_empty(),
        "case {there_case:?}: a create carrying no tags leaves an empty set, got {got:?}"
    );

    // A write addressed to one of them is not a write to the other.
    let (status, written) = tag_resource(&router, &there_arn, json!({ "Where": "there" })).await;
    assert_eq!(
        status, 200,
        "case {there_case:?}: TagResource must answer 200, got {status} {written}"
    );
    let got = stored_tags(&router, &here_arn, here_case).await;
    assert_eq!(
        got,
        expected(&here),
        "case {here_case:?}: a write to the other workspace's task leaves this one's tags \
         untouched, got {got:?}"
    );
}
