// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the `clientToken` `StartPipelineExecution` takes, driven
//! over HTTP through the same `MultiServiceRouter` the emulator boots with, so
//! every assertion is about the wire behaviour a real client sees.
//!
//! The rule these suites pin: a `clientToken` is spent by ONE start. A repeat of
//! the SAME start under that token is a RETRY of that call — answered with the
//! execution that call queued, queueing nothing further — while a DIFFERENT
//! start reusing the token is refused `ConflictingOperationException` (409)
//! naming the execution the first call queued, and queues nothing at all. Which
//! start a request IS covers every member that decides what the run does: the
//! pipeline it runs, the scheduling priority it runs at, and the environment
//! variable and mount overrides it carries. A priority the request omits is the
//! one the member documents as its default, so a start that states that value
//! explicitly is the same start, and an override map's ENTRIES decide the start
//! while the order they happen to be held in does not.
//!
//! The rows span what the rule generalises over: the member the second start
//! differs in (the priority, a global environment override, a per-node
//! environment override, which node is overridden, a mount override, and the
//! pipeline addressed), and whether that member is changed or merely added.
//!
//! The HTTP harness (`call`, `error_code`, `router`), the token helpers
//! (`client_token`, `with_token`) and the pipeline fixtures (`create_workspace`,
//! `create_task`, `create_pipeline`, `start_execution`, `describe_execution`,
//! `mount`, `Node`) are the sibling suites'; this suite reuses them rather than
//! growing a second copy.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::asset_lifecycle::{call, error_code, router, router_with_engine};
use crate::client_token_idempotency::{client_token, with_token};
use crate::hierarchies_etags_and_refusals::error_message;
use crate::pipeline_execution_cancellation::set_run_state;
use crate::pipeline_executions::{
    create_pipeline, create_task, create_workspace, describe_execution, mount, start_execution,
    Node,
};

/// The workspace every suite below works in.
const WORKSPACE: &str = "token-execution-workspace";

/// The pipeline every row's first start runs.
const PIPELINE: &str = "nightly-rollup";

/// A second pipeline of the same workspace, so a start addressing ANOTHER
/// pipeline under a spent token can be shown to queue nothing there either.
const OTHER_PIPELINE: &str = "weekly-rollup";

/// The task both pipelines run on each of their compute nodes.
const TASK: &str = "transform";

/// The compute nodes `PIPELINE` declares, so a row can override one node and not
/// another.
const FIRST_NODE: &str = "extract";
const SECOND_NODE: &str = "load";

/// The wording a refusal carries when a DIFFERENT request reuses a spent token:
/// AWS names the DIMENSION of the request that collided rather than an
/// identifier, which is the wording every other write of this service states it
/// with.
const SPENT_TOKEN_MESSAGE: &str = "Another pipeline execution is already using the same clientToken";

/// The published bounds of `executionPriority` — `@range(min: 0, max: 2)` on
/// `StartPipelineExecution`'s input (smithy_query query="operation"
/// operation="StartPipelineExecution"), and the priority a start that names none
/// runs at, from the member's own doc: "Defaults to 2 when not specified."
const PRIORITY_MIN: i64 = 0;
const DEFAULT_PRIORITY: i64 = 2;

