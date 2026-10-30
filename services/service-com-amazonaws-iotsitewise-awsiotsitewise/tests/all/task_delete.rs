// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteTask`, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with, so every assertion is about the
//! wire behaviour a real client sees.
//!
//! The rules these suites pin:
//!
//! * a delete REMOVES the task the request names and answers its lifecycle
//!   state, whatever that task holds — any of the configurations
//!   `TaskConfiguration` admits, a description and tags or none, its first
//!   version or a history of them — so a later read of it resolves to nothing, it
//!   drops out of the listing, its ARN stops carrying tags, a second delete is the
//!   modelled 404 and the name it held is free again;
//! * a task a pipeline of the same workspace still references on a compute node is
//!   NOT deleted: that is the modelled 400, naming the compute node and the
//!   pipeline that reference it, and the refused call leaves the task exactly as
//!   it was — once the reference is gone the same delete goes through;
//! * a delete is addressed by `(workspaceName, taskName)`, so it removes only the
//!   task that pair names and resolves its path outside in: the same name in
//!   another workspace survives (and a reference held there blocks nothing), a
//!   name no task holds is the modelled 404 for the task, and a workspace nothing
//!   opened is the modelled 404 for the workspace.
//!
//! The HTTP harness (`call`, `router`, `error_code`) is the one the asset suite
//! already has, and the task routes and body builders are `task_lifecycle`'s; this
//! suite reuses both rather than growing a second copy.

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::FIRST_TASK_VERSION;

use crate::asset_lifecycle::{call, error_code, router, REGION};
use crate::task_lifecycle::{
    create as create_task, describe as describe_task, minimal_container, state, task_uri,
    tasks_uri, EXECUTION_ROLE, PROCESSING_TYPES, PROCESSING_UNITS, PUBLIC_IMAGE,
};

/// The workspace every suite below works in unless it is about workspace scoping.
const WORKSPACE: &str = "task-delete-workspace";

/// The ARN form this emulator mints for a task: the workspace as well as the
/// name, since the name is unique only within the workspace.
fn expected_arn(workspace: &str, name: &str) -> String {
    format!(
        "arn:aws:iotsitewise:{REGION}:{}:task/{workspace}/{name}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// A JSON object of `pairs`, for a tag or environment-variable map.
fn map_of(pairs: &[(&str, &str)]) -> Value {
    Value::Object(
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), json!(value)))
            .collect::<Map<String, Value>>(),
    )
}

/// `DeleteTask`.
async fn delete_task(router: &MultiServiceRouter, workspace: &str, name: &str) -> (u16, Value) {
    call(router, "DELETE", &task_uri(workspace, name), None).await
}

/// `ListTasks` in `workspace`.
async fn list_tasks(router: &MultiServiceRouter, workspace: &str) -> (u16, Value) {
    call(router, "GET", &tasks_uri(workspace), None).await
}

