// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the IoT SiteWise workspace -> task -> pipeline ->
//! pipeline-execution slice, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with, so every assertion is about the
//! wire behaviour a real client sees.
//!
//! The rules these suites pin:
//!
//! * `DescribePipelineExecution` answers from the stored execution the request's
//!   workspace, pipeline and execution id together name, and reports the record
//!   `StartPipelineExecution` resolved — never a fabricated one, and never the
//!   same answer for an id the store does not hold (that is the modelled 404);
//! * a pipeline's compute nodes are reported in the DAG order their `dependsOn`
//!   edges define: every node after every node it depends on, ties in declaration
//!   order, whatever order the create declared them in;
//! * a compute node's reported environment variables are the resolution of the
//!   whole hierarchy — task, then pipeline, then compute node, then the
//!   execution's global overrides, then its per-node overrides, each layer
//!   replacing an earlier layer's value for the same key and keeping the keys it
//!   does not name;
//! * a compute node's reported mounts are its task's mounts with the execution's
//!   overrides for that node merged BY NAME: a matching name replaces, a new name
//!   adds, an unreferenced task mount stays, and a node the overrides do not list
//!   keeps its task mounts as-is;
//! * the compute nodes paginate: `maxResults` bounds a page, `nextToken` resumes
//!   the same enumeration, and a token the operation never issued for that
//!   execution is a refusal rather than an empty page;
//! * `StartPipelineExecution` records a NEW execution of the pipeline's current
//!   version under an id of its own, at the scheduling priority the request named
//!   inside the member's modelled range (the documented default when it named
//!   none), and a priority outside that range is refused rather than recorded;
//! * a pipeline whose `computations` is not a DAG (a repeated node name, a
//!   dependency on a node it does not declare, a cycle) is refused at create
//!   time, and a create naming a workspace or task the store does not hold is the
//!   modelled 404.
//!
//! The HTTP harness (`call`, `router`, `error_code`, `is_modelled_id`,
//! `hex_token`) is the one the asset suite already has; this suite reuses it
//! rather than growing a second.

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::FIRST_PIPELINE_VERSION;

use super::asset_lifecycle::{call, error_code, hex_token, is_modelled_id, router, REGION};

/// The workspace every suite below works in.
const WORKSPACE: &str = "pipeline-workspace";

/// The pipeline every suite that needs only one works with.
const PIPELINE: &str = "daily-rollup";

/// A container image URI inside the modelled `EcrUri` pattern
/// (`^(\d{12}\.dkr\.ecr\.[a-z0-9-]+\.[a-z.]+)/...`).
const ECR_URI: &str = "123456789012.dkr.ecr.us-west-2.amazonaws.com/pipeline-task:v1";

/// A role ARN inside the modelled `IamRoleArn` pattern.
const TASK_ROLE: &str = "arn:aws:iam::123456789012:role/sitewise-task";

/// One row of the mount-merge table: the case name, the compute node it is
/// about, and the `(name, relativePath)` pairs that node's mounts must report,
/// in order.
type MountCase = (
    &'static str,
    &'static str,
    &'static [(&'static str, &'static str)],
);

/// One `ComputeNode` of a pipeline's `computations`, as the table rows below
/// declare it.
#[derive(Debug, Clone)]
struct Node {
    name: &'static str,
    task: &'static str,
    depends_on: Vec<&'static str>,
    /// The node-level environment variables, the third layer of the hierarchy.
    environment: Vec<(&'static str, &'static str)>,
}

impl Node {
    /// A node with no dependencies and no environment variables of its own.
    fn new(name: &'static str, task: &'static str) -> Self {
        Self {
            name,
            task,
            depends_on: Vec::new(),
            environment: Vec::new(),
        }
    }

