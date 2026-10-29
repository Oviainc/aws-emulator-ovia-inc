Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 52e42c3071caaf4a5ac680f43132e302f2bbfe9a

# A different StartPipelineExecution reusing a spent clientToken replays the first execution, so the overrides it carries never run

_Filed: 2026-10-29T00:26:49Z_
_Operation: StartPipelineExecution | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `52e42c3071caaf4a5ac680f43132e302f2bbfe9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

StartPipelineExecution keys its token registry on `(workspaceName, pipelineName, clientToken)` and replays the recorded execution id whenever that key is present. The members that make one start different from another — `executionEnvironmentVariableOverrides`, `executionMountOverrides`, `executionPriority` — are not part of the key and are never compared, although `PipelineExecutionRecord` stores all three. A second start on the same pipeline reusing the token with different overrides is therefore answered 202 with the first execution's id while nothing new is queued: the run the caller described never happens. The crate's generic idempotency rule makes a different request under a spent token the modelled `ConflictingOperationException`, which this operation models (409).

## Evidence

Handler — services/.../src/ops/start_pipeline_execution.rs:213-231:
```
        let token_key = input
            .client_token
            .as_deref()
            .map(|token| {
                crate::state::execution_token_key(
                    &input.workspace_name,
                    &input.pipeline_name,
                    token,
                )
            });
        if let Some(started) = token_key
            .as_ref()
            .and_then(|key| store.pipeline_execution_tokens.get(key))
            .filter(|id| store.pipeline_executions.contains_key(*id))
        {
            return Ok(StartPipelineExecutionOutput {
                pipeline_execution_id: started.clone(),
            });
        }
```
The overrides are stored, so they could be compared — start_pipeline_execution.rs:346-350: `execution_priority: input.execution_priority.unwrap_or(...), request_environment_variables: overrides.unwrap_or_default(), request_mount_overrides: input.execution_mount_overrides.clone()`.
The rule skipped — state.rs:9338-9342 (`Some(call) if call.request == request => TokenVerdict::Replay(...)`, `Some(call) => TokenVerdict::Spent(...)`) and create_task.rs:193-198: "a DIFFERENT request reusing it belongs to another call and is the modelled conflict rather than a second task."
The modelled error exists — smithy_query StartPipelineExecution errors include `{"shape": "ConflictingOperationException", "httpError": 409}`.

## Steps to Reproduce

1. CreateWorkspace W, CreateTask and CreatePipeline P in W.
2. StartPipelineExecution on W/P with executionPriority 0 and clientToken T; note pipelineExecutionId E1.
3. StartPipelineExecution on W/P with executionPriority 2 and different executionEnvironmentVariableOverrides, same clientToken T.
4. Observe 202 with pipelineExecutionId E1; ListPipelineExecutions shows one execution carrying the FIRST request's priority and overrides, instead of the modelled ConflictingOperationException.