/// The `taskName`s a `ListTasks` body reports.
fn listed_names(body: &Value) -> Vec<String> {
    body["taskSummaries"]
        .as_array()
        .map(|summaries| {
            summaries
                .iter()
                .map(|summary| summary["taskName"].as_str().unwrap_or("").to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// `ListTagsForResource` on `arn` — the reader that proves a deleted task's ARN
/// stops resolving.
async fn list_tags(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    call(router, "GET", &format!("/tags?resourceArn={arn}"), None).await
}

/// `UpdateTask` with `body`, so a row can give its task a version history.
async fn update_task(
    router: &MultiServiceRouter,
    workspace: &str,
    name: &str,
    body: Value,
) -> (u16, Value) {
    call(router, "PUT", &task_uri(workspace, name), Some(body)).await
}

/// `CreatePipeline` with `body`.
async fn create_pipeline(
    router: &MultiServiceRouter,
    workspace: &str,
    body: Value,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/workspaces/{workspace}/pipelines"),
        Some(body),
    )
    .await
}

/// `UpdatePipeline` with `body`.
async fn update_pipeline(
    router: &MultiServiceRouter,
    workspace: &str,
    name: &str,
    body: Value,
) -> (u16, Value) {
    call(
        router,
        "PUT",
        &format!("/workspaces/{workspace}/pipelines/{name}"),
        Some(body),
    )
    .await
}

/// `DeletePipeline`.
async fn delete_pipeline(router: &MultiServiceRouter, workspace: &str, name: &str) -> (u16, Value) {
    call(
        router,
        "DELETE",
        &format!("/workspaces/{workspace}/pipelines/{name}"),
        None,
    )
    .await
}

/// One compute node: its name, the task it runs and the nodes it waits for.
fn node(name: &str, task: &str, depends_on: &[&str]) -> Value {
    let mut value = json!({ "computeNodeName": name, "taskName": task });
    if !depends_on.is_empty() {
        value
            .as_object_mut()
            .expect("an object node")
            .insert("dependsOn".to_string(), json!(depends_on));
    }
    value
}

/// A `containerTaskConfiguration` carrying the optional members too, so a row can
/// delete a task whose configuration is more than the required minimum
/// (smithy_query query="shape" shape="ContainerTaskConfiguration": `command`,
/// `timeoutSeconds`, `environmentVariables`, `ephemeralStorageConfiguration`).
fn full_container(processing_type: &str, processing_unit: &str) -> Value {
    json!({
        "containerTaskConfiguration": {
            "ecrUri": PUBLIC_IMAGE,
            "taskExecutionRole": EXECUTION_ROLE,
            "processingType": processing_type,
            "processingUnit": processing_unit,
            "command": ["python", "main.py"],
            // The published minimum of `timeoutSeconds` (@range(min: 60, max: 86400)).
            "timeoutSeconds": 60,
            "environmentVariables": map_of(&[("STAGE", "test")]),
            "ephemeralStorageConfiguration": {
                // The first `StorageClass` value (smithy_query query="shape"
                // shape="StorageClass") and the published minimum of
                // `storageSizeInGiB` (@range(min: 1, max: 16384)).
                "storageClass": "STANDARD_1",
                "storageSizeInGiB": 1,
            },
        }
    })
}

/// One row of the reference table: the pipelines it creates, and the reference
/// the refusal has to name.
#[derive(Debug)]
struct ReferenceCase {
    label: &'static str,
    /// The pipelines the row creates, as `(pipelineName, computations)`. The
    /// FIRST is the one the row rewrites to remove the reference; the rest it
    /// deletes outright.
    pipelines: Vec<(&'static str, Value)>,
    /// The pipeline and the compute node the refusal must name: the smallest
    /// `(pipeline, node)` pair of the row that references the task.
    expected_pipeline: &'static str,
    expected_node: &'static str,
}

/// The rows: WHERE the reference sits — the only node of a pipeline, one node of
/// several, a node in the middle of a chain, and the same task referenced by two
/// pipelines at once.
fn reference_cases() -> Vec<ReferenceCase> {
    vec![
        ReferenceCase {
            label: "the-only-node",
            pipelines: vec![("solo", json!([node("only", "referenced", &[])]))],
            expected_pipeline: "solo",
            expected_node: "only",
        },
        ReferenceCase {
            label: "one-node-of-several",
            pipelines: vec![(
                "mixed",
                json!([
                    node("first", "replacement", &[]),
                    node("second", "referenced", &[]),
                ]),
            )],
            expected_pipeline: "mixed",
            expected_node: "second",
        },
        ReferenceCase {
            label: "the-middle-of-a-chain",
            pipelines: vec![(
                "chain",
                json!([
                    node("extract", "replacement", &[]),
                    node("transform", "referenced", &["extract"]),
                    node("load", "replacement", &["transform"]),
                ]),
            )],
            expected_pipeline: "chain",
            expected_node: "transform",
        },
        ReferenceCase {
            label: "two-pipelines",
            pipelines: vec![
                ("alpha", json!([node("run", "referenced", &[])])),
                ("beta", json!([node("run", "referenced", &[])])),
            ],
            expected_pipeline: "alpha",
            expected_node: "run",
        },
    ]
}

/// One row of the delete table: the task a row creates, and how many updates land
/// on it before the delete.
#[derive(Debug)]
struct DeleteCase {
    label: &'static str,
    configuration: Value,
    description: Option<&'static str>,
    tags: Option<Value>,
    /// How many `UpdateTask` calls land before the delete, so the row spans a task
    /// at its first version and one with a version history.
    updates: usize,
}

/// The rows: the configuration spanning the only union member `TaskConfiguration`
/// declares (smithy_query query="shape" shape="TaskConfiguration") at its required
/// minimum and with every optional member set, over both `ProcessingType` values
/// and the first and last `ProcessingUnit` (smithy_query query="shape"
/// shape="ProcessingType" / shape="ProcessingUnit"), crossed with a present /
/// absent description, with / without tags, and at one version or several.
fn delete_cases() -> Vec<DeleteCase> {
    vec![
        DeleteCase {
            label: "minimal-no-members",
            configuration: minimal_container(PROCESSING_TYPES[0], PROCESSING_UNITS[0]),
            description: None,
            tags: None,
            updates: 0,
        },
        DeleteCase {
            label: "minimal-described-and-tagged",
            configuration: minimal_container(
                PROCESSING_TYPES[1],
                PROCESSING_UNITS[PROCESSING_UNITS.len() - 1],
            ),
            description: Some("the accelerated rollup"),
            tags: Some(map_of(&[("owner", "plant-team")])),
            updates: 0,
        },
        DeleteCase {
            label: "full-container-after-one-update",
            configuration: full_container(PROCESSING_TYPES[0], PROCESSING_UNITS[3]),
            description: Some("every optional member set"),
            tags: None,
            updates: 1,
        },
        DeleteCase {
            label: "full-container-after-three-updates",
            configuration: full_container(PROCESSING_TYPES[1], PROCESSING_UNITS[7]),
            description: None,
            tags: Some(map_of(&[("owner", "plant-team"), ("tier", "gold")])),
            updates: 3,
        },
    ]
}

/// The create body a row sends, with every absent member OMITTED rather than sent
/// as null.
fn create_body(case: &DeleteCase, name: &str) -> Value {
    let mut body = json!({ "taskName": name, "taskConfiguration": case.configuration });
    let object = body.as_object_mut().expect("an object body");
    if let Some(description) = case.description {
        object.insert("description".to_string(), json!(description));
    }
    if let Some(tags) = case.tags.clone() {
        object.insert("tags".to_string(), tags);
    }
    body
}

#[tokio::test]
async fn a_delete_removes_the_task_it_names() {
    let router = router();

    // A task no row touches: every delete below must leave it exactly where it is.
    let (status, bystander) = create_task(
        &router,
        WORKSPACE,
        json!({
            "taskName": "bystander",
            "description": "the task that stays",
            "taskConfiguration": minimal_container(PROCESSING_TYPES[0], PROCESSING_UNITS[0]),
        }),
    )
    .await;
    assert_eq!(status, 200, "create the bystander: got {status} {bystander}");

    for case in delete_cases() {
        let label = case.label;
        let (status, created) = create_task(&router, WORKSPACE, create_body(&case, label)).await;
        assert_eq!(
            status, 200,
            "case {label:?}: create expected 200, got {status} {created}"
        );
        for at in 0..case.updates {
            let (status, updated) = update_task(
                &router,
                WORKSPACE,
                label,
                json!({ "description": format!("revision {at}") }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {label:?}: update {at} expected 200, got {status} {updated}"
            );
        }

        // The task is there to begin with: the delete below is about a resource
        // the store really holds.
        let (status, before) = describe_task(&router, WORKSPACE, label, None).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the task must exist before the delete, got {status} {before}"
        );

        let (status, deleted) = delete_task(&router, WORKSPACE, label).await;
        assert_eq!(
            status, 200,
            "case {label:?}: delete expected 200, got {status} {deleted}"
        );
        assert_eq!(
            state(&deleted),
            "DELETING",
            "case {label:?}: the delete must report the removed task's lifecycle state, \
             got {deleted}"
        );

        // Removed, so the reader resolves to nothing rather than to a stale copy.
        let (status, gone) = describe_task(&router, WORKSPACE, label, None).await;
        assert_eq!(
            status, 404,
            "case {label:?}: a describe after the delete expected 404, got {status} {gone}"
        );
        assert_eq!(
            error_code(&gone),
            "ResourceNotFoundException",
            "case {label:?}: unexpected error code after the delete, got {gone}"
        );

        // And a read naming the version it held resolves to nothing either: the
        // record its versions lived on is gone.
        let (status, old_version) = describe_task(
            &router,
            WORKSPACE,
            label,
            Some(&FIRST_TASK_VERSION.to_string()),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {label:?}: a versioned describe after the delete expected 404, \
             got {status} {old_version}"
        );

        // It drops out of the listing, so no summary reports a task nothing can
        // address.
        let (status, listed) = list_tasks(&router, WORKSPACE).await;
        assert_eq!(
            status, 200,
            "case {label:?}: ListTasks expected 200, got {status} {listed}"
        );
        let names = listed_names(&listed);
        assert!(
            !names.contains(&label.to_string()),
            "case {label:?}: the deleted task must not be listed, got {names:?}"
        );
        assert!(
            names.contains(&"bystander".to_string()),
            "case {label:?}: the bystander must still be listed, got {names:?}"
        );

        // Its tags went with its record, so its ARN carries none and resolves to
        // nothing.
        let arn = expected_arn(WORKSPACE, label);
        let (status, tags) = list_tags(&router, &arn).await;
        assert_eq!(
            status, 404,
            "case {label:?}: the deleted task's ARN must stop resolving, got {status} {tags}"
        );

        // A second delete addresses a task the store no longer holds.
        let (status, again) = delete_task(&router, WORKSPACE, label).await;
        assert_eq!(
            status, 404,
            "case {label:?}: a repeated delete expected 404, got {status} {again}"
        );
        assert_eq!(
            error_code(&again),
            "ResourceNotFoundException",
            "case {label:?}: unexpected error code on the repeated delete, got {again}"
        );

        // The name it held is free again, and the task that takes it starts from
        // the first version with its OWN configuration rather than the deleted
        // one's.
        let (status, recreated) = create_task(
            &router,
            WORKSPACE,
            json!({
                "taskName": label,
                "taskConfiguration": minimal_container(
                    PROCESSING_TYPES[0],
                    PROCESSING_UNITS[1],
                ),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: recreating the deleted name expected 200, got {status} {recreated}"
        );
        assert_eq!(
            recreated["version"].as_str().unwrap_or(""),
            FIRST_TASK_VERSION.to_string(),
            "case {label:?}: the recreated task must start at the first version, got {recreated}"
        );
        let (status, read_back) = describe_task(&router, WORKSPACE, label, None).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the recreated task must read back, got {status} {read_back}"
        );
        assert_eq!(
            read_back.get("description"),
            None,
            "case {label:?}: the recreated task must not inherit the deleted one's \
             description, got {read_back}"
        );
        assert_eq!(
            read_back["taskConfiguration"]["containerTaskConfiguration"]["processingUnit"]
                .as_str()
                .unwrap_or(""),
            PROCESSING_UNITS[1],
            "case {label:?}: the recreated task must hold its OWN configuration, \
             got {read_back}"
        );

        // Clean up so the next row starts from the same store, and so the delete
        // of a just-recreated task is exercised too.
        let (status, cleaned) = delete_task(&router, WORKSPACE, label).await;
        assert_eq!(
            status, 200,
            "case {label:?}: deleting the recreated task expected 200, got {status} {cleaned}"
        );

        // Every row leaves the bystander alone.
        let (status, survivor) = describe_task(&router, WORKSPACE, "bystander", None).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the bystander must survive, got {status} {survivor}"
        );
        assert_eq!(
            survivor["description"].as_str().unwrap_or(""),
            "the task that stays",
            "case {label:?}: the bystander must keep its own configuration, got {survivor}"
        );
    }
}

#[tokio::test]
async fn a_task_a_pipeline_references_is_not_deleted() {
    let router = router();

    // The task every row below refuses to delete, and the one the rows rewrite
    // their pipelines onto so the reference can be removed.
    for name in ["referenced", "replacement"] {
        let (status, created) = create_task(
            &router,
            WORKSPACE,
            json!({
                "taskName": name,
                "taskConfiguration": minimal_container(
                    PROCESSING_TYPES[0],
                    PROCESSING_UNITS[0],
                ),
            }),
        )
        .await;
        assert_eq!(status, 200, "create task {name}: got {status} {created}");
    }

    // Each row states the compute node and the pipeline the refusal has to name:
    // the smallest `(pipeline, node)` pair referencing the task, so one store
    // state always states one message.
    for case in reference_cases() {
        let ReferenceCase {
            label,
            pipelines,
            expected_pipeline,
            expected_node,
        } = &case;
        for (name, computations) in pipelines {
            let (status, created) = create_pipeline(
                &router,
                WORKSPACE,
                json!({ "pipelineName": name, "computations": computations }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {label:?}: create pipeline {name} expected 200, got {status} {created}"
            );
        }

        let (status, refused) = delete_task(&router, WORKSPACE, "referenced").await;
        assert_eq!(
            status, 400,
            "case {label:?}: deleting a referenced task expected 400, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "InvalidRequestException",
            "case {label:?}: unexpected error code, got {refused}"
        );
        let expected_message = format!(
            "The task referenced is referenced by the compute node {expected_node} of the \
             pipeline {expected_pipeline}. Remove the task from all pipelines before \
             attempting to delete it."
        );
        assert_eq!(
            refused["message"].as_str().unwrap_or(""),
            expected_message,
            "case {label:?}: the refusal must name the reference that blocks the delete, \
             expected {expected_message:?}, got {refused}"
        );

        // The refused delete wrote nothing: the task is still readable, at the
        // version and with the configuration it had.
        let (status, survivor) = describe_task(&router, WORKSPACE, "referenced", None).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the refused delete must leave the task, got {status} {survivor}"
        );
        assert_eq!(
            survivor["version"].as_str().unwrap_or(""),
            FIRST_TASK_VERSION.to_string(),
            "case {label:?}: the refused delete must not bump the version, got {survivor}"
        );

        // Removing the reference is what unblocks the delete: the first pipeline
        // is rewritten onto the other task, the rest are deleted outright.
        for (at, (name, _)) in pipelines.iter().enumerate() {
            if at == 0 {
                let (status, updated) = update_pipeline(
                    &router,
                    WORKSPACE,
                    name,
                    json!({ "computations": [node("only", "replacement", &[])] }),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {label:?}: rewriting pipeline {name} expected 200, \
                     got {status} {updated}"
                );
            } else {
                let (status, removed) = delete_pipeline(&router, WORKSPACE, name).await;
                assert_eq!(
                    status, 200,
                    "case {label:?}: deleting pipeline {name} expected 200, \
                     got {status} {removed}"
                );
            }
        }

        let (status, deleted) = delete_task(&router, WORKSPACE, "referenced").await;
        assert_eq!(
            status, 200,
            "case {label:?}: with no reference left the delete expected 200, \
             got {status} {deleted}"
        );
        assert_eq!(
            state(&deleted),
            "DELETING",
            "case {label:?}: the delete must report the removed task's lifecycle state, \
             got {deleted}"
        );

        // Put the row's fixtures back for the next row: the task it refuses to
        // delete, and a store holding no pipeline.
        let (status, recreated) = create_task(
            &router,
            WORKSPACE,
            json!({
                "taskName": "referenced",
                "taskConfiguration": minimal_container(
                    PROCESSING_TYPES[0],
                    PROCESSING_UNITS[0],
                ),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: recreating the referenced task expected 200, \
             got {status} {recreated}"
        );
        let (status, removed) = delete_pipeline(&router, WORKSPACE, pipelines[0].0).await;
        assert_eq!(
            status, 200,
            "case {label:?}: cleaning up pipeline {} expected 200, got {status} {removed}",
            pipelines[0].0
        );
    }

    // A pipeline whose CURRENT version no longer names the task blocks nothing,
    // even though the version it superseded still does: what a pipeline
    // references is the definition a start of it would run.
    let (status, created) = create_pipeline(
        &router,
        WORKSPACE,
        json!({
            "pipelineName": "rewritten",
            "computations": [node("only", "referenced", &[])],
        }),
    )
    .await;
    assert_eq!(status, 200, "create the rewritten pipeline: got {status} {created}");
    let (status, updated) = update_pipeline(
        &router,
        WORKSPACE,
        "rewritten",
        json!({ "computations": [node("only", "replacement", &[])] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "rewriting the pipeline onto the other task: got {status} {updated}"
    );
    let (status, deleted) = delete_task(&router, WORKSPACE, "referenced").await;
    assert_eq!(
        status, 200,
        "a task only a superseded pipeline version names must delete, got {status} {deleted}"
    );
}

#[tokio::test]
async fn a_delete_removes_only_the_workspace_and_name_it_addresses() {
    let router = router();
    // One task name, held in two workspaces: a task name is unique only within
    // its workspace, so the pair is what the delete addresses.
    let name = "shared-name";
    for workspace in ["plant-a", "plant-b"] {
        let (status, created) = create_task(
            &router,
            workspace,
            json!({
                "taskName": name,
                "description": workspace,
                "taskConfiguration": minimal_container(
                    PROCESSING_TYPES[0],
                    PROCESSING_UNITS[0],
                ),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "create {workspace}/{name}: got {status} {created}"
        );
    }

    // `plant-b` holds a pipeline referencing ITS task of that name, so this also
    // pins that the reference check is scoped to the workspace the delete
    // addresses: it must not block `plant-a`'s delete.
    let (status, created) = create_pipeline(
        &router,
        "plant-b",
        json!({ "pipelineName": "only-here", "computations": [node("run", name, &[])] }),
    )
    .await;
    assert_eq!(status, 200, "create plant-b's pipeline: got {status} {created}");

    let (status, deleted) = delete_task(&router, "plant-a", name).await;
    assert_eq!(
        status, 200,
        "deleting plant-a/{name} expected 200, got {status} {deleted}"
    );

    let (status, survivor) = describe_task(&router, "plant-b", name, None).await;
    assert_eq!(
        status, 200,
        "case \"other workspace\": plant-b/{name} must survive, got {status} {survivor}"
    );
    assert_eq!(
        survivor["description"].as_str().unwrap_or(""),
        "plant-b",
        "case \"other workspace\": plant-b/{name} must keep its own configuration, got {survivor}"
    );
    // And plant-b's own delete is still refused while its pipeline references it.
    let (status, refused) = delete_task(&router, "plant-b", name).await;
    assert_eq!(
        status, 400,
        "case \"other workspace\": plant-b's referenced task must not delete, \
         got {status} {refused}"
    );

    // A task no create ever made: an unknown name in a workspace that holds
    // another task, and a known name in a workspace nothing created at all —
    // where the workspace is the outermost resource the request got wrong, so the
    // refusal names it rather than the task inside it.
    for (workspace, task, expected) in [
        (
            "plant-b",
            "never-created",
            "Task with ID never-created not found".to_string(),
        ),
        (
            "plant-c",
            name,
            "Workspace with ID plant-c not found".to_string(),
        ),
    ] {
        let (status, body) = delete_task(&router, workspace, task).await;
        assert_eq!(
            status, 404,
            "case {workspace:?}/{task:?}: delete expected 404, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {workspace:?}/{task:?}: unexpected error code, got {body}"
        );
        assert_eq!(
            body["message"].as_str().unwrap_or(""),
            expected,
            "case {workspace:?}/{task:?}: the refusal must name the resource the request \
             got wrong, expected {expected:?}, got {body}"
        );
    }
}
