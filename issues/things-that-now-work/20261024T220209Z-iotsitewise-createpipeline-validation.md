Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 564feb857476f3247a0693cb65aa2d44e3322d3a

# Workspace-scoped creates accept a workspaceName no workspace holds, on the strength of a comment that denies the crate has a workspace registry

_Filed: 2026-10-24T22:02:09Z_
_Operation: CreatePipeline | Class: validation_

> _Note: this issue was identified against the code base at commit `564feb857476f3247a0693cb65aa2d44e3322d3a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreatePipeline`, `CreateTask` and `CreateDatasetExportJob` are all bound at `/workspaces/{workspaceName}/...` and store their records keyed by `(workspaceName, name)`, but none of them ever looks the workspace up. Each carries the same comment asserting that `workspaceName` is "a caller-supplied string this service has no registry to resolve" — which is false inside this very crate: `CreateWorkspace` writes `store.workspaces`, and FOUR sibling handlers resolve a request's `workspaceName` through exactly that map and answer the modelled `ResourceNotFoundException` when it is absent (`DeleteTimeSeries`, `DescribeTimeSeries`, `ListTimeSeries`, `GetCaptureData`).

So a `POST /workspaces/never-created/pipelines` is accepted with 200 and a pipeline ARN naming a workspace that does not exist, while `GET /timeseries?workspaceName=never-created` on the same store is answered 404. The two halves of the same service disagree about whether a workspace name has to resolve. The crate's own test suites are split the same way: `tests/all/pipeline_executions.rs` creates the workspace first (`create_workspace(&router, WORKSPACE).await`, line 328 and others), while `tests/all/pipeline_lifecycle.rs` and `tests/all/task_lifecycle.rs` never create one at all and still get 200s.

The per-workspace cap the comment is justifying is a real concern, but it is a consequence of the missing lookup, not a reason for it: resolving `workspaceName` through `store.workspaces` the way the four siblings already do both fixes the refusal and closes the unbounded-bucket hole the comment describes.

## Evidence

Both sides are in this crate.

The handlers that skip the check, each asserting there is no registry:
- `src/ops/create_pipeline.rs:229-234`: "// The store is bounded: a create is the only thing that adds a pipeline and / nothing evicts one, so past either cap this is a quota refusal. The total / is checked as well as the per-workspace count, because `workspaceName` is / a caller-supplied string this service has no registry to resolve, so a / caller naming a fresh workspace each time would otherwise open one more / per-workspace bucket every time." — the whole handler (lines 177-295) never touches `store.workspaces`.
- `src/ops/create_task.rs:218-223`: "// The store is bounded, for the reason `CreatePipeline` states: ... because `workspaceName` is a caller-supplied string this service has no / registry to resolve ..."
- `src/ops/create_dataset_export_job.rs:217-223`: "// ... because `workspaceName` is a / caller-supplied string this service has no registry to resolve ..."

The registry those comments deny, and the siblings that use it:
- `src/ops/create_workspace.rs:269`: `store.workspaces.insert(record.name.clone(), record);`
- `src/ops/delete_time_series.rs:206-212`: `if let Some(workspace) = input.workspace_name.as_deref() { if !store.workspaces.contains_key(workspace) { return Err(not_found(crate::state::not_found_message(crate::state::RESOURCE_WORKSPACE, workspace,))); }`
- `src/ops/describe_time_series.rs:260-266`: same `!store.workspaces.contains_key(workspace)` -> `RESOURCE_WORKSPACE` 404.
- `src/ops/list_time_series.rs:182-188`: same check, same 404.
- `src/ops/get_capture_data.rs:223-228`: "// The workspace is what \"contains the capture source\", so a workspace this / store does not hold is reported as the absent resource it is" followed by `if !store.workspaces.contains_key(&input.workspace_name)`.

Model: `CreatePipeline`, `CreateTask` and `CreateDatasetExportJob` all bind `workspaceName` as an `@httpLabel` on `/workspaces/{workspaceName}/...` and all three declare `ResourceNotFoundException` (HTTP 404) in their error sets (`smithy_query(query="operation", operation="CreatePipeline,CreateTask,CreateDatasetExportJob")`).

Test coverage: `tests/all/pipeline_lifecycle.rs` (header lines 8-23 pins version/DAG/scoping rules, never creates a workspace) and `tests/all/task_lifecycle.rs` exercise these creates with no workspace in the store; `tests/all/pipeline_executions.rs:145,328` creates the workspace first. `tests/all/workspace_lifecycle.rs` covers the workspace record itself. No test states a reason for accepting an unknown workspace.

## Steps to Reproduce

1. Boot the emulator with an empty IoT SiteWise store (create no workspace).
2. `POST /workspaces/never-created/pipelines` with `{"pipelineName":"p1","computations":[]}` -> 200, with `pipelineArn` = `arn:aws:iotsitewise:<region>:<account>:pipeline/never-created/p1`.
3. `GET /workspaces/never-created/pipelines/p1` -> 200: the pipeline is persisted inside a workspace that does not exist.
4. `POST /workspaces/never-created/tasks` and `POST /workspaces/never-created/dataset-export-jobs` -> likewise accepted.
5. Now call `GET /timeseries?workspaceName=never-created` (ListTimeSeries) on the same store -> 404 ResourceNotFoundException naming the workspace. The two paths disagree about the same name.
