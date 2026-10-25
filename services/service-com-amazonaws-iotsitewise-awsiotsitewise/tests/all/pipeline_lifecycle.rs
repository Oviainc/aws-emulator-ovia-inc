// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the IoT SiteWise workspace pipeline slice
//! (`CreatePipeline` -> `UpdatePipeline` -> `DescribePipeline`), driven over HTTP
//! through the same `MultiServiceRouter` the emulator boots with, so every
//! assertion is about the wire behaviour a real client sees.
//!
//! The rules these suites pin:
//!
//! * a created pipeline is persisted and reads back exactly as it was created, at
//!   its first version, under the ARN that carries both its workspace and its name;
//! * an accepted write APPENDS a version: the version counter rises by one per
//!   write, an update replaces only the members the request carries and preserves
//!   every member it omits, and a read naming an earlier version still answers
//!   from the configuration that version carried;
//! * a `computations` list is a pipeline DAG — uniquely named nodes, every
//!   dependency naming a node of the same pipeline, no cycles — and a list that is
//!   not one is refused by both the create and the update, leaving the stored
//!   pipeline untouched;
//! * a pipeline is addressed by `(workspaceName, pipelineName)`, so one name is
//!   free in every other workspace and taken in its own;
//! * a create and an update both owe a transition the clock makes due, so a read
//!   taken straight afterwards still observes `CREATING` / `UPDATING`.
//!
//! The HTTP harness (`call`, `router`, `error_code`) is the one the asset suite
//! already has; this suite reuses it rather than growing a second.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{
    ACTIVATION_DELAY, FIRST_PIPELINE_VERSION, MAX_PIPELINE_VERSIONS,
};

use crate::asset_lifecycle::{call, error_code, router, REGION};

/// The workspace every suite below works in unless it is about workspace scoping.
const WORKSPACE: &str = "plant-floor";

/// `POST|GET /workspaces/{workspaceName}/pipelines`, as `OPERATIONS.md` reports
/// the route.
///
/// `pub(crate)` because the sibling `create_pipeline_bounds` suite addresses the
/// same route: one definition of it, so the two cannot drift.
pub(crate) fn pipelines_uri(workspace: &str) -> String {
    format!("/workspaces/{workspace}/pipelines")
}

/// `GET|PUT /workspaces/{workspaceName}/pipelines/{pipelineName}`.
pub(crate) fn pipeline_uri(workspace: &str, name: &str) -> String {
    format!("/workspaces/{workspace}/pipelines/{name}")
}

