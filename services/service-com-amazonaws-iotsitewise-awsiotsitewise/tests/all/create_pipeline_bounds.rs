// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the bounds `CreatePipeline` enforces before it writes,
//! driven over HTTP through the same `MultiServiceRouter` the emulator boots with,
//! so every assertion is about the wire behaviour a real client sees.
//!
//! `pipeline_lifecycle.rs` pins what an ACCEPTED create persists and reads back;
//! this suite pins the two refusals that stand between a request and that write,
//! and that each of them leaves the store exactly as it was:
//!
//! * the pipeline store's entry count is bounded PER WORKSPACE, so a create past
//!   that many pipelines in one workspace is a quota refusal (the modelled
//!   `LimitExceededException`, HTTP 410) while the very same name still creates in
//!   a workspace that is not full;
//! * a `tags` key must satisfy the length bound the model puts on `TagKey`
//!   (smithy_query query="shape" shape="TagMap": key target `TagKey`,
//!   `@length(min: 1, max: 128)`), so a key inside the bound is persisted on the
//!   pipeline and a key outside it is refused (`InvalidRequestException`, HTTP 400)
//!   with no pipeline stored.
//!
//! The route, ARN and request helpers are `pipeline_lifecycle.rs`'s and the tag
//! reader is `resource_tags.rs`'s, imported rather than copied so the suites
//! cannot drift on the wire form; the HTTP harness (`router`, `error_code`) is the
//! one the asset suite already has.
//!
//! COST NOTE: the quota suite below drives [`MAX_PIPELINES_PER_WORKSPACE`]
//! sequential creates to reach the ceiling, and relies on the global
//! [`MAX_PIPELINES`] staying above it so the overflow is refused by the
//! per-workspace bound and a second workspace can still create. Raising either
//! constant changes what this suite costs and what it proves.

use serde_json::{json, Value};

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{
    FIRST_PIPELINE_VERSION, MAX_PIPELINES, MAX_PIPELINES_PER_WORKSPACE, TAG_KEY_MAX_LENGTH,
    TAG_KEY_MIN_LENGTH,
};

use crate::asset_lifecycle::{error_code, router};
use crate::pipeline_lifecycle::{create, describe, expected_arn};
use crate::resource_tags::list_tags;

/// The smallest well-formed `computations` a create can carry: `ComputeNodeList`
/// is `@length(min: 0, max: 50)`, so an empty DAG is a valid one.
fn no_nodes() -> Value {
    json!([])
}