    /// The same, waiting on `depends_on`.
    fn after(name: &'static str, task: &'static str, depends_on: &[&'static str]) -> Self {
        Self {
            name,
            task,
            depends_on: depends_on.to_vec(),
            environment: Vec::new(),
        }
    }

    /// The same, carrying node-level environment variables.
    fn with_environment(mut self, environment: &[(&'static str, &'static str)]) -> Self {
        self.environment = environment.to_vec();
        self
    }

    /// The JSON body this node sends, with every absent member OMITTED rather
    /// than sent as null.
    fn to_json(&self) -> Value {
        let mut body = json!({ "computeNodeName": self.name, "taskName": self.task });
        let object = body.as_object_mut().expect("an object body");
        if !self.depends_on.is_empty() {
            object.insert("dependsOn".to_string(), json!(self.depends_on));
        }
        if !self.environment.is_empty() {
            object.insert("environmentVariables".to_string(), map_of(&self.environment));
        }
        body
    }
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

/// A `Mount` naming an S3 access point, in the modelled shape.
fn mount(name: &str, relative_path: &str) -> Value {
    json!({
        "name": name,
        "relativePath": relative_path,
        "source": {
            "s3AccessPoint": {
                "accessPointArn":
                    format!("arn:aws:s3:{REGION}:123456789012:accesspoint/{name}"),
            }
        },
        "storageType": "SHARED_STORAGE",
    })
}

/// Create a workspace with the service's default encryption and assert it was
/// accepted.
async fn create_workspace(router: &MultiServiceRouter, name: &str) -> Value {
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
    body
}

/// Create a task carrying `environment` and `mounts`, and assert it was accepted.
async fn create_task(
    router: &MultiServiceRouter,
    workspace: &str,
    task: &str,
    environment: &[(&str, &str)],
    mounts: &[Value],
) -> Value {
    let mut container = json!({
        "ecrUri": ECR_URI,
        "taskExecutionRole": TASK_ROLE,
        "processingType": "GENERIC_COMPUTE_PROCESSING",
        "processingUnit": "UNITS_2",
    });
    let object = container.as_object_mut().expect("an object body");
    if !environment.is_empty() {
        object.insert("environmentVariables".to_string(), map_of(environment));
    }
    if !mounts.is_empty() {
        object.insert("mounts".to_string(), json!(mounts));
    }
    let (status, body) = call(
        router,
        "POST",
        &format!("/workspaces/{workspace}/tasks"),
        Some(json!({
            "taskName": task,
            "taskConfiguration": { "containerTaskConfiguration": container },
        })),
    )
    .await;
    assert_eq!(status, 200, "create task {task}: got {status} {body}");
    body
}

/// Create a pipeline out of `nodes`, returning the raw `(status, body)` so a
/// suite can assert a refusal too.
async fn create_pipeline(
    router: &MultiServiceRouter,
    workspace: &str,
    pipeline: &str,
    environment: &[(&str, &str)],
    nodes: &[Node],
) -> (u16, Value) {
    let computations: Vec<Value> = nodes.iter().map(Node::to_json).collect();
    let mut body = json!({ "pipelineName": pipeline, "computations": computations });
    let object = body.as_object_mut().expect("an object body");
    if !environment.is_empty() {
        object.insert("environmentVariables".to_string(), map_of(environment));
    }
    call(
        router,
        "POST",
        &format!("/workspaces/{workspace}/pipelines"),
        Some(body),
    )
    .await
}

/// Start an execution of `pipeline` with `body`, returning the raw
/// `(status, body)`.
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

/// Start an execution with no overrides at all and return its id.
async fn start_plain(router: &MultiServiceRouter, workspace: &str, pipeline: &str) -> String {
    let (status, body) = start_execution(router, workspace, pipeline, json!({})).await;
    assert_eq!(status, 202, "start execution: got {status} {body}");
    let id = body["pipelineExecutionId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        is_modelled_id(&id),
        "start execution: {id:?} is not a modelled ID"
    );
    id
}

/// Describe an execution, appending `query` (already in `?a=b` form) to the URI.
async fn describe_execution(
    router: &MultiServiceRouter,
    workspace: &str,
    pipeline: &str,
    id: &str,
    query: &str,
) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/workspaces/{workspace}/pipelines/{pipeline}/executions/{id}{query}"),
        None,
    )
    .await
}