/// The ARN form this emulator mints for a pipeline: the workspace as well as the
/// name, since the name is unique only within the workspace.
pub(crate) fn expected_arn(workspace: &str, name: &str) -> String {
    format!(
        "arn:aws:iotsitewise:{REGION}:{}:pipeline/{workspace}/{name}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// One compute node: its name, the task it runs and the nodes it waits for.
///
/// `dependsOn` is sent only when the node declares dependencies, so a round-trip
/// comparison is against exactly the members the request carried.
///
/// `pub(crate)` because the sibling `describe_pipeline_reads` suite writes the
/// same compute nodes: one definition of them, so the two cannot drift.
pub(crate) fn node(name: &str, task: &str, depends_on: &[&str]) -> Value {
    let mut value = json!({ "computeNodeName": name, "taskName": task });
    if !depends_on.is_empty() {
        value
            .as_object_mut()
            .expect("an object node")
            .insert("dependsOn".to_string(), json!(depends_on));
    }
    value
}

/// The `state` of a `ResourceStatus` in a response body.
fn state(body: &Value) -> &str {
    body["status"]["state"].as_str().unwrap_or("")
}

/// `CreatePipeline` with `body`.
pub(crate) async fn create(
    router: &MultiServiceRouter,
    workspace: &str,
    body: Value,
) -> (u16, Value) {
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

/// `DescribePipeline`, at the latest version or at `version`.
pub(crate) async fn describe(
    router: &MultiServiceRouter,
    workspace: &str,
    name: &str,
    version: Option<&str>,
) -> (u16, Value) {
    let uri = match version {
        Some(version) => format!("{}?version={version}", pipeline_uri(workspace, name)),
        None => pipeline_uri(workspace, name),
    };
    call(router, "GET", &uri, None).await
}

/// One row of the create-and-read-back table: the configuration a create sends.
#[derive(Debug)]
struct CreateCase {
    label: &'static str,
    computations: Value,
    description: Option<&'static str>,
    environment_variables: Option<Value>,
    tags: Option<Value>,
}

/// The rows: `computations` spanning the DAG shapes `ComputeNodeList` admits —
/// none, one, independent nodes, a chain, a diamond (smithy_query query="shape"
/// shape="ComputeNode": `computeNodeName`, `taskName`, `environmentVariables`,
/// `dependsOn`) — crossed with a present / absent `description`, with
/// `environmentVariables` absent, set and empty, and with / without tags.
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            label: "no-nodes-at-all",
            computations: json!([]),
            description: None,
            environment_variables: None,
            tags: None,
        },
        CreateCase {
            label: "one-node",
            computations: json!([node("extract", "extract-task", &[])]),
            description: Some("the smallest pipeline"),
            environment_variables: Some(json!({ "LOG_LEVEL": "debug" })),
            tags: None,
        },
        CreateCase {
            label: "three-independent-nodes",
            computations: json!([
                node("one", "task-one", &[]),
                node("two", "task-two", &[]),
                node("three", "task-three", &[]),
            ]),
            description: None,
            environment_variables: Some(json!({ "LOG_LEVEL": "info", "SHARD": "2" })),
            tags: Some(json!({ "team": "ops" })),
        },
        CreateCase {
            label: "a-chain",
            computations: json!([
                node("extract", "extract-task", &[]),
                node("transform", "transform-task", &["extract"]),
                node("load", "load-task", &["transform"]),
            ]),
            description: Some("extract, transform, load"),
            environment_variables: None,
            tags: Some(json!({ "team": "data", "env": "test" })),
        },
        CreateCase {
            label: "a-diamond",
            computations: json!([
                node("fan_out", "fan-out-task", &[]),
                node("left", "left-task", &["fan_out"]),
                node("right", "right-task", &["fan_out"]),
                node("fan_in", "fan-in-task", &["left", "right"]),
            ]),
            description: Some("two branches joined"),
            environment_variables: Some(json!({ "MODE": "batch" })),
            tags: None,
        },
        CreateCase {
            label: "empty-environment-variables",
            computations: json!([node("solo", "solo-task", &[])]),
            description: None,
            // Sent as an empty map, so the read must report an empty map and not
            // omit the member.
            environment_variables: Some(json!({})),
            tags: None,
        },
        CreateCase {
            label: "node-level-environment-variables",
            computations: json!([{
                "computeNodeName": "overrider",
                "taskName": "overriding-task",
                "environmentVariables": { "LOG_LEVEL": "trace" },
            }]),
            description: None,
            environment_variables: Some(json!({ "LOG_LEVEL": "warn" })),
            tags: None,
        },
    ]
}

/// The body one row sends.
fn create_body(case: &CreateCase, name: &str) -> Value {
    let mut body = json!({ "pipelineName": name, "computations": case.computations.clone() });
    let object = body.as_object_mut().expect("an object body");
    if let Some(description) = case.description {
        object.insert("description".to_string(), json!(description));
    }
    if let Some(variables) = case.environment_variables.clone() {
        object.insert("environmentVariables".to_string(), variables);
    }
    if let Some(tags) = case.tags.clone() {
        object.insert("tags".to_string(), tags);
    }
    body
}

