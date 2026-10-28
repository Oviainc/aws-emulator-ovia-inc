Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 5cb591631d698c9098c00693287b08686d77b0d1

# DescribePipelineExecution resolves only the execution id, skipping the workspace and pipeline its sibling CancelPipelineExecution resolves outside-in

_Filed: 2026-10-28T18:04:05Z_
_Operation: DescribePipelineExecution | Class: error-response_

> _Note: this issue was identified against the code base at commit `5cb591631d698c9098c00693287b08686d77b0d1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

DescribePipelineExecution takes the same three path/body components as CancelPipelineExecution (`workspaceName`, `pipelineName`, `pipelineExecutionId`), but it resolves ONLY the execution id and then filters the record by the two names. So a request naming a workspace this store holds no record of, or a pipeline nothing created, is answered as an absent PIPELINE EXECUTION — the innermost resource — instead of as the absent workspace/pipeline the request actually got wrong. Its sibling cancel of the very same resource states the crate rule and resolves workspace -> pipeline -> execution in that order, and the other workspace-scoped pipeline reads (ListPipelines, ListPipelineExecutions) resolve the workspace before anything else. The response status and error shape are identical, but the refusal names the wrong resource, so a caller cannot tell a typo'd workspace from a typo'd execution id.

## Evidence

The rule, from the sibling handler on the same resource — services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/cancel_pipeline_execution.rs:178-196:

    // The resources the path names are resolved OUTSIDE IN, as this service's
    // other handlers resolve the resource a path names before the one inside
    // it, so a request that names none of them reports the outermost absence.
    if !store.workspaces.contains_key(&input.workspace_name) {
        return Err(not_found(
            crate::state::RESOURCE_WORKSPACE,
            &input.workspace_name,
        ));
    }
    ...
        .ok_or_else(|| not_found(crate::state::RESOURCE_PIPELINE, &input.pipeline_name))?;

The same sentence again in services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/list_pipelines.rs:165-177: "The workspace registry is the one place that says which workspaces exist ... Resolved BEFORE the page is cut, so a request that is wrong in both ways reports the resource that is absent." followed by `if !store.workspaces.contains_key(&input.workspace_name) { return Err(not_found(... RESOURCE_WORKSPACE ...` .

What this handler does instead — services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/describe_pipeline_execution.rs:202-224:

    // An execution is identified by its id, and the record names the pipeline it
    // belongs to: an id scoped to another workspace or pipeline names an
    // execution this request does not address, ...
    let execution = store
        .pipeline_executions
        .get(&input.pipeline_execution_id)
        .filter(|execution| {
            execution.workspace_name == input.workspace_name
                && execution.pipeline_name == input.pipeline_name
        })
        .ok_or_else(|| ... RESOURCE_PIPELINE_EXECUTION, &input.pipeline_execution_id ...

There is no `store.workspaces.contains_key(..)` and no `store.pipelines.get(&pipeline_key(..))` anywhere in the file (grep for `workspaces.contains_key` over `src/ops` lists every handler that applies the check; describe_pipeline_execution.rs is absent from it).

Test coverage: tests/all/pipeline_executions.rs:898-911 asserts only `status == 404` and `error_code(&body) == "ResourceNotFoundException"` for the "a pipeline nothing created" row — it pins neither the message nor which resource the refusal names, so resolving outside-in leaves that test passing.

## Steps to Reproduce

1. CreateWorkspace `floor-1`, CreateTask `alpha` in it, CreatePipeline `line-1` in it with one compute node on `alpha`.
2. StartPipelineExecution on `floor-1`/`line-1` and keep the returned `pipelineExecutionId`.
3. Call DescribePipelineExecution with `workspaceName=no-such-workspace`, `pipelineName=line-1` and that execution id: the refusal names the pipeline execution as the absent resource.
4. Call CancelPipelineExecution with the SAME three values: the refusal names the workspace as the absent resource — the two handlers disagree about which resource a request is wrong about.