/// The compute node names a `DescribePipelineExecution` body reports, in the
/// order it reports them.
fn reported_nodes(body: &Value) -> Vec<String> {
    body["computeNodeExecutionDetails"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(|detail| {
            detail["computeNodeName"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// One compute node's reported detail, by name.
fn reported_node<'a>(body: &'a Value, name: &str) -> &'a Value {
    body["computeNodeExecutionDetails"]
        .as_array()
        .expect("a compute node list")
        .iter()
        .find(|detail| detail["computeNodeName"].as_str() == Some(name))
        .unwrap_or_else(|| panic!("no compute node named {name} in {body}"))
}

/// Assert that `reported` holds each of `nodes` exactly once and places every
/// node after every node it depends on.
fn assert_dag_order(case: &str, nodes: &[Node], reported: &[String]) {
    assert_eq!(
        reported.len(),
        nodes.len(),
        "case {case:?}: expected {} compute nodes, got {reported:?}",
        nodes.len()
    );
    for node in nodes {
        let at = reported
            .iter()
            .position(|name| name == node.name)
            .unwrap_or_else(|| panic!("case {case:?}: {} missing from {reported:?}", node.name));
        for dependency in &node.depends_on {
            let dependency_at = reported
                .iter()
                .position(|name| name == dependency)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: {dependency} missing from {reported:?}")
                });
            assert!(
                dependency_at < at,
                "case {case:?}: {} must be reported after its dependency {dependency}, \
                 got {reported:?}",
                node.name
            );
        }
    }
}

#[tokio::test]
async fn pipeline_execution_reads_back_through_describe() {
    let router = router();
    create_workspace(&router, WORKSPACE).await;
    let task = create_task(
        &router,
        WORKSPACE,
        "extract",
        &[("TASK_ONLY", "task")],
        &[mount("inputs", "inputs")],
    )
    .await;
    let task_arn = task["taskArn"].as_str().unwrap_or_default().to_string();
    assert_eq!(
        task_arn,
        format!("arn:aws:iotsitewise:{REGION}:{}:task/{WORKSPACE}/extract", account_id()),
        "the created task's ARN names the workspace and the task"
    );

    let nodes = [
        Node::new("extract-node", "extract"),
        Node::after("load-node", "extract", &["extract-node"]),
    ];

    // Before anything is started, the execution the id names is absent — the
    // modelled 404, never a 501 and never an empty success.
    let (status, created) = create_pipeline(&router, WORKSPACE, PIPELINE, &[], &nodes).await;
    assert_eq!(status, 200, "create pipeline: got {status} {created}");
    assert_eq!(
        created["version"].as_str(),
        Some("1"),
        "a freshly created pipeline is at version 1, got {created}"
    );
    let absent = "11111111-2222-3333-4444-555555555555";
    let (status, body) = describe_execution(&router, WORKSPACE, PIPELINE, absent, "").await;
    assert_eq!(
        status, 404,
        "an execution nothing started: got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "an execution nothing started: got {body}"
    );

    let id = start_plain(&router, WORKSPACE, PIPELINE).await;
    let (status, body) = describe_execution(&router, WORKSPACE, PIPELINE, &id, "").await;
    assert_eq!(status, 200, "describe execution: got {status} {body}");

    assert_eq!(
        body["pipelineExecutionId"].as_str(),
        Some(id.as_str()),
        "describe reports the id it was asked about, got {body}"
    );
    assert_eq!(
        body["pipelineName"].as_str(),
        Some(PIPELINE),
        "describe reports the pipeline, got {body}"
    );
    assert_eq!(
        body["workspaceName"].as_str(),
        Some(WORKSPACE),
        "describe reports the workspace, got {body}"
    );
    assert_eq!(
        body["pipelineVersion"].as_str(),
        Some("1"),
        "describe reports the pipeline version the execution ran against, got {body}"
    );
    assert_eq!(
        body["executionPriority"].as_i64(),
        Some(2),
        "an execution that named no priority runs at the documented default of 2, got {body}"
    );
    assert_eq!(
        body["status"]["state"].as_str(),
        Some("NOT_STARTED"),
        "nothing ran, so the execution is reported NOT_STARTED, got {body}"
    );
    assert!(
        body.get("startTime").is_none() && body.get("endTime").is_none(),
        "nothing ran, so neither instant is reported, got {body}"
    );

    let order = reported_nodes(&body);
    assert_dag_order("round trip", &nodes, &order);
    let detail = reported_node(&body, "extract-node");
    assert_eq!(
        detail["taskArn"].as_str(),
        Some(task_arn.as_str()),
        "a compute node names the task the pipeline resolved, got {detail}"
    );
    assert_eq!(
        detail["taskVersion"].as_str(),
        Some("1"),
        "a compute node names the task version it resolved, got {detail}"
    );
    assert_eq!(
        detail["status"]["state"].as_str(),
        Some("NOT_STARTED"),
        "nothing ran, so the node is reported NOT_STARTED, got {detail}"
    );
    assert_eq!(
        detail["executionEnvironmentVariables"]["TASK_ONLY"].as_str(),
        Some("task"),
        "a node with no other layer runs with the task's own variables, got {detail}"
    );
    let dependent = reported_node(&body, "load-node");
    assert_eq!(
        dependent["dependsOn"].as_array().map(Vec::len),
        Some(1),
        "a node reports the dependencies the pipeline declared, got {dependent}"
    );

    // A read changes nothing a later caller sees.
    let (status, again) = describe_execution(&router, WORKSPACE, PIPELINE, &id, "").await;
    assert_eq!(status, 200, "second describe: got {status} {again}");
    assert_eq!(
        again, body,
        "a second describe of the same execution answers the same, got {again}"
    );
}

/// The account id every ARN this emulator mints carries.
fn account_id() -> String {
    engine::EMULATOR_ACCOUNT_ID.to_string()
}

#[tokio::test]
async fn compute_nodes_are_reported_in_dag_order() {
    // The rows differ in the SHAPE of the graph and in the order the create
    // declares it: a chain declared backwards, a diamond declared out of order,
    // independent nodes, a single node and no nodes at all.
    let cases: [(&str, Vec<Node>); 6] = [
        ("no nodes", vec![]),
        ("one node", vec![Node::new("only", "alpha")]),
        (
            "chain declared backwards",
            vec![
                Node::after("third", "alpha", &["second"]),
                Node::after("second", "alpha", &["first"]),
                Node::new("first", "alpha"),
            ],
        ),
        (
            "diamond declared out of order",
            vec![
                Node::after("sink", "beta", &["left", "right"]),
                Node::after("left", "alpha", &["source"]),
                Node::new("source", "alpha"),
                Node::after("right", "beta", &["source"]),
            ],
        ),
        (
            "independent nodes",
            vec![
                Node::new("one", "alpha"),
                Node::new("two", "beta"),
                Node::new("three", "alpha"),
            ],
        ),
        (
            "fan-in from two tasks",
            vec![
                Node::after("merge", "beta", &["ingest", "clean"]),
                Node::new("ingest", "alpha"),
                Node::new("clean", "beta"),
            ],
        ),
    ];

    for (case, nodes) in cases {
        let router = router();
        create_workspace(&router, WORKSPACE).await;
        create_task(&router, WORKSPACE, "alpha", &[], &[]).await;
        create_task(&router, WORKSPACE, "beta", &[], &[]).await;
        let (status, body) = create_pipeline(&router, WORKSPACE, PIPELINE, &[], &nodes).await;
        assert_eq!(
            status, 200,
            "case {case:?}: create pipeline got {status} {body}"
        );
        let id = start_plain(&router, WORKSPACE, PIPELINE).await;
        let (status, body) = describe_execution(&router, WORKSPACE, PIPELINE, &id, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: describe execution got {status} {body}"
        );
        assert_dag_order(case, &nodes, &reported_nodes(&body));
    }
}

#[tokio::test]
async fn environment_variables_resolve_through_the_hierarchy() {
    // Every layer defines `SHARED`, so each row's expected winner is the highest
    // layer it supplies; each layer also defines a key only it names, so a row
    // also pins that no layer's keys are dropped.
    const TASK: [(&str, &str); 2] = [("SHARED", "task"), ("TASK_ONLY", "task")];
    const PIPELINE_ENV: [(&str, &str); 2] = [("SHARED", "pipeline"), ("PIPELINE_ONLY", "pipeline")];
    const NODE: [(&str, &str); 2] = [("SHARED", "node"), ("NODE_ONLY", "node")];
    const GLOBAL: [(&str, &str); 2] = [("SHARED", "global"), ("GLOBAL_ONLY", "global")];
    const PER_NODE: [(&str, &str); 2] = [("SHARED", "per_node"), ("PER_NODE_ONLY", "per_node")];

    /// Which layers a row supplies: task, pipeline, node, execution global,
    /// execution per-node.
    #[derive(Debug, Clone, Copy)]
    struct Layers {
        task: bool,
        pipeline: bool,
        node: bool,
        global: bool,
        per_node: bool,
    }

    let cases: [(&str, Layers, &str, &[&str]); 6] = [
        (
            "task only",
            Layers { task: true, pipeline: false, node: false, global: false, per_node: false },
            "task",
            &["TASK_ONLY"],
        ),
        (
            "pipeline over task",
            Layers { task: true, pipeline: true, node: false, global: false, per_node: false },
            "pipeline",
            &["TASK_ONLY", "PIPELINE_ONLY"],
        ),
        (
            "node over pipeline",
            Layers { task: true, pipeline: true, node: true, global: false, per_node: false },
            "node",
            &["TASK_ONLY", "PIPELINE_ONLY", "NODE_ONLY"],
        ),
        (
            "execution global over node",
            Layers { task: true, pipeline: true, node: true, global: true, per_node: false },
            "global",
            &["TASK_ONLY", "PIPELINE_ONLY", "NODE_ONLY", "GLOBAL_ONLY"],
        ),
        (
            "execution per node over global",
            Layers { task: true, pipeline: true, node: true, global: true, per_node: true },
            "per_node",
            &[
                "TASK_ONLY",
                "PIPELINE_ONLY",
                "NODE_ONLY",
                "GLOBAL_ONLY",
                "PER_NODE_ONLY",
            ],
        ),
        (
            "per node without the layers under it",
            Layers { task: false, pipeline: false, node: false, global: false, per_node: true },
            "per_node",
            &["PER_NODE_ONLY"],
        ),
    ];

    for (case, layers, expected_shared, expected_keys) in cases {
        let router = router();
        create_workspace(&router, WORKSPACE).await;
        create_task(
            &router,
            WORKSPACE,
            "alpha",
            if layers.task { &TASK } else { &[] },
            &[],
        )
        .await;
        let mut node = Node::new("target", "alpha");
        if layers.node {
            node = node.with_environment(&NODE);
        }
        // A second node, which the per-node override never names, is what pins
        // that a per-node override applies only to the node it names.
        let nodes = [node, Node::new("other", "alpha")];
        let (status, body) = create_pipeline(
            &router,
            WORKSPACE,
            PIPELINE,
            if layers.pipeline { &PIPELINE_ENV } else { &[] },
            &nodes,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: create pipeline got {status} {body}"
        );

        let mut overrides = Map::new();
        if layers.global {
            overrides.insert("global".to_string(), map_of(&GLOBAL));
        }
        if layers.per_node {
            overrides.insert(
                "computeNodes".to_string(),
                json!({ "target": map_of(&PER_NODE) }),
            );
        }
        let start_body = if overrides.is_empty() {
            json!({})
        } else {
            json!({ "executionEnvironmentVariableOverrides": Value::Object(overrides) })
        };
        let (status, body) = start_execution(&router, WORKSPACE, PIPELINE, start_body).await;
        assert_eq!(
            status, 202,
            "case {case:?}: start execution got {status} {body}"
        );
        let id = body["pipelineExecutionId"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        let (status, body) = describe_execution(&router, WORKSPACE, PIPELINE, &id, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: describe execution got {status} {body}"
        );
        let resolved = &reported_node(&body, "target")["executionEnvironmentVariables"];
        if expected_keys.is_empty() {
            assert!(
                resolved["SHARED"].is_null(),
                "case {case:?}: expected no SHARED, got {resolved}"
            );
        } else {
            assert_eq!(
                resolved["SHARED"].as_str(),
                Some(expected_shared),
                "case {case:?}: expected SHARED={expected_shared}, got {resolved}"
            );
        }
        let reported_keys = resolved.as_object().expect("a variable map");
        assert_eq!(
            reported_keys.len(),
            expected_keys.len() + 1,
            "case {case:?}: expected the keys {expected_keys:?} and SHARED, got {resolved}"
        );
        for key in expected_keys {
            assert!(
                reported_keys.contains_key(*key),
                "case {case:?}: expected the key {key}, got {resolved}"
            );
        }

        // The node the per-node override does not name resolves without it.
        let other = &reported_node(&body, "other")["executionEnvironmentVariables"];
        assert!(
            other["PER_NODE_ONLY"].is_null(),
            "case {case:?}: a per-node override must not reach another node, got {other}"
        );
    }
}

#[tokio::test]
async fn mount_overrides_merge_by_name() {
    let router = router();
    create_workspace(&router, WORKSPACE).await;
    create_task(
        &router,
        WORKSPACE,
        "alpha",
        &[],
        &[mount("inputs", "inputs"), mount("outputs", "outputs")],
    )
    .await;
    let nodes = [Node::new("target", "alpha"), Node::new("other", "alpha")];
    let (status, body) = create_pipeline(&router, WORKSPACE, PIPELINE, &[], &nodes).await;
    assert_eq!(status, 200, "create pipeline: got {status} {body}");

    // `inputs` is replaced by name, `extra` is added, and `outputs` — which the
    // overrides never name — stays as the task defined it.
    let (status, body) = start_execution(
        &router,
        WORKSPACE,
        PIPELINE,
        json!({
            "executionMountOverrides": {
                "computeNodes": {
                    "target": [mount("inputs", "replaced/inputs"), mount("extra", "extra")],
                }
            }
        }),
    )
    .await;
    assert_eq!(status, 202, "start execution: got {status} {body}");
    let id = body["pipelineExecutionId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let (status, body) = describe_execution(&router, WORKSPACE, PIPELINE, &id, "").await;
    assert_eq!(status, 200, "describe execution: got {status} {body}");

    let cases: [MountCase; 2] = [
        (
            "the overridden node",
            "target",
            &[
                ("inputs", "replaced/inputs"),
                ("outputs", "outputs"),
                ("extra", "extra"),
            ],
        ),
        (
            "a node the overrides do not list",
            "other",
            &[("inputs", "inputs"), ("outputs", "outputs")],
        ),
    ];
    for (case, node, expected) in cases {
        let mounts = reported_node(&body, node)["executionMounts"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no mounts reported for {node}"))
            .clone();
        let reported: Vec<(String, String)> = mounts
            .iter()
            .map(|entry| {
                (
                    entry["name"].as_str().unwrap_or_default().to_string(),
                    entry["relativePath"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                )
            })
            .collect();
        let expected: Vec<(String, String)> = expected
            .iter()
            .map(|(name, path)| ((*name).to_string(), (*path).to_string()))
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the mounts {expected:?}, got {reported:?}"
        );
    }

    // The request's own override map is reported back as the execution's input.
    assert_eq!(
        body["requestMountOverrides"]["computeNodes"]["target"]
            .as_array()
            .map(Vec::len),
        Some(2),
        "the execution reports the mount overrides its start request carried, got {body}"
    );
}

#[tokio::test]
async fn compute_nodes_paginate_by_max_results() {
    let router = router();
    create_workspace(&router, WORKSPACE).await;
    create_task(&router, WORKSPACE, "alpha", &[], &[]).await;
    // A chain, so the DAG order is fully determined and a page boundary is too.
    let nodes = [
        Node::new("n1", "alpha"),
        Node::after("n2", "alpha", &["n1"]),
        Node::after("n3", "alpha", &["n2"]),
        Node::after("n4", "alpha", &["n3"]),
        Node::after("n5", "alpha", &["n4"]),
    ];
    let (status, body) = create_pipeline(&router, WORKSPACE, PIPELINE, &[], &nodes).await;
    assert_eq!(status, 200, "create pipeline: got {status} {body}");
    let id = start_plain(&router, WORKSPACE, PIPELINE).await;

    let (status, whole) = describe_execution(&router, WORKSPACE, PIPELINE, &id, "").await;
    assert_eq!(status, 200, "describe execution: got {status} {whole}");
    let every_node = reported_nodes(&whole);
    assert_eq!(
        every_node.len(),
        nodes.len(),
        "a request carrying no maxResults takes the modelled default of 50, got {every_node:?}"
    );
    assert!(
        whole.get("nextToken").is_none(),
        "a single page carries no nextToken, got {whole}"
    );

    // Each row walks the whole enumeration at a different page size, so the page
    // boundary falls in a different place: a size that divides the node count, one
    // that does not, one page per node, and one page for all of them.
    for page_size in [1_usize, 2, 3, 5, 250] {
        let mut walked = Vec::new();
        let mut query = format!("?maxResults={page_size}");
        loop {
            let (status, page) = describe_execution(&router, WORKSPACE, PIPELINE, &id, &query).await;
            assert_eq!(
                status, 200,
                "case {page_size}: describe page got {status} {page}"
            );
            let names = reported_nodes(&page);
            assert!(
                names.len() <= page_size,
                "case {page_size}: a page holds at most {page_size} nodes, got {names:?}"
            );
            assert_eq!(
                page["pipelineExecutionId"].as_str(),
                Some(id.as_str()),
                "case {page_size}: every page names the same execution, got {page}"
            );
            walked.extend(names);
            match page["nextToken"].as_str() {
                Some(token) => {
                    let resume = every_node
                        .get(walked.len())
                        .unwrap_or_else(|| panic!("case {page_size}: a token past the last node"));
                    assert_eq!(
                        token,
                        hex_token(resume),
                        "case {page_size}: the token names the node the next page resumes from"
                    );
                    query = format!("?maxResults={page_size}&nextToken={token}");
                }
                None => break,
            }
        }
        assert_eq!(
            walked, every_node,
            "case {page_size}: paging through the execution walks the same DAG order"
        );
    }

    // A token this operation never issued for this execution is a refusal, not an
    // empty page.
    let cases: [(&str, String); 2] = [
        ("not a token at all", "not-a-token".to_string()),
        ("a node of no pipeline", hex_token("absent-node")),
    ];
    for (case, token) in cases {
        let (status, body) =
            describe_execution(&router, WORKSPACE, PIPELINE, &id, &format!("?nextToken={token}"))
                .await;
        assert_eq!(status, 400, "case {case:?}: got {status} {body}");
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {case:?}: got {body}"
        );
    }
}

#[tokio::test]
async fn an_execution_of_another_pipeline_is_not_this_one() {
    let router = router();
    create_workspace(&router, WORKSPACE).await;
    create_workspace(&router, "other-workspace").await;
    create_task(&router, WORKSPACE, "alpha", &[], &[]).await;
    create_task(&router, "other-workspace", "alpha", &[], &[]).await;
    let nodes = [Node::new("only", "alpha")];
    for (workspace, pipeline) in [
        (WORKSPACE, PIPELINE),
        (WORKSPACE, "second-pipeline"),
        ("other-workspace", PIPELINE),
    ] {
        let (status, body) = create_pipeline(&router, workspace, pipeline, &[], &nodes).await;
        assert_eq!(
            status, 200,
            "create pipeline {workspace}/{pipeline}: got {status} {body}"
        );
    }
    let id = start_plain(&router, WORKSPACE, PIPELINE).await;

    // The id is the execution's, but each row addresses it through a pipeline it
    // does not belong to, which is an absence.
    let cases: [(&str, &str, &str); 3] = [
        ("another pipeline of the same workspace", WORKSPACE, "second-pipeline"),
        ("the same pipeline name in another workspace", "other-workspace", PIPELINE),
        ("a pipeline nothing created", WORKSPACE, "no-such-pipeline"),
    ];
    for (case, workspace, pipeline) in cases {
        let (status, body) = describe_execution(&router, workspace, pipeline, &id, "").await;
        assert_eq!(status, 404, "case {case:?}: got {status} {body}");
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: got {body}"
        );
    }

    // Through its own pipeline the same id resolves.
    let (status, body) = describe_execution(&router, WORKSPACE, PIPELINE, &id, "").await;
    assert_eq!(
        status, 200,
        "the execution's own pipeline resolves it: got {status} {body}"
    );
}

#[tokio::test]
async fn a_pipeline_that_is_not_a_dag_is_refused() {
    let router = router();
    create_workspace(&router, WORKSPACE).await;
    create_task(&router, WORKSPACE, "alpha", &[], &[]).await;

    let cases: [(&str, Vec<Node>); 5] = [
        (
            "a repeated compute node name",
            vec![Node::new("twice", "alpha"), Node::new("twice", "alpha")],
        ),
        (
            "a dependency on a node the pipeline does not declare",
            vec![Node::after("one", "alpha", &["absent"])],
        ),
        (
            "a node depending on itself",
            vec![Node::after("loop", "alpha", &["loop"])],
        ),
        (
            "a two-node cycle",
            vec![
                Node::after("left", "alpha", &["right"]),
                Node::after("right", "alpha", &["left"]),
            ],
        ),
        (
            "a cycle behind a node that can run",
            vec![
                Node::new("root", "alpha"),
                Node::after("left", "alpha", &["root", "right"]),
                Node::after("right", "alpha", &["left"]),
            ],
        ),
    ];
    for (case, nodes) in cases {
        let (status, body) = create_pipeline(&router, WORKSPACE, PIPELINE, &[], &nodes).await;
        assert_eq!(status, 400, "case {case:?}: got {status} {body}");
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {case:?}: got {body}"
        );
    }

    // A compute node "references a task", so an execution of a pipeline whose node
    // names a task the workspace does not hold resolves to nothing. The pipeline
    // itself stores the reference (`CreatePipeline` holds no task registry), so the
    // absence shows at the call that has to resolve it.
    let (status, body) = create_pipeline(
        &router,
        WORKSPACE,
        "unresolvable",
        &[],
        &[Node::new("one", "no-such-task")],
    )
    .await;
    assert_eq!(status, 200, "create pipeline: got {status} {body}");
    let (status, body) = start_execution(&router, WORKSPACE, "unresolvable", json!({})).await;
    assert_eq!(status, 404, "an unknown task: got {status} {body}");
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "an unknown task: got {body}"
    );

    // And an execution of a pipeline nothing created is an absence too, whether or
    // not the workspace it names exists.
    for (case, workspace) in [
        ("a workspace the store holds", WORKSPACE),
        ("a workspace nothing created", "no-such-workspace"),
    ] {
        let (status, body) =
            start_execution(&router, workspace, "no-such-pipeline", json!({})).await;
        assert_eq!(status, 404, "case {case:?}: got {status} {body}");
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: got {body}"
        );
    }
}

