// SPDX-License-Identifier: CC0-1.0

//! Every operation whose route nests a resource inside `/workspaces/{name}`
//! resolves the path's resources OUTSIDE IN, so the refusal names the OUTERMOST
//! resource the request got wrong: the workspace when the store holds no
//! workspace of that name, and only then the pipeline, task, query, export job or
//! pipeline execution inside it.
//!
//! The rows span that whole class of routes — the reads, the writes and the
//! deletes, at every nesting depth the service has (one, two and three path
//! resources) — rather than the one request an issue happened to record, and each
//! row carries its own identifier so the expected message is built from what the
//! row sent.
//!
//! The expected strings are written out BY HAND as an independent oracle: this
//! suite never calls the crate's message helpers, so a handler that drifts to a
//! template of its own cannot agree with itself.
//!
//! Everything is driven over HTTP through the same `MultiServiceRouter` the
//! emulator boots with, so every assertion is about the wire behaviour a client
//! sees.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::asset_lifecycle::{call, error_code, router};
use super::hierarchies_etags_and_refusals::error_message;
use super::pipeline_executions::{
    create_pipeline, create_task, create_workspace, start_plain, Node,
};

/// The workspace the fixture opens, so a row can be wrong only INSIDE it.
const WORKSPACE: &str = "resolution-workspace";

/// The pipeline the fixture creates in it, and the task its compute node runs.
const PIPELINE: &str = "nightly-rollup";
const TASK: &str = "alpha";

/// Workspace names no workspace answers to, each inside the modelled
/// `WorkspaceName` (len 1..64, pattern `^[a-zA-Z0-9_-]+$`): a hyphenated name, a
/// mixed-case name with an underscore and digits, and one at the published
/// maximum length.
const UNKNOWN_WORKSPACES: [&str; 3] = [
    "no-such-workspace",
    "Absent_Workspace_42",
    "wwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwww",
];

/// An `ID` (len 36..36, lower-case hex groups) no pipeline execution was ever
/// started under, and which is not the identifier the pattern excludes.
const UNKNOWN_EXECUTION: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

/// A `DatasetExportJobId` (the same 36-character form) no export job holds.
const UNKNOWN_JOB: &str = "11111111-2222-3333-4444-555555555555";

/// A `QueryId` (len 1..64, pattern `^[a-zA-Z0-9-]+$`) no query holds.
const UNKNOWN_QUERY: &str = "no-such-query";

/// Resource names nothing in the fixture was created under.
const UNKNOWN_PIPELINE: &str = "no-such-pipeline";
const UNKNOWN_TASK: &str = "no-such-task";

/// The resource KINDS this service words its absences with, as the wire reports
/// them.
const WORKSPACE_KIND: &str = "Workspace";
const PIPELINE_KIND: &str = "Pipeline";
const PIPELINE_EXECUTION_KIND: &str = "Pipeline Execution";
const TASK_KIND: &str = "Task";
const QUERY_KIND: &str = "Query";
const EXPORT_JOB_KIND: &str = "Dataset Export Job";

/// One row: the case name, the HTTP method, the URI, the request body, and the
/// absence the refusal must state.
type Row = (&'static str, &'static str, String, Option<Value>, String);

/// How this service states an absence, written out by hand.
fn absent(kind: &str, identifier: &str) -> String {
    format!("{kind} with ID {identifier} not found")
}

/// A workspace holding one task and one pipeline, with one execution of that
/// pipeline started, so every row below is wrong in exactly the place it says.
async fn fixture(router: &MultiServiceRouter) -> String {
    create_workspace(router, WORKSPACE).await;
    create_task(router, WORKSPACE, TASK, &[], &[]).await;
    let (status, body) = create_pipeline(
        router,
        WORKSPACE,
        PIPELINE,
        &[],
        &[Node::new("only", TASK)],
    )
    .await;
    assert_eq!(status, 200, "create pipeline: got {status} {body}");
    start_plain(router, WORKSPACE, PIPELINE).await
}

/// Drive `rows` and assert each is the modelled 404 stating the absence the row
/// expects.
async fn assert_rows(router: &MultiServiceRouter, rows: Vec<Row>) {
    for (case, method, uri, body, expected) in rows {
        let (status, answered) = call(router, method, &uri, body).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {answered}"
        );
        assert_eq!(
            error_code(&answered),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {answered}"
        );
        assert_eq!(
            error_message(&answered),
            expected,
            "case {case:?}: expected {expected:?}, got {answered}"
        );
    }
}

