// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeletePipeline`, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with, so every assertion is about the
//! wire behaviour a real client sees.
//!
//! The rules these suites pin:
//!
//! * a delete REMOVES the pipeline the request names and answers its lifecycle
//!   state, whatever that pipeline holds — no compute nodes or many, a
//!   description, environment variables and tags or none, one version or a
//!   history of them — so a later read of it resolves to nothing, a second
//!   delete of it is the modelled 404, and the name it held is free again;
//! * a pipeline with an execution that has not finished is NOT deleted: that is
//!   the modelled 409, naming the pipeline it refuses to remove, and the refused
//!   call leaves the pipeline, its version and its executions exactly as they
//!   were;
//! * a delete is addressed by `(workspaceName, pipelineName)`, so it removes
//!   only the pipeline that pair names: the same name in another workspace
//!   survives, a name no pipeline holds is the modelled 404, and another
//!   pipeline's executions stay readable.
//!
//! The HTTP harness (`call`, `router`, `error_code`, `is_modelled_id`) is the one
//! the asset suite already has; this suite reuses it rather than growing a
//! second.

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::asset_lifecycle::{call, error_code, is_modelled_id, router, REGION};

/// The workspace every suite below works in unless it is about workspace
/// scoping.
const WORKSPACE: &str = "delete-workspace";

/// The task every compute node of these suites runs.
const TASK: &str = "rollup-task";

/// A container image URI inside the modelled `EcrUri` pattern.
const ECR_URI: &str = "123456789012.dkr.ecr.us-west-2.amazonaws.com/pipeline-task:v1";

/// A role ARN inside the modelled `IamRoleArn` pattern.
const TASK_ROLE: &str = "arn:aws:iam::123456789012:role/sitewise-task";

/// `POST /workspaces/{workspaceName}/pipelines`.
fn pipelines_uri(workspace: &str) -> String {
    format!("/workspaces/{workspace}/pipelines")
}

/// `GET|PUT|DELETE /workspaces/{workspaceName}/pipelines/{pipelineName}`.
fn pipeline_uri(workspace: &str, name: &str) -> String {
    format!("/workspaces/{workspace}/pipelines/{name}")
}

/// The ARN form this emulator mints for a pipeline: the workspace as well as the
/// name, since the name is unique only within the workspace.
fn expected_arn(workspace: &str, name: &str) -> String {
    format!(
        "arn:aws:iotsitewise:{REGION}:{}:pipeline/{workspace}/{name}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// A JSON object of `pairs`, for an environment-variable or tag map.
fn map_of(pairs: &[(&str, &str)]) -> Value {
    Value::Object(
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), json!(value)))
            .collect::<Map<String, Value>>(),
    )
}

/// One compute node: its name, the task it runs and the nodes it waits for.
fn node(name: &str, depends_on: &[&str]) -> Value {
    let mut value = json!({ "computeNodeName": name, "taskName": TASK });
    if !depends_on.is_empty() {
        value
            .as_object_mut()
            .expect("an object node")
            .insert("dependsOn".to_string(), json!(depends_on));
    }
    value
}

/// Create a workspace with the service's default encryption.
async fn create_workspace(router: &MultiServiceRouter, name: &str) {
    let (status, body) = call(
        router,
        "POST",
        "/workspaces",
        Some(json!({
            "workspaceName": name,
            "encryptionConfiguration": { "encryptionType": "SITEWISE_DEFAULT_ENCRYPTION" },
        })),
    )
    .await;
    assert_eq!(status, 200, "create workspace {name}: got {status} {body}");
}

/// Create the task every compute node below runs.
async fn create_task(router: &MultiServiceRouter, workspace: &str) {
    let (status, body) = call(
        router,
        "POST",
        &format!("/workspaces/{workspace}/tasks"),
        Some(json!({
            "taskName": TASK,
            "taskConfiguration": {
                "containerTaskConfiguration": {
                    "ecrUri": ECR_URI,
                    "taskExecutionRole": TASK_ROLE,
                    "processingType": "GENERIC_COMPUTE_PROCESSING",
                    "processingUnit": "UNITS_2",
                }
            },
        })),
    )
    .await;
    assert_eq!(status, 200, "create task in {workspace}: got {status} {body}");
}