/// A `ClientToken`, which the model bounds at `len 36..64, pattern ^\S{36,64}$`.
fn client_token(suffix: &str) -> String {
    format!("retry-token-{suffix:0>24}")
}

#[tokio::test]
async fn a_retry_with_the_same_client_token_replays_its_answer() {
    // Each row retries one `@idempotencyToken` call with the token the first,
    // successful call carried: the retry must answer as that call did and write
    // nothing, while the same call under a DIFFERENT token collides or starts
    // afresh.
    let router = router();
    let workspace_body = json!({
        "workspaceName": WORKSPACE,
        "encryptionConfiguration": { "encryptionType": "SITEWISE_DEFAULT_ENCRYPTION" },
        "clientToken": client_token("ws"),
    });
    let (status, first) = call(&router, "POST", "/workspaces", Some(workspace_body.clone())).await;
    assert_eq!(status, 200, "create workspace: got {status} {first}");
    let (status, replay) = call(&router, "POST", "/workspaces", Some(workspace_body)).await;
    assert_eq!(status, 200, "retried create workspace: got {status} {replay}");
    assert_eq!(
        replay["workspaceArn"], first["workspaceArn"],
        "case \"workspace retry\": expected the first answer's ARN, got {replay}"
    );
    let (status, body) = call(
        &router,
        "POST",
        "/workspaces",
        Some(json!({
            "workspaceName": WORKSPACE,
            "encryptionConfiguration": { "encryptionType": "SITEWISE_DEFAULT_ENCRYPTION" },
            "clientToken": client_token("other"),
        })),
    )
    .await;
    assert_eq!(
        status, 409,
        "case \"workspace, another token\": got {status} {body}"
    );

    let task_body = json!({
        "taskName": "alpha",
        "taskConfiguration": { "containerTaskConfiguration": {
            "ecrUri": ECR_URI,
            "taskExecutionRole": TASK_ROLE,
            "processingType": "GENERIC_COMPUTE_PROCESSING",
            "processingUnit": "UNITS_2",
        }},
        "clientToken": client_token("task"),
    });
    let uri = format!("/workspaces/{WORKSPACE}/tasks");
    let (status, first) = call(&router, "POST", &uri, Some(task_body.clone())).await;
    assert_eq!(status, 200, "create task: got {status} {first}");
    let (status, replay) = call(&router, "POST", &uri, Some(task_body)).await;
    assert_eq!(status, 200, "retried create task: got {status} {replay}");
    assert_eq!(
        replay["taskArn"], first["taskArn"],
        "case \"task retry\": expected the first answer's ARN, got {replay}"
    );
    assert_eq!(
        replay["version"], first["version"],
        "case \"task retry\": expected the first answer's version, got {replay}"
    );

    let (status, body) = create_pipeline(
        &router,
        WORKSPACE,
        PIPELINE,
        &[],
        &[Node::new("only", "alpha")],
    )
    .await;
    assert_eq!(status, 200, "create pipeline: got {status} {body}");
    let start_body = json!({ "clientToken": client_token("exec") });
    let (status, first) = start_execution(&router, WORKSPACE, PIPELINE, start_body.clone()).await;
    assert_eq!(status, 202, "start execution: got {status} {first}");
    let (status, replay) = start_execution(&router, WORKSPACE, PIPELINE, start_body).await;
    assert_eq!(status, 202, "retried start execution: got {status} {replay}");
    assert_eq!(
        replay["pipelineExecutionId"], first["pipelineExecutionId"],
        "case \"execution retry\": expected the first answer's id, got {replay}"
    );
    let (status, fresh) = start_execution(
        &router,
        WORKSPACE,
        PIPELINE,
        json!({ "clientToken": client_token("second") }),
    )
    .await;
    assert_eq!(status, 202, "a second start: got {status} {fresh}");
    assert_ne!(
        fresh["pipelineExecutionId"], first["pipelineExecutionId"],
        "case \"execution, another token\": expected a new execution, got {fresh}"
    );
}