#[tokio::test]
async fn created_pipeline_reads_back_through_describe_for_every_shape() {
    let router = router();
    let first_version = FIRST_PIPELINE_VERSION.to_string();

    for case in create_cases() {
        let name = format!("pipeline-{}", case.label);
        let (status, created) = create(&router, WORKSPACE, create_body(&case, &name)).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200, got {status} {created}",
            case.label
        );
        assert_eq!(
            created["pipelineName"].as_str().unwrap_or(""),
            name,
            "case {:?}: unexpected pipelineName in {created}",
            case.label
        );
        assert_eq!(
            created["pipelineArn"].as_str().unwrap_or(""),
            expected_arn(WORKSPACE, &name),
            "case {:?}: unexpected pipelineArn in {created}",
            case.label
        );
        assert_eq!(
            created["version"].as_str().unwrap_or(""),
            first_version,
            "case {:?}: a created pipeline is at its first version, got {created}",
            case.label
        );
        assert_eq!(
            state(&created),
            "CREATING",
            "case {:?}: a created pipeline reports CREATING, got {created}",
            case.label
        );

        let (status, described) = describe(&router, WORKSPACE, &name, None).await;
        assert_eq!(
            status, 200,
            "case {:?}: DescribePipeline expected 200, got {status} {described}",
            case.label
        );
        assert_eq!(
            described["pipelineName"].as_str().unwrap_or(""),
            name,
            "case {:?}: DescribePipeline reported another name in {described}",
            case.label
        );
        assert_eq!(
            described["workspaceName"].as_str().unwrap_or(""),
            WORKSPACE,
            "case {:?}: DescribePipeline reported another workspace in {described}",
            case.label
        );
        assert_eq!(
            described["pipelineArn"], created["pipelineArn"],
            "case {:?}: DescribePipeline reported another ARN in {described}",
            case.label
        );
        assert_eq!(
            described["version"].as_str().unwrap_or(""),
            first_version,
            "case {:?}: unexpected version in {described}",
            case.label
        );
        assert_eq!(
            described["computations"], case.computations,
            "case {:?}: DescribePipeline reported other computations in {described}",
            case.label
        );
        assert_eq!(
            described.get("description").cloned(),
            case.description.map(|text| json!(text)),
            "case {:?}: unexpected description in {described}",
            case.label
        );
        assert_eq!(
            described.get("environmentVariables").cloned(),
            case.environment_variables.clone(),
            "case {:?}: unexpected environmentVariables in {described}",
            case.label
        );

        let created_at = described["createdAt"].as_f64().unwrap_or(0.0);
        let updated_at = described["updatedAt"].as_f64().unwrap_or(0.0);
        assert!(
            created_at > 0.0,
            "case {:?}: createdAt must be a real instant, got {described}",
            case.label
        );
        assert_eq!(
            created_at, updated_at,
            "case {:?}: an untouched pipeline's updatedAt is its createdAt, got {described}",
            case.label
        );

        // Reading does not change what the next caller sees. The lifecycle state is
        // the one member time moves on its own — the owed CREATING -> ACTIVE
        // transition — so it is compared by value and the rest by equality.
        let (status, again) = describe(&router, WORKSPACE, &name, None).await;
        assert_eq!(
            status, 200,
            "case {:?}: second DescribePipeline expected 200, got {status} {again}",
            case.label
        );
        assert!(
            matches!(state(&again), "CREATING" | "ACTIVE"),
            "case {:?}: a created pipeline is CREATING or settled ACTIVE, got {again}",
            case.label
        );
        assert_eq!(
            without_status(&again),
            without_status(&described),
            "case {:?}: DescribePipeline answered differently on the second read: {again}",
            case.label
        );

        // The tags the create carried are persisted on the pipeline and read back
        // through `ListTagsForResource`, which resolves them by the pipeline's ARN.
        let (status, listed) = call(
            &router,
            "GET",
            &format!("/tags?resourceArn={}", expected_arn(WORKSPACE, &name)),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: ListTagsForResource expected 200, got {status} {listed}",
            case.label
        );
        assert_eq!(
            listed["tags"],
            case.tags.clone().unwrap_or_else(|| json!({})),
            "case {:?}: unexpected tags in {listed}",
            case.label
        );
    }
}

/// A `DescribePipeline` body without its lifecycle `status`, which time moves on
/// its own.
///
/// `pub(crate)` for the reason [`node`] is: the sibling `describe_pipeline_reads`
/// suite compares the same answers member for member.
pub(crate) fn without_status(body: &Value) -> Value {
    let mut copy = body.clone();
    if let Some(object) = copy.as_object_mut() {
        object.remove("status");
    }
    copy
}

/// The configuration every row of the update table starts from.
fn base_computations() -> Value {
    json!([
        node("extract", "extract-task", &[]),
        node("load", "load-task", &["extract"]),
    ])
}

