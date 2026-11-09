Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__stepfunctions__xray.xray-segment-tree-marks-map-and-parallel-cancellation-and-a-stopped-executions-fault lambda__stepfunctions__xray.xray-segment-tree-reflects-choice-path-lambda-status-and-retry-attempts
Issue-Key: lambda__stepfunctions__xray.xray-segment-tree-marks-map-and-parallel-cancellation-and-a-stopped-executions-fault|_await_full_tree#13|TimeoutError|raise TimeoutError(
Issue-Key: lambda__stepfunctions__xray.xray-segment-tree-reflects-choice-path-lambda-status-and-retry-attempts|_await_full_tree#12|TimeoutError|raise TimeoutError(
Issue-Commit: fb003e0a3fdd2a31392ea928ed9540deb9fe4c34

# lambda__stepfunctions__xray: a traced state-machine execution ingests no X-Ray segments at all — the root segment is absent and no state/Lambda subsegment tree exists (1 cause)

> _Note: this issue was identified against the code base at commit `fb003e0a3fdd2a31392ea928ed9540deb9fe4c34`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetTraces

## Description
One net-new root cause behind 2 failing checks.

Cause 1 of 1: executions of state machines created with tracingConfiguration.enabled=true publish nothing to X-Ray — the execution's trace id resolves to no root segment and no subsegments, so the per-state segment tree (Task/Choice/Parallel branch/Map iteration subsegments, their Lambda child subsegments, and their fault/error/cause marks) does not exist.

AWS: with tracing enabled (Smithy sfn: TracingConfiguration.enabled, boolean, default false, "When set to true, X-Ray tracing is enabled"), Step Functions publishes an execution's trace under the Root id reported by DescribeExecution's traceHeader. BatchGetTraces for that id (Smithy xray: BatchGetTraces POST /Traces -> Traces/UnprocessedTraceIds) returns a root segment with origin AWS::StepFunctions::StateMachine carrying one subsegment per executed state, named after the state; a Parallel state carries one subsegment per branch ("Branch 0", "Branch 1") each containing its task subsegments; a state that invoked Lambda carries a Lambda subsegment per attempt, so a retried state shows as many as it attempted; a state that raised carries fault=true and a canceled sibling carries error=true with cause.message "Canceled". The particular trace id, state machine and function names of a run are per-run values; the requirement is that a traced execution's tree is retrievable and mirrors the states that ran.

Emulator: for both checks the polling helper reports an entirely empty ingestion. Parallel/Map/Stop check: `TimeoutError: trace '1-<8 hex>-<24 hex>' had not ingested all of {'RunParallel': {'Branch 0': 'FastBoom', 'Branch 1': 'SlowOk'}} when the run budget ended; last seen states [] among segment origins []; not graded`. Choice/retry check: `TimeoutError: trace '1-<8 hex>-<24 hex>' had not ingested all of {'Route': 0, 'InvokeOk': 1, 'InvokeMissing': 1, 'InvokeRetry': 2, 'InvokeBoom': 1, 'Recovered': 0} when the run budget ended; incomplete ['root segment absent']; last seen states [] among segment origins []; Lambda subsegments per state {}; not graded`. The executions themselves reach their expected statuses (FAILED/SUCCEEDED), so only the X-Ray publication is missing: origins [], root segment absent, zero Lambda subsegments.

Why: a client relying on AWS's behavior gets no execution tracing from Step Functions. It cannot see which Choice branch was taken, how many times a Task was retried, which Parallel branch or Map iteration was canceled by a sibling's failure, or that StopExecution faulted the root — exactly the signals X-Ray exists to provide. Because the control plane reports tracing as enabled and no error is raised, the loss is silent.

Fix: the trace-publication path of the sfn service crate's execution runtime and its X-Ray integration — emit the root segment (origin AWS::StepFunctions::StateMachine) plus one subsegment per executed state (branch/iteration subsegments for Parallel/Map, one Lambda subsegment per invocation attempt) with the fault/error/cause marks the state's outcome implies, into X-Ray's segment store so BatchGetTraces and GetTraceSummaries resolve them.

Traces: lambda__stepfunctions__xray.xray-segment-tree-marks-map-and-parallel-cancellation-and-a-stopped-executions-fault; lambda__stepfunctions__xray.xray-segment-tree-reflects-choice-path-lambda-status-and-retry-attempts. Related open issue 20261106T214711Z-stepfunctions-xray-a-tracingconfiguration-enabled-state-machine-publishes.md records the same publication gap under the stepfunctions__xray heading and does not carry these checks' replay keys.

## Evidence
Expected (AWS): BatchGetTraces on the Root id of a tracing-enabled execution returns a root segment with origin AWS::StepFunctions::StateMachine whose subsegment tree names the executed states — for a Parallel state, RunParallel -> {"Branch 0" -> FastBoom, "Branch 1" -> SlowOk} with the raising task fault=true and the canceled sibling error=true / cause.message "Canceled"; for the Choice/retry machine, subsegments for Route, InvokeOk, InvokeMissing, InvokeRetry, InvokeBoom, Recovered with Lambda subsegment counts 1/1/2/1 for the invoking states.

Actual (emulator):
- trace '1-6af1c56c-6081aeabb9ec7e57efc1eb65' had not ingested all of {'RunParallel': {'Branch 0': 'FastBoom', 'Branch 1': 'SlowOk'}}; last seen states [] among segment origins [].
- trace '1-6af1c4b9-6603664678df80196eddaee1' had not ingested all of {'Route': 0, 'InvokeOk': 1, 'InvokeMissing': 1, 'InvokeRetry': 2, 'InvokeBoom': 1, 'Recovered': 0}; incomplete ['root segment absent']; last seen states [] among segment origins []; Lambda subsegments per state {}.

## Steps to Reproduce
1. Create a Lambda function whose handler fails fast on one input, sleeps on another, and succeeds otherwise.
2. Create a role trusted by states.amazonaws.com allowing lambda:InvokeFunction and xray:PutTraceSegments/PutTelemetryRecords/GetSamplingRules/GetSamplingTargets.
3. CreateStateMachine with tracingConfiguration {enabled: true} for (a) a Parallel definition with a fast-failing branch and a slow branch, and (b) a Choice + Retry definition invoking the function.
4. StartExecution each and wait for the terminal status; read the Root trace id from DescribeExecution's traceHeader.
5. Poll BatchGetTraces for that Root id.
6. Expected: a root segment with origin AWS::StepFunctions::StateMachine and the per-state subsegment tree described above. Observed: no segments at all (segment origins [], 'root segment absent', no Lambda subsegments).