#[tokio::test]
async fn a_workspace_scoped_time_series_lookup_resolves_to_nothing() {
    // The time-series store holds no series inside a workspace, so a scoped
    // lookup or delete is an absence whether or not the workspace exists — the
    // rows are a workspace this store holds and one it does not.
    let router = router();
    create_workspace(&router, WORKSPACE).await;
    let cases: [(&str, &str); 2] = [
        ("a workspace the store holds", WORKSPACE),
        ("a workspace nothing created", "no-such-workspace"),
    ];
    for (case, workspace) in cases {
        let query = format!("alias=/plant/1/line/2&workspaceName={workspace}");
        let (status, body) = call(
            &router,
            "GET",
            &format!("/timeseries/describe?{query}"),
            None,
        )
        .await;
        assert_eq!(status, 404, "case {case:?}: describe got {status} {body}");
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: describe got {body}"
        );
        let (status, body) =
            call(&router, "POST", &format!("/timeseries/delete?{query}"), None).await;
        assert_eq!(status, 404, "case {case:?}: delete got {status} {body}");
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: delete got {body}"
        );
    }

    // A LISTING is not a lookup of one series: scoped to a workspace the store
    // holds it is empty, and only a workspace it does not hold is the refusal.
    let (status, body) = call(
        &router,
        "GET",
        &format!("/timeseries?workspaceName={WORKSPACE}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "a scoped listing: got {status} {body}");
    assert_eq!(
        body["TimeSeriesSummaries"].as_array().map(Vec::len),
        Some(0),
        "a workspace holds no time series, so the listing is empty, got {body}"
    );
    assert_eq!(
        body["workspaceName"].as_str(),
        Some(WORKSPACE),
        "a scoped listing reports the workspace it was scoped to, got {body}"
    );
    let (status, body) = call(
        &router,
        "GET",
        "/timeseries?workspaceName=no-such-workspace",
        None,
    )
    .await;
    assert_eq!(status, 404, "a listing of an absent workspace: got {status} {body}");
}

#[tokio::test]
async fn workspace_encryption_configuration_is_checked() {
    let router = router();
    // `kmsKeyId` is "Required for KMS_BASED_ENCRYPTION; must be omitted for
    // SITEWISE_DEFAULT_ENCRYPTION", as the member documents; the rows are the two
    // `EncryptionType` values with and without a key.
    let key = format!("arn:aws:kms:{REGION}:123456789012:key/abcd");
    let cases: [(&str, &str, Option<&str>, u16); 4] = [
        ("default encryption", "SITEWISE_DEFAULT_ENCRYPTION", None, 200),
        (
            "default encryption with a key",
            "SITEWISE_DEFAULT_ENCRYPTION",
            Some(&key),
            400,
        ),
        ("kms encryption with a key", "KMS_BASED_ENCRYPTION", Some(&key), 200),
        ("kms encryption without a key", "KMS_BASED_ENCRYPTION", None, 400),
    ];
    for (at, (case, encryption_type, kms_key, expected)) in cases.into_iter().enumerate() {
        let mut configuration = json!({ "encryptionType": encryption_type });
        if let Some(kms_key) = kms_key {
            configuration
                .as_object_mut()
                .expect("an object body")
                .insert("kmsKeyId".to_string(), json!(kms_key));
        }
        let name = format!("workspace-{at}");
        let (status, body) = call(
            &router,
            "POST",
            "/workspaces",
            Some(json!({ "workspaceName": name, "encryptionConfiguration": configuration })),
        )
        .await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status} {body}"
        );
        if expected == 200 {
            assert_eq!(
                body["workspaceArn"].as_str(),
                Some(
                    format!(
                        "arn:aws:iotsitewise:{REGION}:{}:workspace/{name}",
                        account_id()
                    )
                    .as_str()
                ),
                "case {case:?}: got {body}"
            );
            assert_eq!(
                body["workspaceStatus"]["state"].as_str(),
                Some("CREATING"),
                "case {case:?}: a created workspace is CREATING, got {body}"
            );
        } else {
            assert_eq!(
                error_code(&body),
                "InvalidRequestException",
                "case {case:?}: got {body}"
            );
        }
    }

    // A second create of a name a workspace already holds collides with it.
    create_workspace(&router, WORKSPACE).await;
    let (status, body) = call(
        &router,
        "POST",
        "/workspaces",
        Some(json!({
            "workspaceName": WORKSPACE,
            "encryptionConfiguration": { "encryptionType": "SITEWISE_DEFAULT_ENCRYPTION" },
        })),
    )
    .await;
    assert_eq!(status, 409, "a repeated workspace name: got {status} {body}");
    assert_eq!(
        error_code(&body),
        "ConflictingOperationException",
        "a repeated workspace name: got {body}"
    );
}