/// The description every row of the update table starts from.
const BASE_DESCRIPTION: &str = "the configuration the create wrote";

/// The environment variables every row of the update table starts from.
fn base_environment_variables() -> Value {
    json!({ "LOG_LEVEL": "info" })
}

/// Create the pipeline the update table starts from.
async fn create_base(
    router: &MultiServiceRouter,
    name: &str,
) -> (u16, Value) {
    create(
        router,
        WORKSPACE,
        json!({
            "pipelineName": name,
            "computations": base_computations(),
            "description": BASE_DESCRIPTION,
            "environmentVariables": base_environment_variables(),
        }),
    )
    .await
}

#[tokio::test]
async fn an_update_writes_a_new_version_and_preserves_the_members_it_omits() {
    let router = router();
    let replacement_nodes = json!([node("only", "only-task", &[])]);

    // (case, the update body, the description / environmentVariables /
    // computations the pipeline must carry afterwards). One row per subset of the
    // three updatable members, plus the two rows where a member is REPLACED by an
    // empty value rather than omitted.
    let cases: Vec<(&str, Value, Value, Value, Value)> = vec![
        (
            "nothing at all",
            json!({}),
            json!(BASE_DESCRIPTION),
            base_environment_variables(),
            base_computations(),
        ),
        (
            "description only",
            json!({ "description": "a new description" }),
            json!("a new description"),
            base_environment_variables(),
            base_computations(),
        ),
        (
            "environmentVariables only",
            json!({ "environmentVariables": { "LOG_LEVEL": "trace", "SHARD": "7" } }),
            json!(BASE_DESCRIPTION),
            json!({ "LOG_LEVEL": "trace", "SHARD": "7" }),
            base_computations(),
        ),
        (
            "computations only",
            json!({ "computations": replacement_nodes.clone() }),
            json!(BASE_DESCRIPTION),
            base_environment_variables(),
            replacement_nodes.clone(),
        ),
        (
            "all three members",
            json!({
                "description": "everything replaced",
                "environmentVariables": { "MODE": "stream" },
                "computations": replacement_nodes.clone(),
            }),
            json!("everything replaced"),
            json!({ "MODE": "stream" }),
            replacement_nodes.clone(),
        ),
        (
            "an empty computations list",
            json!({ "computations": [] }),
            json!(BASE_DESCRIPTION),
            base_environment_variables(),
            json!([]),
        ),
        (
            "an empty environmentVariables map",
            json!({ "environmentVariables": {} }),
            json!(BASE_DESCRIPTION),
            json!({}),
            base_computations(),
        ),
    ];

    for (case, body, description, variables, computations) in cases {
        let name = format!("updated-{}", case.replace(' ', "-"));
        let (status, created) = create_base(&router, &name).await;
        assert_eq!(status, 200, "case {case:?}: the base create got {status} {created}");

        let (status, updated) = update(&router, WORKSPACE, &name, body).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} {updated}");
        assert_eq!(
            updated["version"].as_str().unwrap_or(""),
            (FIRST_PIPELINE_VERSION + 1).to_string(),
            "case {case:?}: an update writes the next version, got {updated}"
        );
        assert_eq!(
            state(&updated),
            "UPDATING",
            "case {case:?}: an updated pipeline reports UPDATING, got {updated}"
        );

        let (status, described) = describe(&router, WORKSPACE, &name, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribePipeline expected 200, got {status} {described}"
        );
        assert_eq!(
            described["version"], updated["version"],
            "case {case:?}: the read must answer from the version the update wrote, got {described}"
        );
        assert_eq!(
            described["description"], description,
            "case {case:?}: unexpected description in {described}"
        );
        assert_eq!(
            described["environmentVariables"], variables,
            "case {case:?}: unexpected environmentVariables in {described}"
        );
        assert_eq!(
            described["computations"], computations,
            "case {case:?}: unexpected computations in {described}"
        );
        // The update touched the configuration, so `updatedAt` moved past
        // `createdAt` while the create's own instant stayed put.
        assert!(
            described["updatedAt"].as_f64().unwrap_or(0.0)
                >= described["createdAt"].as_f64().unwrap_or(0.0),
            "case {case:?}: updatedAt must not precede createdAt, got {described}"
        );

        // The version the update superseded still reads back as it was written.
        let (status, original) = describe(
            &router,
            WORKSPACE,
            &name,
            Some(&FIRST_PIPELINE_VERSION.to_string()),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: reading version 1 expected 200, got {status} {original}"
        );
        assert_eq!(
            original["computations"],
            base_computations(),
            "case {case:?}: version 1 must still carry the created DAG, got {original}"
        );
        assert_eq!(
            original["description"].as_str().unwrap_or(""),
            BASE_DESCRIPTION,
            "case {case:?}: version 1 must still carry the created description, got {original}"
        );
        assert_eq!(
            original["environmentVariables"],
            base_environment_variables(),
            "case {case:?}: version 1 must still carry the created variables, got {original}"
        );
    }
}