#[tokio::test]
async fn a_workspace_this_service_does_not_hold_is_the_absence_every_route_under_it_reports() {
    let router = router();
    let execution = fixture(&router).await;

    // Each row names a workspace the store holds no record of. The rest of the
    // path is sometimes right and sometimes wrong, which changes nothing: the
    // workspace is the outermost resource the request got wrong, so it is the one
    // the refusal names. The three unknown workspace names rotate through the rows
    // so the expected message is built from the identifier the row sent.
    let [first, second, third] = UNKNOWN_WORKSPACES;
    let rows: Vec<Row> = vec![
        (
            "DescribePipelineExecution, the execution's own pipeline and id",
            "GET",
            format!("/workspaces/{first}/pipelines/{PIPELINE}/executions/{execution}"),
            None,
            absent(WORKSPACE_KIND, first),
        ),
        (
            "DescribePipelineExecution, wrong in all three resources",
            "GET",
            format!(
                "/workspaces/{second}/pipelines/{UNKNOWN_PIPELINE}/executions/{UNKNOWN_EXECUTION}"
            ),
            None,
            absent(WORKSPACE_KIND, second),
        ),
        (
            "DescribePipelineExecution, a workspace name at the modelled maximum length",
            "GET",
            format!("/workspaces/{third}/pipelines/{PIPELINE}/executions/{execution}"),
            None,
            absent(WORKSPACE_KIND, third),
        ),
        (
            "CancelPipelineExecution, the same three values",
            "POST",
            format!("/workspaces/{first}/pipelines/{PIPELINE}/executions/{execution}/cancel"),
            Some(json!({})),
            absent(WORKSPACE_KIND, first),
        ),
        (
            "ListPipelineExecutions",
            "GET",
            format!("/workspaces/{second}/pipelines/{PIPELINE}/executions"),
            None,
            absent(WORKSPACE_KIND, second),
        ),
        (
            "StartPipelineExecution",
            "POST",
            format!("/workspaces/{third}/pipelines/{PIPELINE}/executions"),
            Some(json!({})),
            absent(WORKSPACE_KIND, third),
        ),
        (
            "DescribePipeline",
            "GET",
            format!("/workspaces/{first}/pipelines/{PIPELINE}"),
            None,
            absent(WORKSPACE_KIND, first),
        ),
        (
            "DescribePipeline, an earlier version of it",
            "GET",
            format!("/workspaces/{second}/pipelines/{PIPELINE}?version=1"),
            None,
            absent(WORKSPACE_KIND, second),
        ),
        (
            "UpdatePipeline",
            "PUT",
            format!("/workspaces/{third}/pipelines/{PIPELINE}"),
            Some(json!({ "description": "whatever" })),
            absent(WORKSPACE_KIND, third),
        ),
        (
            "DeletePipeline",
            "DELETE",
            format!("/workspaces/{first}/pipelines/{PIPELINE}"),
            None,
            absent(WORKSPACE_KIND, first),
        ),
        (
            "ListPipelines",
            "GET",
            format!("/workspaces/{second}/pipelines"),
            None,
            absent(WORKSPACE_KIND, second),
        ),
        (
            "DescribeTask",
            "GET",
            format!("/workspaces/{third}/tasks/{TASK}"),
            None,
            absent(WORKSPACE_KIND, third),
        ),
        (
            "UpdateTask",
            "PUT",
            format!("/workspaces/{first}/tasks/{TASK}"),
            Some(json!({ "description": "whatever" })),
            absent(WORKSPACE_KIND, first),
        ),
        (
            "ListTasks",
            "GET",
            format!("/workspaces/{second}/tasks"),
            None,
            absent(WORKSPACE_KIND, second),
        ),
        (
            "DescribeQuery",
            "GET",
            format!("/workspaces/{third}/queries/{UNKNOWN_QUERY}"),
            None,
            absent(WORKSPACE_KIND, third),
        ),
        (
            "DescribeDatasetExportJob",
            "GET",
            format!("/workspaces/{first}/dataset-export-jobs/{UNKNOWN_JOB}"),
            None,
            absent(WORKSPACE_KIND, first),
        ),
    ];
    assert_rows(&router, rows).await;
}