#[tokio::test]
async fn the_pipeline_quota_is_counted_per_workspace() {
    let router = router();
    let full = "full-workspace";
    // What this suite proves holds only while the store's total ceiling leaves room
    // beyond one workspace's: otherwise the overflow below would be refused by the
    // total bound and the second workspace could not create at all.
    const { assert!(MAX_PIPELINES > MAX_PIPELINES_PER_WORKSPACE) };

    // Fill one workspace right up to its ceiling. Every create inside the bound is
    // accepted, so the refusal below is about the count and nothing else.
    for index in 0..MAX_PIPELINES_PER_WORKSPACE {
        let name = format!("held-{index}");
        let (status, body) = create(
            &router,
            full,
            json!({ "pipelineName": name, "computations": no_nodes() }),
        )
        .await;
        assert_eq!(
            status, 200,
            "pipeline {index} of {MAX_PIPELINES_PER_WORKSPACE}: expected 200, got {status} {body}"
        );
    }

    // One past the ceiling is a quota refusal, not a conflict: the name is free.
    let overflow = "one-too-many";
    let (status, refused) = create(
        &router,
        full,
        json!({ "pipelineName": overflow, "computations": no_nodes() }),
    )
    .await;
    assert_eq!(
        status, 410,
        "a create past the per-workspace ceiling: expected 410, got {status} {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "LimitExceededException",
        "a create past the per-workspace ceiling: unexpected error code in {refused}"
    );

    // The refused create wrote nothing.
    let (status, body) = describe(&router, full, overflow, None).await;
    assert_eq!(
        status, 404,
        "a refused create must store no pipeline, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "a refused create must store no pipeline, got {body}"
    );

    // And it left the pipelines the workspace already held untouched.
    for index in [0, MAX_PIPELINES_PER_WORKSPACE - 1] {
        let name = format!("held-{index}");
        let (status, body) = describe(&router, full, &name, None).await;
        assert_eq!(
            status, 200,
            "case {name:?}: a held pipeline must survive a refused create, got {status} {body}"
        );
        assert_eq!(
            body["version"].as_str().unwrap_or(""),
            FIRST_PIPELINE_VERSION.to_string(),
            "case {name:?}: a refused create must not bump a held pipeline, got {body}"
        );
    }

    // The ceiling is that workspace's entry count, so the very name it refused
    // creates in a workspace that is not full.
    let (status, created) = create(
        &router,
        "roomy-workspace",
        json!({ "pipelineName": overflow, "computations": no_nodes() }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the quota is per workspace: expected 200 in an empty workspace, got {status} {created}"
    );
    assert_eq!(
        created["pipelineArn"].as_str().unwrap_or(""),
        expected_arn("roomy-workspace", overflow),
        "the quota is per workspace: unexpected pipelineArn in {created}"
    );
}

/// One row of the tag-key table: the key a create sends and whether the modelled
/// `TagKey` length bound admits it.
#[derive(Debug)]
struct TagKeyCase {
    label: &'static str,
    key: String,
    accepted: bool,
}

/// The rows: the published bounds of `TagKey`'s `@length` (1 and 128, from
/// smithy_query query="shape" shape="TagMap"), one key inside the range that the
/// sibling suites do not use, and one key past each bound.
fn tag_key_cases() -> Vec<TagKeyCase> {
    vec![
        TagKeyCase {
            label: "the shortest key the bound admits",
            key: "k".repeat(TAG_KEY_MIN_LENGTH),
            accepted: true,
        },
        TagKeyCase {
            label: "a key well inside the bound",
            key: "cost-centre:analytics".to_string(),
            accepted: true,
        },
        TagKeyCase {
            label: "the longest key the bound admits",
            key: "k".repeat(TAG_KEY_MAX_LENGTH),
            accepted: true,
        },
        TagKeyCase {
            label: "one character past the upper bound",
            key: "k".repeat(TAG_KEY_MAX_LENGTH + 1),
            accepted: false,
        },
        TagKeyCase {
            label: "one character short of the lower bound",
            key: "k".repeat(TAG_KEY_MIN_LENGTH - 1),
            accepted: false,
        },
    ]
}

#[tokio::test]
async fn a_create_enforces_the_modelled_tag_key_bound_before_it_writes() {
    let router = router();
    let workspace = "tagged-workspace";

    for (index, case) in tag_key_cases().into_iter().enumerate() {
        let name = format!("tagged-{index}");
        let tags = json!({ case.key.clone(): "pinned" });
        let (status, body) = create(
            &router,
            workspace,
            json!({
                "pipelineName": name,
                "computations": no_nodes(),
                "tags": tags.clone(),
            }),
        )
        .await;

        if !case.accepted {
            assert_eq!(
                status, 400,
                "case {:?}: a key outside the bound expects 400, got {status} {body}",
                case.label
            );
            assert_eq!(
                error_code(&body),
                "InvalidRequestException",
                "case {:?}: unexpected error code in {body}",
                case.label
            );
            let (status, described) = describe(&router, workspace, &name, None).await;
            assert_eq!(
                status, 404,
                "case {:?}: a refused create must store no pipeline, got {status} {described}",
                case.label
            );
            assert_eq!(
                error_code(&described),
                "ResourceNotFoundException",
                "case {:?}: a refused create must store no pipeline, got {described}",
                case.label
            );
            continue;
        }

        assert_eq!(
            status, 200,
            "case {:?}: a key inside the bound expects 200, got {status} {body}",
            case.label
        );

        // An accepted create persists the tags on the pipeline it wrote: they read
        // back through `ListTagsForResource` under the ARN the create answered.
        let arn = body["pipelineArn"].as_str().unwrap_or("").to_string();
        assert_eq!(
            arn,
            expected_arn(workspace, &name),
            "case {:?}: unexpected pipelineArn in {body}",
            case.label
        );
        let (status, listed) = list_tags(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {:?}: ListTagsForResource expected 200, got {status} {listed}",
            case.label
        );
        assert_eq!(
            listed["tags"], tags,
            "case {:?}: the create's tags must read back, got {listed}",
            case.label
        );
    }
}