/// `CreatePipeline` with `body`.
async fn create(router: &MultiServiceRouter, workspace: &str, body: Value) -> (u16, Value) {
    call(router, "POST", &pipelines_uri(workspace), Some(body)).await
}

/// `UpdatePipeline` with `body`.
async fn update(
    router: &MultiServiceRouter,
    workspace: &str,
    name: &str,
    body: Value,
) -> (u16, Value) {
    call(router, "PUT", &pipeline_uri(workspace, name), Some(body)).await
}

/// `DescribePipeline` at the latest version.
async fn describe(router: &MultiServiceRouter, workspace: &str, name: &str) -> (u16, Value) {
    call(router, "GET", &pipeline_uri(workspace, name), None).await
}

/// `DeletePipeline`.
async fn delete(router: &MultiServiceRouter, workspace: &str, name: &str) -> (u16, Value) {
    call(router, "DELETE", &pipeline_uri(workspace, name), None).await
}

/// `StartPipelineExecution` with `body`, returning the raw `(status, body)`.
async fn start_execution(
    router: &MultiServiceRouter,
    workspace: &str,
    pipeline: &str,
    body: Value,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/workspaces/{workspace}/pipelines/{pipeline}/executions"),
        Some(body),
    )
    .await
}

/// Start an execution and return its id.
async fn start(
    router: &MultiServiceRouter,
    workspace: &str,
    pipeline: &str,
    body: Value,
) -> String {
    let (status, answer) = start_execution(router, workspace, pipeline, body).await;
    assert_eq!(status, 202, "start execution: got {status} {answer}");
    let id = answer["pipelineExecutionId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        is_modelled_id(&id),
        "start execution: {id:?} is not a modelled ID"
    );
    id
}

/// `DescribePipelineExecution`.
async fn describe_execution(
    router: &MultiServiceRouter,
    workspace: &str,
    pipeline: &str,
    id: &str,
) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/workspaces/{workspace}/pipelines/{pipeline}/executions/{id}"),
        None,
    )
    .await
}

/// The `state` of a `ResourceStatus` in a response body.
fn state(body: &Value) -> &str {
    body["status"]["state"].as_str().unwrap_or("")
}

/// One row of the delete table: a pipeline configuration, and how many updates
/// are written to it before it is deleted.
#[derive(Debug)]
struct DeleteCase {
    label: &'static str,
    computations: Value,
    description: Option<&'static str>,
    environment_variables: Option<Value>,
    tags: Option<Value>,
    /// How many `UpdatePipeline` calls land before the delete, so the row spans
    /// a pipeline at its first version and one with a version history.
    updates: usize,
}