/// `GET /workspaces/{workspaceName}/pipelines/{pipelineName}/executions`.
async fn listed_ids(router: &MultiServiceRouter, pipeline: &str) -> Vec<String> {
    let (status, body) = call(
        router,
        "GET",
        &format!("/workspaces/{WORKSPACE}/pipelines/{pipeline}/executions"),
        None,
    )
    .await;
    assert_eq!(status, 200, "list {pipeline}: got {status} {body}");
    body["pipelineExecutionSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("list {pipeline}: no pipelineExecutionSummaries in {body}"))
        .iter()
        .map(|summary| {
            summary["pipelineExecutionId"]
                .as_str()
                .unwrap_or_else(|| panic!("list {pipeline}: a summary without an id: {summary}"))
                .to_string()
        })
        .collect()
}

/// The workspace, the task and the two pipelines every row starts executions of,
/// with the ARN each pipeline reports — what a conflict over one of its
/// executions names.
async fn fixture(router: &MultiServiceRouter) -> Vec<(&'static str, String)> {
    create_workspace(router, WORKSPACE).await;
    create_task(
        router,
        WORKSPACE,
        TASK,
        &[("LOG_LEVEL", "warn")],
        &[mount("inputs", "inputs")],
    )
    .await;
    let mut arns = Vec::new();
    for (pipeline, nodes) in [
        (
            PIPELINE,
            vec![Node::new(FIRST_NODE, TASK), Node::new(SECOND_NODE, TASK)],
        ),
        (OTHER_PIPELINE, vec![Node::new(FIRST_NODE, TASK)]),
    ] {
        let (status, body) = create_pipeline(router, WORKSPACE, pipeline, &[], &nodes).await;
        assert_eq!(status, 200, "create pipeline {pipeline}: got {status} {body}");
        arns.push((
            pipeline,
            body["pipelineArn"]
                .as_str()
                .unwrap_or_else(|| panic!("create pipeline {pipeline}: no ARN in {body}"))
                .to_string(),
        ));
    }
    arns
}

/// One row of the spent-token table: a start that spends a token, the DIFFERENT
/// start that reuses it, and the priority the first start's execution must still
/// report afterwards.
struct Row {
    case: &'static str,
    /// The seed this row's token is built from: non-whitespace, as
    /// `ClientToken`'s `^\S{36,64}$` requires.
    token_seed: &'static str,
    /// The pipeline the first start addresses, and the one the second does — the
    /// same, except for the row that differs in the pipeline.
    pipeline: &'static str,
    other_pipeline: &'static str,
    body: Value,
    other_body: Value,
    /// The `executionPriority` a read of the queued execution must report: the
    /// FIRST start's, never the refused one's.
    priority: i64,
}

/// Environment variable overrides carrying `global` and, when `node` is given,
/// that node's own map. Several keys per map, so the rendering the registry
/// recognises a start by cannot depend on the order a map happens to iterate in.
fn environment_overrides(global: &[(&str, &str)], node: Option<(&str, &[(&str, &str)])>) -> Value {
    let mut overrides = json!({ "global": map_of(global) });
    if let Some((name, variables)) = node {
        overrides
            .as_object_mut()
            .expect("an object of overrides")
            .insert(
                "computeNodes".to_string(),
                json!({ name: map_of(variables) }),
            );
    }
    overrides
}

/// A JSON object of `pairs`.
fn map_of(pairs: &[(&str, &str)]) -> Value {
    Value::Object(
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), json!(value)))
            .collect(),
    )
}

/// The environment variables the table's rows override globally, and the second
/// value of the row that changes one of them.
fn global_overrides() -> Value {
    environment_overrides(&[("LOG_LEVEL", "debug"), ("SHARD", "1"), ("REGION", "a")], None)
}

fn other_global_overrides() -> Value {
    environment_overrides(&[("LOG_LEVEL", "info"), ("SHARD", "1"), ("REGION", "a")], None)
}

/// Mount overrides for `node`, replacing the task-defined `inputs` mount with
/// one rooted at `path`.
fn mount_overrides(node: &str, path: &str) -> Value {
    json!({ "computeNodes": { node: [mount("inputs", path)] } })
}