#[tokio::test]
async fn a_workspace_that_resolves_refuses_the_resource_inside_it_the_path_names() {
    let router = router();
    let execution = fixture(&router).await;

    // Each row names the workspace the fixture opened, so the outermost resource
    // the request gets wrong is the one INSIDE it: the pipeline, the task, the
    // query, the export job, or — with the workspace and the pipeline both right —
    // the execution itself.
    let rows: Vec<Row> = vec![
        (
            "DescribePipelineExecution, a pipeline nothing created",
            "GET",
            format!(
                "/workspaces/{WORKSPACE}/pipelines/{UNKNOWN_PIPELINE}/executions/{execution}"
            ),
            None,
            absent(PIPELINE_KIND, UNKNOWN_PIPELINE),
        ),
        (
            "DescribePipelineExecution, an execution id nothing started",
            "GET",
            format!(
                "/workspaces/{WORKSPACE}/pipelines/{PIPELINE}/executions/{UNKNOWN_EXECUTION}"
            ),
            None,
            absent(PIPELINE_EXECUTION_KIND, UNKNOWN_EXECUTION),
        ),
        (
            "CancelPipelineExecution, a pipeline nothing created",
            "POST",
            format!(
                "/workspaces/{WORKSPACE}/pipelines/{UNKNOWN_PIPELINE}/executions/{execution}/cancel"
            ),
            Some(json!({})),
            absent(PIPELINE_KIND, UNKNOWN_PIPELINE),
        ),
        (
            "CancelPipelineExecution, an execution id nothing started",
            "POST",
            format!(
                "/workspaces/{WORKSPACE}/pipelines/{PIPELINE}/executions/{UNKNOWN_EXECUTION}/cancel"
            ),
            Some(json!({})),
            absent(PIPELINE_EXECUTION_KIND, UNKNOWN_EXECUTION),
        ),
        (
            "ListPipelineExecutions, a pipeline nothing created",
            "GET",
            format!("/workspaces/{WORKSPACE}/pipelines/{UNKNOWN_PIPELINE}/executions"),
            None,
            absent(PIPELINE_KIND, UNKNOWN_PIPELINE),
        ),
        (
            "StartPipelineExecution, a pipeline nothing created",
            "POST",
            format!("/workspaces/{WORKSPACE}/pipelines/{UNKNOWN_PIPELINE}/executions"),
            Some(json!({})),
            absent(PIPELINE_KIND, UNKNOWN_PIPELINE),
        ),
        (
            "DescribePipeline, a pipeline nothing created",
            "GET",
            format!("/workspaces/{WORKSPACE}/pipelines/{UNKNOWN_PIPELINE}"),
            None,
            absent(PIPELINE_KIND, UNKNOWN_PIPELINE),
        ),
        (
            "UpdatePipeline, a pipeline nothing created",
            "PUT",
            format!("/workspaces/{WORKSPACE}/pipelines/{UNKNOWN_PIPELINE}"),
            Some(json!({ "description": "whatever" })),
            absent(PIPELINE_KIND, UNKNOWN_PIPELINE),
        ),
        (
            "DeletePipeline, a pipeline nothing created",
            "DELETE",
            format!("/workspaces/{WORKSPACE}/pipelines/{UNKNOWN_PIPELINE}"),
            None,
            absent(PIPELINE_KIND, UNKNOWN_PIPELINE),
        ),
        (
            "DescribeTask, a task nothing created",
            "GET",
            format!("/workspaces/{WORKSPACE}/tasks/{UNKNOWN_TASK}"),
            None,
            absent(TASK_KIND, UNKNOWN_TASK),
        ),
        (
            "UpdateTask, a task nothing created",
            "PUT",
            format!("/workspaces/{WORKSPACE}/tasks/{UNKNOWN_TASK}"),
            Some(json!({ "description": "whatever" })),
            absent(TASK_KIND, UNKNOWN_TASK),
        ),
        (
            "DescribeQuery, a query nothing started",
            "GET",
            format!("/workspaces/{WORKSPACE}/queries/{UNKNOWN_QUERY}"),
            None,
            absent(QUERY_KIND, UNKNOWN_QUERY),
        ),
        (
            "DescribeDatasetExportJob, a job nothing created",
            "GET",
            format!("/workspaces/{WORKSPACE}/dataset-export-jobs/{UNKNOWN_JOB}"),
            None,
            absent(EXPORT_JOB_KIND, UNKNOWN_JOB),
        ),
    ];
    assert_rows(&router, rows).await;
}

#[tokio::test]
async fn resolving_outside_in_still_serves_every_path_the_store_does_hold() {
    let router = router();
    let execution = fixture(&router).await;

    // The added resolution refuses nothing that resolves: each row is the same
    // route as a refusal above, with every resource on it present.
    let served: [(&str, String); 5] = [
        (
            "DescribePipelineExecution",
            format!("/workspaces/{WORKSPACE}/pipelines/{PIPELINE}/executions/{execution}"),
        ),
        (
            "DescribePipeline",
            format!("/workspaces/{WORKSPACE}/pipelines/{PIPELINE}"),
        ),
        (
            "ListPipelineExecutions",
            format!("/workspaces/{WORKSPACE}/pipelines/{PIPELINE}/executions"),
        ),
        (
            "DescribeTask",
            format!("/workspaces/{WORKSPACE}/tasks/{TASK}"),
        ),
        ("ListPipelines", format!("/workspaces/{WORKSPACE}/pipelines")),
    ];
    for (case, uri) in served {
        let (status, body) = call(&router, "GET", &uri, None).await;
        assert_eq!(status, 200, "case {case:?}: got {status} {body}");
    }

    // And the execution the fixture started is still the one its own path reports.
    let (status, body) = call(
        &router,
        "GET",
        &format!("/workspaces/{WORKSPACE}/pipelines/{PIPELINE}/executions/{execution}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "the execution's own path: got {status} {body}");
    assert_eq!(
        body["pipelineExecutionId"].as_str(),
        Some(execution.as_str()),
        "the execution's own path reports it, expected {execution}, got {body}"
    );
    assert_eq!(
        body["workspaceName"].as_str(),
        Some(WORKSPACE),
        "the execution's own path reports its workspace, expected {WORKSPACE}, got {body}"
    );
}