#[tokio::test]
async fn every_version_a_write_produced_still_reads_back() {
    let router = router();
    let name = "versioned";
    let (status, created) = create_base(&router, name).await;
    assert_eq!(status, 200, "the base create got {status} {created}");

    // Each entry is the description the write at that version left behind; the
    // create wrote the first one.
    let mut per_version: Vec<(String, String)> =
        vec![(FIRST_PIPELINE_VERSION.to_string(), BASE_DESCRIPTION.to_string())];

    for step in 1..=3u64 {
        let description = format!("description written by update {step}");
        let (status, updated) = update(
            &router,
            WORKSPACE,
            name,
            json!({ "description": description.clone() }),
        )
        .await;
        assert_eq!(status, 200, "update {step}: got {status} {updated}");
        let version = updated["version"].as_str().unwrap_or("").to_string();
        assert_eq!(
            version,
            (FIRST_PIPELINE_VERSION + step).to_string(),
            "update {step}: the version counter must rise by one per write, got {updated}"
        );
        per_version.push((version, description));
    }

    for (version, description) in &per_version {
        let (status, described) = describe(&router, WORKSPACE, name, Some(version)).await;
        assert_eq!(
            status, 200,
            "version {version}: expected 200, got {status} {described}"
        );
        assert_eq!(
            described["version"].as_str().unwrap_or(""),
            version,
            "version {version}: the read must report the version it answered from, got {described}"
        );
        assert_eq!(
            described["description"].as_str().unwrap_or(""),
            description,
            "version {version}: unexpected description in {described}"
        );
    }

    // A read naming no version answers from the newest one.
    let (status, latest) = describe(&router, WORKSPACE, name, None).await;
    let (newest_version, newest_description) =
        per_version.last().expect("at least one version").clone();
    assert_eq!(status, 200, "the latest version: got {status} {latest}");
    assert_eq!(
        latest["version"].as_str().unwrap_or(""),
        newest_version,
        "a read naming no version answers from the latest one, got {latest}"
    );
    assert_eq!(
        latest["description"].as_str().unwrap_or(""),
        newest_description,
        "a read naming no version answers from the latest one, got {latest}"
    );

    // Versions no write ever produced: zero (the counter starts at one), the one
    // past the newest, and a far one.
    for version in ["0", "5", "9999999999"] {
        let (status, body) = describe(&router, WORKSPACE, name, Some(version)).await;
        assert_eq!(
            status, 404,
            "version {version:?}: expected 404, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "version {version:?}: unexpected error code in {body}"
        );
    }
}