/// The rows: `computations` spanning the DAG shapes `ComputeNodeList` admits —
/// none, one, independent nodes, a chain, a diamond (smithy_query query="shape"
/// shape="ComputeNode": `computeNodeName`, `taskName`, `environmentVariables`,
/// `dependsOn`) — crossed with a present / absent `description`, with
/// `environmentVariables` absent, set and empty, with / without tags, and at one
/// version or several.
fn delete_cases() -> Vec<DeleteCase> {
    vec![
        DeleteCase {
            label: "no-nodes-no-members",
            computations: json!([]),
            description: None,
            environment_variables: None,
            tags: None,
            updates: 0,
        },
        DeleteCase {
            label: "one-node-described",
            computations: json!([node("only", &[])]),
            description: Some("the only node"),
            environment_variables: None,
            tags: None,
            updates: 0,
        },
        DeleteCase {
            label: "independent-nodes-with-environment",
            computations: json!([node("left", &[]), node("right", &[])]),
            description: None,
            environment_variables: Some(map_of(&[("STAGE", "test"), ("REGION", REGION)])),
            tags: None,
            updates: 0,
        },
        DeleteCase {
            label: "chain-of-three-tagged",
            computations: json!([
                node("extract", &[]),
                node("transform", &["extract"]),
                node("load", &["transform"]),
            ]),
            description: Some("a three-stage chain"),
            environment_variables: Some(map_of(&[])),
            tags: Some(map_of(&[("owner", "plant-team")])),
            updates: 0,
        },
        DeleteCase {
            label: "diamond-after-one-update",
            computations: json!([
                node("fan-out", &[]),
                node("left", &["fan-out"]),
                node("right", &["fan-out"]),
                node("fan-in", &["left", "right"]),
            ]),
            description: None,
            environment_variables: None,
            tags: None,
            updates: 1,
        },
        DeleteCase {
            label: "one-node-after-three-updates",
            computations: json!([node("only", &[])]),
            description: Some("rewritten repeatedly"),
            environment_variables: Some(map_of(&[("KEEP", "me")])),
            tags: Some(map_of(&[("owner", "plant-team"), ("tier", "gold")])),
            updates: 3,
        },
    ]
}

/// The create body a row sends, with every absent member OMITTED rather than
/// sent as null.
fn create_body(case: &DeleteCase, name: &str) -> Value {
    let mut body = json!({ "pipelineName": name, "computations": case.computations });
    let object = body.as_object_mut().expect("an object body");
    if let Some(description) = case.description {
        object.insert("description".to_string(), json!(description));
    }
    if let Some(environment) = case.environment_variables.clone() {
        object.insert("environmentVariables".to_string(), environment);
    }
    if let Some(tags) = case.tags.clone() {
        object.insert("tags".to_string(), tags);
    }
    body
}