/// The rows, one per member the second start differs in.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "another executionPriority",
            token_seed: "start-execution-another-priority",
            pipeline: PIPELINE,
            other_pipeline: PIPELINE,
            body: json!({}),
            other_body: json!({ "executionPriority": PRIORITY_MIN }),
            priority: DEFAULT_PRIORITY,
        },
        Row {
            case: "another global environment override",
            token_seed: "start-execution-another-global-env",
            pipeline: PIPELINE,
            other_pipeline: PIPELINE,
            body: json!({
                "executionPriority": PRIORITY_MIN,
                "executionEnvironmentVariableOverrides": global_overrides(),
            }),
            other_body: json!({
                "executionPriority": PRIORITY_MIN,
                "executionEnvironmentVariableOverrides": other_global_overrides(),
            }),
            priority: PRIORITY_MIN,
        },
        Row {
            case: "a global environment override added",
            token_seed: "start-execution-env-override-added",
            pipeline: PIPELINE,
            other_pipeline: PIPELINE,
            body: json!({}),
            other_body: json!({
                "executionEnvironmentVariableOverrides": global_overrides(),
            }),
            priority: DEFAULT_PRIORITY,
        },
        Row {
            case: "another per-node environment override",
            token_seed: "start-execution-another-node-env",
            pipeline: PIPELINE,
            other_pipeline: PIPELINE,
            body: json!({
                "executionEnvironmentVariableOverrides": environment_overrides(
                    &[("LOG_LEVEL", "debug")],
                    Some((FIRST_NODE, &[("SHARD", "1"), ("PASS", "first")])),
                ),
            }),
            other_body: json!({
                "executionEnvironmentVariableOverrides": environment_overrides(
                    &[("LOG_LEVEL", "debug")],
                    Some((FIRST_NODE, &[("SHARD", "2"), ("PASS", "first")])),
                ),
            }),
            priority: DEFAULT_PRIORITY,
        },
        Row {
            case: "another compute node overridden",
            token_seed: "start-execution-another-node-named",
            pipeline: PIPELINE,
            other_pipeline: PIPELINE,
            body: json!({
                "executionEnvironmentVariableOverrides": environment_overrides(
                    &[("LOG_LEVEL", "debug")],
                    Some((FIRST_NODE, &[("SHARD", "1")])),
                ),
            }),
            other_body: json!({
                "executionEnvironmentVariableOverrides": environment_overrides(
                    &[("LOG_LEVEL", "debug")],
                    Some((SECOND_NODE, &[("SHARD", "1")])),
                ),
            }),
            priority: DEFAULT_PRIORITY,
        },
        Row {
            case: "another mount override",
            token_seed: "start-execution-another-mount",
            pipeline: PIPELINE,
            other_pipeline: PIPELINE,
            body: json!({ "executionMountOverrides": mount_overrides(FIRST_NODE, "runs/first") }),
            other_body: json!({
                "executionMountOverrides": mount_overrides(FIRST_NODE, "runs/second"),
            }),
            priority: DEFAULT_PRIORITY,
        },
        Row {
            case: "a mount override added",
            token_seed: "start-execution-mount-override-added",
            pipeline: PIPELINE,
            other_pipeline: PIPELINE,
            body: json!({}),
            other_body: json!({
                "executionMountOverrides": mount_overrides(FIRST_NODE, "runs/first"),
            }),
            priority: DEFAULT_PRIORITY,
        },
        Row {
            case: "another pipeline",
            token_seed: "start-execution-another-pipeline",
            pipeline: PIPELINE,
            other_pipeline: OTHER_PIPELINE,
            body: json!({ "executionPriority": PRIORITY_MIN }),
            other_body: json!({ "executionPriority": PRIORITY_MIN }),
            priority: PRIORITY_MIN,
        },
    ]
}