/// The published bounds of `executionPriority` — `@range(min: 0, max: 2)` on
/// `StartPipelineExecution`'s input (smithy_query query="operation"
/// operation="StartPipelineExecution").
const PRIORITY_MIN: i64 = 0;
const PRIORITY_MAX: i64 = 2;

/// The priority an execution that names none runs at, from the member's own doc:
/// "Defaults to 2 when not specified."
const DEFAULT_PRIORITY: i64 = 2;

#[tokio::test]
async fn an_execution_records_the_priority_and_the_version_it_started_against() {
    let router = router();
    create_workspace(&router, WORKSPACE).await;
    create_task(&router, WORKSPACE, "alpha", &[], &[]).await;
    let (status, created) = create_pipeline(
        &router,
        WORKSPACE,
        PIPELINE,
        &[],
        &[Node::new("one", "alpha")],
    )
    .await;
    assert_eq!(status, 200, "create pipeline: got {status} {created}");
    let first_version = FIRST_PIPELINE_VERSION.to_string();

    // The rows differ in the priority the start names: none at all, each published
    // bound of the modelled range, and the value between them. Each row starts its
    // OWN execution of the SAME pipeline, so what a later read reports can only
    // have come from that execution's stored record.
    let cases: [(&str, Option<i64>, i64); 4] = [
        ("no priority named", None, DEFAULT_PRIORITY),
        ("the range minimum", Some(PRIORITY_MIN), PRIORITY_MIN),
        (
            "between the bounds",
            Some(PRIORITY_MIN + 1),
            PRIORITY_MIN + 1,
        ),
        ("the range maximum", Some(PRIORITY_MAX), PRIORITY_MAX),
    ];
    let mut started: Vec<(&str, String, i64)> = Vec::new();
    for (case, requested, expected) in cases {
        let body = match requested {
            Some(priority) => json!({ "executionPriority": priority }),
            None => json!({}),
        };
        let (status, answer) = start_execution(&router, WORKSPACE, PIPELINE, body).await;
        assert_eq!(status, 202, "case {case:?}: start got {status} {answer}");
        let id = answer["pipelineExecutionId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            is_modelled_id(&id),
            "case {case:?}: expected a modelled execution id, got {id:?}"
        );
        assert!(
            started.iter().all(|(_, earlier, _)| earlier != &id),
            "case {case:?}: expected an id no earlier start minted, got {id:?}"
        );
        started.push((case, id, expected));
    }

    for (case, id, expected) in &started {
        let (status, body) = describe_execution(&router, WORKSPACE, PIPELINE, id, "").await;
        assert_eq!(status, 200, "case {case:?}: describe got {status} {body}");
        assert_eq!(
            body["executionPriority"].as_i64(),
            Some(*expected),
            "case {case:?}: expected priority {expected}, got {body}"
        );
        assert_eq!(
            body["pipelineVersion"].as_str(),
            Some(first_version.as_str()),
            "case {case:?}: expected the pipeline version the start ran against \
             ({first_version}), got {body}"
        );
        assert_eq!(
            body["status"]["state"].as_str(),
            Some("NOT_STARTED"),
            "case {case:?}: nothing ran, so the execution is NOT_STARTED, got {body}"
        );
    }

    // One step past each published bound is outside the modelled range, so the
    // start is refused rather than recorded.
    for (case, priority) in [
        ("one below the range minimum", PRIORITY_MIN - 1),
        ("one above the range maximum", PRIORITY_MAX + 1),
    ] {
        let (status, body) = start_execution(
            &router,
            WORKSPACE,
            PIPELINE,
            json!({ "executionPriority": priority }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a priority outside the modelled range must be refused, \
             got {status} {body}"
        );
    }

    // A refused start writes nothing, so every execution above still reports the
    // priority it was started with.
    for (case, id, expected) in &started {
        let (status, body) = describe_execution(&router, WORKSPACE, PIPELINE, id, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: describe after the refusals got {status} {body}"
        );
        assert_eq!(
            body["executionPriority"].as_i64(),
            Some(*expected),
            "case {case:?}: a refused start must not change a stored execution, got {body}"
        );
    }

    // An execution runs against the pipeline's CURRENT version, so a start after an
    // update records the version that update wrote, runs the DAG that version
    // declares, and leaves every earlier execution on the version it started
    // against.
    let (status, updated) = call(
        &router,
        "PUT",
        &format!("/workspaces/{WORKSPACE}/pipelines/{PIPELINE}"),
        Some(json!({ "computations": [Node::new("two", "alpha").to_json()] })),
    )
    .await;
    assert_eq!(status, 200, "update pipeline: got {status} {updated}");
    let next_version = (FIRST_PIPELINE_VERSION + 1).to_string();
    assert_eq!(
        updated["version"].as_str(),
        Some(next_version.as_str()),
        "an update writes the next version, got {updated}"
    );

    let after_update = start_plain(&router, WORKSPACE, PIPELINE).await;
    let (status, body) = describe_execution(&router, WORKSPACE, PIPELINE, &after_update, "").await;
    assert_eq!(
        status, 200,
        "case \"after an update\": describe got {status} {body}"
    );
    assert_eq!(
        body["pipelineVersion"].as_str(),
        Some(next_version.as_str()),
        "case \"after an update\": expected version {next_version}, got {body}"
    );
    assert_eq!(
        reported_nodes(&body),
        vec!["two".to_string()],
        "case \"after an update\": the execution runs the updated DAG, got {body}"
    );
    for (case, id, _) in &started {
        let (status, body) = describe_execution(&router, WORKSPACE, PIPELINE, id, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: describe after the update got {status} {body}"
        );
        assert_eq!(
            body["pipelineVersion"].as_str(),
            Some(first_version.as_str()),
            "case {case:?}: an execution keeps the version it started against \
             ({first_version}), got {body}"
        );
    }
}