#[tokio::test]
async fn only_a_directed_acyclic_graph_of_uniquely_named_nodes_is_accepted() {
    let router = router();

    // (case, computations, accepted)
    let cases: Vec<(&str, Value, bool)> = vec![
        ("no nodes", json!([]), true),
        ("one node", json!([node("solo", "solo-task", &[])]), true),
        (
            "a chain",
            json!([
                node("a", "task-a", &[]),
                node("b", "task-b", &["a"]),
                node("c", "task-c", &["b"]),
            ]),
            true,
        ),
        (
            "a diamond",
            json!([
                node("a", "task-a", &[]),
                node("b", "task-b", &["a"]),
                node("c", "task-c", &["a"]),
                node("d", "task-d", &["b", "c"]),
            ]),
            true,
        ),
        (
            "a node declared before the one it depends on",
            json!([
                node("b", "task-b", &["a"]),
                node("a", "task-a", &[]),
            ]),
            true,
        ),
        (
            "two nodes of one name",
            json!([
                node("twin", "task-a", &[]),
                node("twin", "task-b", &[]),
            ]),
            false,
        ),
        (
            "a dependency on a node of no pipeline",
            json!([node("a", "task-a", &["absent"])]),
            false,
        ),
        (
            "a node depending on itself",
            json!([node("a", "task-a", &["a"])]),
            false,
        ),
        (
            "a two-node cycle",
            json!([
                node("a", "task-a", &["b"]),
                node("b", "task-b", &["a"]),
            ]),
            false,
        ),
        (
            "a three-node cycle",
            json!([
                node("a", "task-a", &["c"]),
                node("b", "task-b", &["a"]),
                node("c", "task-c", &["b"]),
            ]),
            false,
        ),
        (
            "a cycle reachable from an acyclic node",
            json!([
                node("root", "task-root", &[]),
                node("a", "task-a", &["root", "b"]),
                node("b", "task-b", &["a"]),
            ]),
            false,
        ),
    ];

    for (case, computations, accepted) in cases {
        // The create and the update take the same list, so each row is asserted
        // against both.
        let created_name = format!("dag-create-{}", case.replace(' ', "-"));
        let (status, body) = create(
            &router,
            WORKSPACE,
            json!({ "pipelineName": created_name, "computations": computations.clone() }),
        )
        .await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: CreatePipeline expected 200, got {status} {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: CreatePipeline expected 400, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidRequestException",
                "case {case:?}: unexpected error code in {body}"
            );
            let (status, body) = describe(&router, WORKSPACE, &created_name, None).await;
            assert_eq!(
                status, 404,
                "case {case:?}: a refused create must store no pipeline, got {status} {body}"
            );
        }

        let updated_name = format!("dag-update-{}", case.replace(' ', "-"));
        let (status, created) = create_base(&router, &updated_name).await;
        assert_eq!(status, 200, "case {case:?}: the base create got {status} {created}");
        let (status, body) = update(
            &router,
            WORKSPACE,
            &updated_name,
            json!({ "computations": computations.clone() }),
        )
        .await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: UpdatePipeline expected 200, got {status} {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: UpdatePipeline expected 400, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidRequestException",
                "case {case:?}: unexpected error code in {body}"
            );
            // A refused update writes nothing: neither a version nor a
            // configuration changed.
            let (status, described) = describe(&router, WORKSPACE, &updated_name, None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the pipeline must survive a refused update, got {status} {described}"
            );
            assert_eq!(
                described["version"].as_str().unwrap_or(""),
                FIRST_PIPELINE_VERSION.to_string(),
                "case {case:?}: a refused update must not bump the version, got {described}"
            );
            assert_eq!(
                described["computations"],
                base_computations(),
                "case {case:?}: a refused update must not change the DAG, got {described}"
            );
        }
    }
}