#[tokio::test]
async fn a_token_one_start_spent_refuses_a_different_start_and_queues_nothing() {
    for row in rows() {
        // A router per row, so the listings below report exactly what this row's
        // own calls left behind.
        let router = router();
        let arns = fixture(&router).await;
        let pipeline_arn = arns
            .iter()
            .find(|(pipeline, _)| *pipeline == row.pipeline)
            .map(|(_, arn)| arn.clone())
            .expect("the fixture created this row's pipeline");
        let token = client_token(row.token_seed);

        let (status, first) = start_execution(
            &router,
            WORKSPACE,
            row.pipeline,
            with_token(&row.body, &token),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {:?}: the first start expected 202, got {status} {first}",
            row.case
        );
        let queued = first["pipelineExecutionId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: no execution id in {first}", row.case))
            .to_string();

        // The SAME start under that token is a retry of the call above: it is
        // answered with the execution that call queued and queues no second one.
        let (status, retried) = start_execution(
            &router,
            WORKSPACE,
            row.pipeline,
            with_token(&row.body, &token),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {:?}: the retry expected 202, got {status} {retried}",
            row.case
        );
        assert_eq!(
            retried["pipelineExecutionId"].as_str(),
            Some(queued.as_str()),
            "case {:?}: a retry replays the execution the first start queued ({queued}), \
             got {retried}",
            row.case
        );
        let (status, before) =
            describe_execution(&router, WORKSPACE, row.pipeline, &queued, "").await;
        assert_eq!(
            status, 200,
            "case {:?}: describe expected 200, got {status} {before}",
            row.case
        );

        // A DIFFERENT start reusing the spent token is the modelled conflict,
        // naming the execution the first call queued and the ARN of the pipeline
        // it belongs to.
        let (status, refused) = start_execution(
            &router,
            WORKSPACE,
            row.other_pipeline,
            with_token(&row.other_body, &token),
        )
        .await;
        assert_eq!(
            (status, error_code(&refused)),
            (409, "ConflictingOperationException"),
            "case {:?}: a spent token expected the modelled conflict, got {status} {refused}",
            row.case
        );
        assert_eq!(
            refused["resourceId"].as_str(),
            Some(queued.as_str()),
            "case {:?}: the conflict names the execution the first start queued ({queued}), \
             got {refused}",
            row.case
        );
        assert_eq!(
            refused["resourceArn"].as_str(),
            Some(pipeline_arn.as_str()),
            "case {:?}: the conflict names that execution's pipeline ({pipeline_arn}), \
             got {refused}",
            row.case
        );
        assert_eq!(
            error_message(&refused),
            SPENT_TOKEN_MESSAGE,
            "case {:?}: expected the spent-token wording, got {refused}",
            row.case
        );

        // The refused start queued nothing: each pipeline holds exactly the
        // executions the accepted call left it, and the one that was queued still
        // reports the FIRST start's own request.
        for (pipeline, _) in &arns {
            let held = listed_ids(&router, pipeline).await;
            let expected: Vec<String> = if *pipeline == row.pipeline {
                vec![queued.clone()]
            } else {
                Vec::new()
            };
            assert_eq!(
                held, expected,
                "case {:?}: a refused start queues nothing, {pipeline} holds {held:?}",
                row.case
            );
        }
        let (status, after) =
            describe_execution(&router, WORKSPACE, row.pipeline, &queued, "").await;
        assert_eq!(
            status, 200,
            "case {:?}: describe after the refusal expected 200, got {status} {after}",
            row.case
        );
        assert_eq!(
            after, before,
            "case {:?}: a refused start writes nothing to the execution it collided with, \
             got {after}",
            row.case
        );
        assert_eq!(
            after["executionPriority"].as_i64(),
            Some(row.priority),
            "case {:?}: expected the first start's priority {}, got {after}",
            row.case,
            row.priority
        );
    }
}

/// One row of the replay table: a start, and a second request that ASKS FOR THE
/// SAME RUN, so the token it reuses makes it a retry rather than a conflict.
struct ReplayRow {
    case: &'static str,
    token_seed: &'static str,
    body: Value,
    same_run: Value,
}

/// The rows: the identical request, the default priority stated explicitly
/// instead of omitted (and the other way round), and the same override maps
/// written in another key order.
fn replay_rows() -> Vec<ReplayRow> {
    let overrides = json!({
        "executionEnvironmentVariableOverrides": environment_overrides(
            &[("LOG_LEVEL", "debug"), ("SHARD", "1"), ("REGION", "a")],
            Some((FIRST_NODE, &[("PASS", "first"), ("DEPTH", "3")])),
        ),
        "executionMountOverrides": mount_overrides(FIRST_NODE, "runs/first"),
    });
    vec![
        ReplayRow {
            case: "the identical request, overrides and all",
            token_seed: "start-execution-identical-request",
            body: overrides.clone(),
            same_run: overrides,
        },
        ReplayRow {
            case: "the default priority stated explicitly",
            token_seed: "start-execution-default-priority-named",
            body: json!({}),
            same_run: json!({ "executionPriority": DEFAULT_PRIORITY }),
        },
        ReplayRow {
            case: "the default priority omitted instead",
            token_seed: "start-execution-default-priority-omitted",
            body: json!({ "executionPriority": DEFAULT_PRIORITY }),
            same_run: json!({}),
        },
        ReplayRow {
            case: "the same overrides in another key order",
            token_seed: "start-execution-overrides-reordered",
            body: json!({
                "executionEnvironmentVariableOverrides": environment_overrides(
                    &[("LOG_LEVEL", "debug"), ("SHARD", "1"), ("REGION", "a")],
                    Some((FIRST_NODE, &[("PASS", "first"), ("DEPTH", "3")])),
                ),
            }),
            same_run: json!({
                "executionEnvironmentVariableOverrides": environment_overrides(
                    &[("REGION", "a"), ("SHARD", "1"), ("LOG_LEVEL", "debug")],
                    Some((FIRST_NODE, &[("DEPTH", "3"), ("PASS", "first")])),
                ),
            }),
        },
    ]
}