#[tokio::test]
async fn a_delete_removes_the_pipeline_it_names() {
    let router = router();
    create_workspace(&router, WORKSPACE).await;
    create_task(&router, WORKSPACE).await;

    // A pipeline no row touches: every delete below must leave it exactly where
    // it is.
    let (status, bystander) = create(
        &router,
        WORKSPACE,
        json!({ "pipelineName": "bystander", "computations": [node("only", &[])] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "create the bystander: got {status} {bystander}"
    );

    for case in delete_cases() {
        let label = case.label;
        let (status, created) = create(&router, WORKSPACE, create_body(&case, label)).await;
        assert_eq!(
            status, 200,
            "case {label:?}: create expected 200, got {status} {created}"
        );
        for at in 0..case.updates {
            let (status, updated) = update(
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

        // The pipeline is there to begin with: the delete below is about a
        // resource the store really holds.
        let (status, before) = describe(&router, WORKSPACE, label).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the pipeline must exist before the delete, got {status} {before}"
        );

        let (status, deleted) = delete(&router, WORKSPACE, label).await;
        assert_eq!(
            status, 200,
            "case {label:?}: delete expected 200, got {status} {deleted}"
        );
        assert_eq!(
            state(&deleted),
            "DELETING",
            "case {label:?}: the delete must report the removed pipeline's lifecycle state, \
             got {deleted}"
        );

        // Removed, so the reader resolves to nothing rather than to a stale copy.
        let (status, gone) = describe(&router, WORKSPACE, label).await;
        assert_eq!(
            status, 404,
            "case {label:?}: a describe after the delete expected 404, got {status} {gone}"
        );
        assert_eq!(
            error_code(&gone),
            "ResourceNotFoundException",
            "case {label:?}: unexpected error code after the delete, got {gone}"
        );

        // A second delete addresses a pipeline the store no longer holds.
        let (status, again) = delete(&router, WORKSPACE, label).await;
        assert_eq!(
            status, 404,
            "case {label:?}: a repeated delete expected 404, got {status} {again}"
        );
        assert_eq!(
            error_code(&again),
            "ResourceNotFoundException",
            "case {label:?}: unexpected error code on the repeated delete, got {again}"
        );

        // The name it held is free again, and the pipeline that takes it starts
        // from the first version rather than carrying the deleted one's history.
        let (status, recreated) = create(
            &router,
            WORKSPACE,
            json!({ "pipelineName": label, "computations": [] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: recreating the deleted name expected 200, got {status} {recreated}"
        );
        assert_eq!(
            recreated["version"].as_str().unwrap_or(""),
            "1",
            "case {label:?}: the recreated pipeline must start at the first version, \
             got {recreated}"
        );
        let (status, read_back) = describe(&router, WORKSPACE, label).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the recreated pipeline must read back, got {status} {read_back}"
        );
        assert_eq!(
            read_back["computations"].as_array().map(Vec::len),
            Some(0),
            "case {label:?}: the recreated pipeline must hold its OWN nodes, got {read_back}"
        );
        assert_eq!(
            read_back.get("description"),
            None,
            "case {label:?}: the recreated pipeline must not inherit the deleted one's \
             description, got {read_back}"
        );

        // Clean up so the next row starts from the same store, and so the
        // delete of a just-recreated pipeline is exercised too.
        let (status, cleaned) = delete(&router, WORKSPACE, label).await;
        assert_eq!(
            status, 200,
            "case {label:?}: deleting the recreated pipeline expected 200, got {status} {cleaned}"
        );

        // Every row leaves the bystander alone.
        let (status, survivor) = describe(&router, WORKSPACE, "bystander").await;
        assert_eq!(
            status, 200,
            "case {label:?}: the bystander must survive, got {status} {survivor}"
        );
        assert_eq!(
            survivor["computations"][0]["computeNodeName"]
                .as_str()
                .unwrap_or(""),
            "only",
            "case {label:?}: the bystander's DAG must be untouched, got {survivor}"
        );
    }
}

#[tokio::test]
async fn a_pipeline_with_an_unfinished_execution_is_not_deleted() {
    let router = router();
    create_workspace(&router, WORKSPACE).await;
    create_task(&router, WORKSPACE).await;

    // The rows differ in how many executions the pipeline carries and in what
    // the starts sent, so the refusal is pinned over a set of inputs rather
    // than over one.
    let cases: Vec<(&str, Vec<Value>)> = vec![
        ("one-plain-execution", vec![json!({})]),
        (
            "one-execution-with-a-client-token",
            vec![json!({ "clientToken": "4ddc4d31-2f9b-4b0b-9a1e-8b8d6c3f0a11" })],
        ),
        (
            "two-executions",
            vec![json!({}), json!({ "executionPriority": 1 })],
        ),
        (
            "three-executions",
            vec![
                json!({}),
                json!({ "executionPriority": 2 }),
                json!({
                    "executionEnvironmentVariableOverrides": { "global": { "STAGE": "test" } }
                }),
            ],
        ),
    ];

    for (label, starts) in cases {
        let (status, created) = create(
            &router,
            WORKSPACE,
            json!({
                "pipelineName": label,
                "computations": [node("extract", &[]), node("load", &["extract"])],
                "description": "a pipeline with executions",
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: create expected 200, got {status} {created}"
        );

        let mut ids = Vec::new();
        for body in &starts {
            ids.push(start(&router, WORKSPACE, label, body.clone()).await);
        }

        let (status, refused) = delete(&router, WORKSPACE, label).await;
        assert_eq!(
            status, 409,
            "case {label:?}: a pipeline with {} unfinished execution(s) expected 409, \
             got {status} {refused}",
            ids.len()
        );
        assert_eq!(
            error_code(&refused),
            "ConflictingOperationException",
            "case {label:?}: unexpected error code, got {refused}"
        );
        assert_eq!(
            refused["resourceId"].as_str().unwrap_or(""),
            label,
            "case {label:?}: the refusal must name the pipeline it refuses, got {refused}"
        );
        assert_eq!(
            refused["resourceArn"].as_str().unwrap_or(""),
            expected_arn(WORKSPACE, label),
            "case {label:?}: the refusal must carry the pipeline's ARN, got {refused}"
        );

        // The refused delete wrote nothing: the pipeline is still there, at the
        // version and with the configuration it had.
        let (status, survivor) = describe(&router, WORKSPACE, label).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the refused delete must leave the pipeline, got {status} {survivor}"
        );
        assert_eq!(
            survivor["version"].as_str().unwrap_or(""),
            "1",
            "case {label:?}: the refused delete must not bump the version, got {survivor}"
        );
        assert_eq!(
            survivor["computations"].as_array().map(Vec::len),
            Some(2),
            "case {label:?}: the refused delete must not touch the DAG, got {survivor}"
        );

        // And it left every execution readable.
        for id in &ids {
            let (status, execution) = describe_execution(&router, WORKSPACE, label, id).await;
            assert_eq!(
                status, 200,
                "case {label:?}: execution {id} must survive the refused delete, \
                 got {status} {execution}"
            );
        }
    }

    // A pipeline of the same workspace that nothing was started on is deletable
    // all the same: the refusal is about THIS pipeline's executions.
    let (status, created) = create(
        &router,
        WORKSPACE,
        json!({ "pipelineName": "never-started", "computations": [node("only", &[])] }),
    )
    .await;
    assert_eq!(status, 200, "create never-started: got {status} {created}");
    let (status, deleted) = delete(&router, WORKSPACE, "never-started").await;
    assert_eq!(
        status, 200,
        "a pipeline with no executions must delete, got {status} {deleted}"
    );
    assert_eq!(
        state(&deleted),
        "DELETING",
        "the delete must report the removed pipeline's lifecycle state, got {deleted}"
    );
}

#[tokio::test]
async fn a_delete_removes_only_the_workspace_and_name_it_addresses() {
    let router = router();
    // One pipeline name, held in two workspaces: a pipeline name is unique only
    // within its workspace, so the pair is what the delete addresses.
    let name = "shared-name";
    for workspace in ["plant-a", "plant-b"] {
        create_workspace(&router, workspace).await;
        create_task(&router, workspace).await;
        let (status, created) = create(
            &router,
            workspace,
            json!({
                "pipelineName": name,
                "computations": [node("only", &[])],
                "description": workspace,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "create {workspace}/{name}: got {status} {created}"
        );
    }

    // `plant-b`'s pipeline carries an execution, so this also pins that
    // deleting one pipeline leaves ANOTHER pipeline's executions alone.
    let kept = start(&router, "plant-b", name, json!({})).await;

    let (status, deleted) = delete(&router, "plant-a", name).await;
    assert_eq!(
        status, 200,
        "deleting plant-a/{name} expected 200, got {status} {deleted}"
    );

    let (status, survivor) = describe(&router, "plant-b", name).await;
    assert_eq!(
        status, 200,
        "case \"other workspace\": plant-b/{name} must survive, got {status} {survivor}"
    );
    assert_eq!(
        survivor["description"].as_str().unwrap_or(""),
        "plant-b",
        "case \"other workspace\": plant-b/{name} must keep its own configuration, got {survivor}"
    );
    let (status, execution) = describe_execution(&router, "plant-b", name, &kept).await;
    assert_eq!(
        status, 200,
        "case \"other workspace\": plant-b's execution must survive, got {status} {execution}"
    );

    // A pipeline no create ever made: an unknown name, and a known name in a
    // workspace that holds no pipeline at all.
    for (workspace, pipeline) in [("plant-b", "never-created"), ("plant-c", name)] {
        let (status, body) = delete(&router, workspace, pipeline).await;
        assert_eq!(
            status, 404,
            "case {workspace:?}/{pipeline:?}: delete expected 404, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {workspace:?}/{pipeline:?}: unexpected error code, got {body}"
        );
        assert_eq!(
            body["message"].as_str().unwrap_or(""),
            format!("Pipeline with ID {pipeline} not found"),
            "case {workspace:?}/{pipeline:?}: the refusal must name the identifier the \
             request sent, got {body}"
        );
    }
}