#[tokio::test]
async fn a_pipeline_is_addressed_by_its_workspace_and_name() {
    let router = router();
    let name = "shared-name";

    // The same pipeline name in two workspaces is two pipelines, each with its own
    // ARN and its own configuration.
    for (workspace, task) in [("plant-a", "task-a"), ("plant-b", "task-b")] {
        let (status, created) = create(
            &router,
            workspace,
            json!({
                "pipelineName": name,
                "computations": [node("only", task, &[])],
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "workspace {workspace:?}: expected 200, got {status} {created}"
        );
        assert_eq!(
            created["pipelineArn"].as_str().unwrap_or(""),
            expected_arn(workspace, name),
            "workspace {workspace:?}: the ARN must carry the workspace, got {created}"
        );
    }
    for (workspace, task) in [("plant-a", "task-a"), ("plant-b", "task-b")] {
        let (status, described) = describe(&router, workspace, name, None).await;
        assert_eq!(
            status, 200,
            "workspace {workspace:?}: DescribePipeline got {status} {described}"
        );
        assert_eq!(
            described["computations"][0]["taskName"].as_str().unwrap_or(""),
            task,
            "workspace {workspace:?}: a pipeline of one workspace must not be read \
             from another, got {described}"
        );
    }

    // An update names the pipeline of ONE workspace.
    let (status, updated) = update(
        &router,
        "plant-a",
        name,
        json!({ "description": "only plant-a" }),
    )
    .await;
    assert_eq!(status, 200, "UpdatePipeline on plant-a: got {status} {updated}");
    let (status, other) = describe(&router, "plant-b", name, None).await;
    assert_eq!(status, 200, "plant-b after the update: got {status} {other}");
    assert_eq!(
        other.get("description"),
        None,
        "updating one workspace's pipeline must not touch another's, got {other}"
    );
    assert_eq!(
        other["version"].as_str().unwrap_or(""),
        FIRST_PIPELINE_VERSION.to_string(),
        "updating one workspace's pipeline must not bump another's version, got {other}"
    );

    // A pipeline no create ever made: an unknown name, and a known name in a
    // workspace that holds no pipeline at all.
    for (workspace, pipeline) in [("plant-a", "never-created"), ("plant-c", name)] {
        let (status, body) = describe(&router, workspace, pipeline, None).await;
        assert_eq!(
            status, 404,
            "{workspace}/{pipeline}: DescribePipeline expected 404, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "{workspace}/{pipeline}: unexpected error code in {body}"
        );

        let (status, body) = update(
            &router,
            workspace,
            pipeline,
            json!({ "description": "a pipeline that is not there" }),
        )
        .await;
        assert_eq!(
            status, 404,
            "{workspace}/{pipeline}: UpdatePipeline expected 404, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "{workspace}/{pipeline}: unexpected error code in {body}"
        );
    }

    // A name taken in its own workspace is a conflict naming the pipeline that
    // holds it.
    let (status, conflict) = create(
        &router,
        "plant-a",
        json!({ "pipelineName": name, "computations": [] }),
    )
    .await;
    assert_eq!(status, 409, "a duplicate name expected 409, got {status} {conflict}");
    assert_eq!(
        error_code(&conflict),
        "ConflictingOperationException",
        "unexpected error code in {conflict}"
    );
    assert_eq!(
        conflict["resourceArn"].as_str().unwrap_or(""),
        expected_arn("plant-a", name),
        "the conflict must name the pipeline holding the name, got {conflict}"
    );
    // The refused create left the pipeline that holds the name as it was.
    let (status, survivor) = describe(&router, "plant-a", name, None).await;
    assert_eq!(status, 200, "the held pipeline must survive, got {status} {survivor}");
    assert_eq!(
        survivor["computations"][0]["taskName"].as_str().unwrap_or(""),
        "task-a",
        "the refused create must not overwrite the DAG, got {survivor}"
    );
}

#[tokio::test]
async fn a_retried_create_with_the_same_client_token_replays_its_answer() {
    let router = router();
    let token = "11111111-2222-3333-4444-555555555555-retry";
    let body = json!({
        "pipelineName": "idempotent",
        "computations": [node("only", "only-task", &[])],
        "clientToken": token,
    });

    let (status, first) = create(&router, WORKSPACE, body.clone()).await;
    assert_eq!(status, 200, "the first create got {status} {first}");

    let (status, replayed) = create(&router, WORKSPACE, body).await;
    assert_eq!(status, 200, "the retry got {status} {replayed}");
    assert_eq!(
        replayed["version"], first["version"],
        "a retry must replay the original version, got {replayed}"
    );
    assert_eq!(
        replayed["pipelineArn"], first["pipelineArn"],
        "a retry must replay the original ARN, got {replayed}"
    );

    // The retry performed no second operation: the pipeline is still at its first
    // version.
    let (status, described) = describe(&router, WORKSPACE, "idempotent", None).await;
    assert_eq!(status, 200, "DescribePipeline got {status} {described}");
    assert_eq!(
        described["version"].as_str().unwrap_or(""),
        FIRST_PIPELINE_VERSION.to_string(),
        "a retry must not write another version, got {described}"
    );

    // A create carrying ANOTHER token is a create of a pipeline whose name is
    // taken, so it collides.
    let (status, conflict) = create(
        &router,
        WORKSPACE,
        json!({
            "pipelineName": "idempotent",
            "computations": [],
            "clientToken": "99999999-8888-7777-6666-555555555555-other",
        }),
    )
    .await;
    assert_eq!(status, 409, "another token expected 409, got {status} {conflict}");
    assert_eq!(
        error_code(&conflict),
        "ConflictingOperationException",
        "unexpected error code in {conflict}"
    );
}

#[tokio::test]
async fn the_retained_version_history_is_bounded() {
    let router = router();
    let name = "bounded-history";
    let (status, created) = create_base(&router, name).await;
    assert_eq!(status, 200, "the base create got {status} {created}");

    // Past the cap the OLDEST retained version is evicted, so a few writes beyond
    // it leave exactly the newest `MAX_PIPELINE_VERSIONS` readable.
    let beyond = 5u64;
    let writes = MAX_PIPELINE_VERSIONS as u64 + beyond - 1;
    for step in 1..=writes {
        let (status, updated) = update(
            &router,
            WORKSPACE,
            name,
            json!({ "description": format!("version {}", FIRST_PIPELINE_VERSION + step) }),
        )
        .await;
        assert_eq!(status, 200, "update {step}: got {status} {updated}");
    }

    let newest = FIRST_PIPELINE_VERSION + writes;
    let oldest_retained = newest - MAX_PIPELINE_VERSIONS as u64 + 1;
    for version in oldest_retained..=newest {
        let (status, described) =
            describe(&router, WORKSPACE, name, Some(&version.to_string())).await;
        assert_eq!(
            status, 200,
            "version {version}: a retained version must read back, got {status} {described}"
        );
        assert_eq!(
            described["version"].as_str().unwrap_or(""),
            version.to_string(),
            "version {version}: unexpected version in {described}"
        );
    }

    // Everything older than that was evicted, and reads as the same not-found
    // answer as a version no write ever produced.
    for version in FIRST_PIPELINE_VERSION..oldest_retained {
        let (status, body) = describe(&router, WORKSPACE, name, Some(&version.to_string())).await;
        assert_eq!(
            status, 404,
            "version {version}: an evicted version expected 404, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "version {version}: unexpected error code in {body}"
        );
    }
}

#[tokio::test]
async fn the_creating_and_updating_states_settle_to_active() {
    let router = router();
    let name = "settling";
    let (status, created) = create_base(&router, name).await;
    assert_eq!(status, 200, "the create got {status} {created}");

    // The transition the create owes is not instant: a read taken straight after
    // it still observes CREATING.
    let (status, during_create) = describe(&router, WORKSPACE, name, None).await;
    assert_eq!(status, 200, "the read during the create got {status} {during_create}");
    assert_eq!(
        state(&during_create),
        "CREATING",
        "a pipeline reports CREATING until the owed transition falls due, got {during_create}"
    );

    let settle = ACTIVATION_DELAY + Duration::from_millis(200);
    tokio::time::sleep(settle).await;
    let (status, settled) = describe(&router, WORKSPACE, name, None).await;
    assert_eq!(status, 200, "the read after the create settled got {status} {settled}");
    assert_eq!(
        state(&settled),
        "ACTIVE",
        "the owed CREATING -> ACTIVE transition must fall due, got {settled}"
    );

    let (status, updated) = update(
        &router,
        WORKSPACE,
        name,
        json!({ "description": "settling again" }),
    )
    .await;
    assert_eq!(status, 200, "the update got {status} {updated}");
    let (status, during_update) = describe(&router, WORKSPACE, name, None).await;
    assert_eq!(status, 200, "the read during the update got {status} {during_update}");
    assert_eq!(
        state(&during_update),
        "UPDATING",
        "a pipeline reports UPDATING until the owed transition falls due, got {during_update}"
    );

    tokio::time::sleep(settle).await;
    let (status, settled) = describe(&router, WORKSPACE, name, None).await;
    assert_eq!(status, 200, "the read after the update settled got {status} {settled}");
    assert_eq!(
        state(&settled),
        "ACTIVE",
        "the owed UPDATING -> ACTIVE transition must fall due, got {settled}"
    );
    assert_eq!(
        settled["description"].as_str().unwrap_or(""),
        "settling again",
        "settling must not undo the update, got {settled}"
    );
}