#[tokio::test]
async fn a_start_asking_for_the_same_run_replays_it_rather_than_colliding() {
    for row in replay_rows() {
        let router = router();
        fixture(&router).await;
        let token = client_token(row.token_seed);

        let (status, first) =
            start_execution(&router, WORKSPACE, PIPELINE, with_token(&row.body, &token)).await;
        assert_eq!(
            status, 202,
            "case {:?}: the first start expected 202, got {status} {first}",
            row.case
        );
        let queued = first["pipelineExecutionId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: no execution id in {first}", row.case))
            .to_string();

        let (status, replayed) = start_execution(
            &router,
            WORKSPACE,
            PIPELINE,
            with_token(&row.same_run, &token),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {:?}: a start asking for the same run expected 202, got {status} {replayed}",
            row.case
        );
        assert_eq!(
            replayed["pipelineExecutionId"].as_str(),
            Some(queued.as_str()),
            "case {:?}: expected the execution the first start queued ({queued}), got {replayed}",
            row.case
        );
        let held = listed_ids(&router, PIPELINE).await;
        assert_eq!(
            held,
            vec![queued.clone()],
            "case {:?}: a retry queues no second execution, the pipeline holds {held:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn a_token_spent_on_a_start_whose_execution_is_gone_serves_the_next_start() {
    // The conflict names the execution the spent token's call queued, so a token
    // whose execution this store no longer holds — its pipeline was deleted and
    // created afresh — has nothing to name: the next start under it is served as
    // the start it is.
    let (router, engine) = router_with_engine();
    fixture(&router).await;
    let token = client_token("start-execution-outlives-its-execution");

    let (status, first) = start_execution(
        &router,
        WORKSPACE,
        PIPELINE,
        with_token(&json!({ "executionPriority": PRIORITY_MIN }), &token),
    )
    .await;
    assert_eq!(status, 202, "the first start got {status} {first}");
    let gone = first["pipelineExecutionId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // A pipeline holding an active execution cannot be deleted, and nothing runs
    // here, so the queued execution is put in a terminal state through the store
    // the way the cancellation suite does.
    set_run_state(&engine, &gone, "SUCCEEDED");
    let (status, deleted) = call(
        &router,
        "DELETE",
        &format!("/workspaces/{WORKSPACE}/pipelines/{PIPELINE}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "delete pipeline: got {status} {deleted}");
    let (status, recreated) = create_pipeline(
        &router,
        WORKSPACE,
        PIPELINE,
        &[],
        &[Node::new(FIRST_NODE, TASK)],
    )
    .await;
    assert_eq!(status, 200, "recreate pipeline: got {status} {recreated}");

    let (status, served) = start_execution(
        &router,
        WORKSPACE,
        PIPELINE,
        with_token(&json!({ "executionPriority": DEFAULT_PRIORITY }), &token),
    )
    .await;
    assert_eq!(
        status, 202,
        "a token whose execution is gone must be served, got {status} {served}"
    );
    let fresh = served["pipelineExecutionId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_ne!(
        fresh, gone,
        "expected an execution of its own, got the forgotten {gone}"
    );
    let held = listed_ids(&router, PIPELINE).await;
    assert_eq!(
        held,
        vec![fresh.clone()],
        "the recreated pipeline holds exactly the execution the served start queued, got {held:?}"
    );
}
