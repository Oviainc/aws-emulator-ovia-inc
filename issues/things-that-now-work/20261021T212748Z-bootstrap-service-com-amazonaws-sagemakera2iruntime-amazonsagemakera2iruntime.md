Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 57742dc737335ed8f07f6b76d23b07c0a9c73918

# Bootstrap the SageMaker A2I Runtime emulator

_Filed: 2026-10-21T21:27:48Z_
_Operation: service-com-amazonaws-sagemakera2iruntime-amazonsagemakera2iruntime | Class: stub_

> _Note: this issue was identified against the code base at commit `57742dc737335ed8f07f6b76d23b07c0a9c73918`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 5 operations in this crate is an unmodified scaffold answering HTTP 501 `NotImplemented` and holding zero state, so no client workflow works at all. Implement a first stateful slice covering the human-loop lifecycle, in this order:

1. **StartHumanLoop** (`POST /human-loops`) — input `HumanLoopName` (required, len 1..63, pattern `^[a-z0-9](-*[a-z0-9])*$`), `FlowDefinitionArn` (required, must match `^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:flow-definition/`), `HumanLoopInput` (required, `{InputContent}` string up to 3145728 bytes), `DataAttributes` (optional `{ContentClassifiers}`). Must create and PERSIST a new human loop record keyed by name, set `CreationTime` to now, `HumanLoopStatus = InProgress`, mint `HumanLoopArn` as `arn:aws:sagemaker:<region>:<account>:human-loop/<HumanLoopName>`, and return that ARN in the output. Re-starting an existing, non-deleted name must return `ConflictException` (HTTP 409), not a second success.
2. **DescribeHumanLoop** (`GET /human-loops/{HumanLoopName}`) — must look the record up in state and return the stored `HumanLoopName`, `HumanLoopArn`, `FlowDefinitionArn`, `CreationTime`, `HumanLoopStatus` (all `@required` in the model), plus `FailureReason`/`FailureCode` when failed and `HumanLoopOutput {OutputS3Uri}` once the loop has reached a terminal state. Unknown name -> `ResourceNotFoundException` (HTTP 404).
3. **ListHumanLoops** (`GET /human-loops`) — must enumerate persisted loops, FILTERED by the required `FlowDefinitionArn` query param, and honor `CreationTimeAfter`/`CreationTimeBefore`, `SortOrder` (Ascending|Descending on CreationTime, default Descending as in real A2I), `MaxResults` (1..100) and `NextToken`. Output `HumanLoopSummaries` is a list of `HumanLoopSummary{HumanLoopName, HumanLoopStatus, CreationTime, FailureReason, FlowDefinitionArn}` built from the stored records, with `NextToken` returned only when more pages remain and resolvable by a follow-up call.
4. **StopHumanLoop** (`POST /human-loops/stop`) — input `HumanLoopName`. Must mutate the stored record's status to `Stopped` (A2I goes `Stopping` -> `Stopped`; settling directly on `Stopped` is acceptable for the first slice) so a subsequent DescribeHumanLoop observes it. Unknown name -> `ResourceNotFoundException` (404). Empty output body with HTTP 200.
5. **DeleteHumanLoop** (`DELETE /human-loops/{HumanLoopName}`) — must remove the record from state so a following DescribeHumanLoop returns `ResourceNotFoundException` (404) and ListHumanLoops no longer includes it. Unknown name -> 404.

Every response must be COMPUTED FROM PERSISTED STATE reached via `crate::state::service_state(&*self.engine)` — no canned bodies, no echoing the request back, no minting an ARN without storing the loop. Input constraints from the model (name length/pattern, ARN patterns, `MaxResults` range) must be rejected with `ValidationException` (HTTP 400). Protocol is `restJson1`; errors must serialize with the modeled HTTP statuses (400/404/409/429/500).

## State Model

In `src/state.rs`, add to the service state struct:

- `human_loops: BTreeMap<String /* HumanLoopName */, HumanLoop>` — insertion/lookup/removal keyed by name, giving StartHumanLoop its conflict check and Delete/Describe their 404s.
- `struct HumanLoop { name: String, arn: String, flow_definition_arn: String, input_content: String, content_classifiers: Vec<String>, status: HumanLoopStatus /* InProgress | Stopping | Stopped | Completed | Failed */, creation_time: SystemTime, failure_reason: Option<String>, failure_code: Option<String>, output_s3_uri: Option<String> }`
- Pagination for ListHumanLoops: a deterministic, opaque `NextToken` encoding the sort key already emitted (e.g. `creation_time` + `name` of the last returned summary), so pages are stable without extra server state.

The remaining model surface outside this slice stays at 501 and will be filed as individual gap issues later.

## Evidence

`services/service-com-amazonaws-sagemakera2iruntime-amazonsagemakera2iruntime/OPERATIONS.md` lists exactly 5 operations (StartHumanLoop, DescribeHumanLoop, ListHumanLoops, StopHumanLoop, DeleteHumanLoop) with the routes, required members, constraints and modeled errors quoted above; `smithy_query shape=HumanLoopInput,HumanLoopOutput,HumanLoopSummary,HumanLoopDataAttributes` supplies the nested member names (`InputContent`, `OutputS3Uri`, `ContentClassifiers`, and the summary fields). `grep -rn 'SCAFFOLDED, NOT IMPLEMENTED' src/ops/` matches all 5 handler files (`start_human_loop.rs`, `describe_human_loop.rs`, `list_human_loops.rs`, `stop_human_loop.rs`, `delete_human_loop.rs`), each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` -> HTTP 501. No tests exercise any human-loop behavior.

## Steps to Reproduce

1. POST /human-loops with {"HumanLoopName":"demo-loop-1","FlowDefinitionArn":"arn:aws:sagemaker:us-east-1:123456789012:flow-definition/demo-flow","HumanLoopInput":{"InputContent":"{\"k\":\"v\"}"}} — expect HTTP 200 with HumanLoopArn ending in `human-loop/demo-loop-1`; actual: HTTP 501 NotImplemented.
2. GET /human-loops/demo-loop-1 — expect HTTP 200 echoing the stored HumanLoopName, HumanLoopArn, FlowDefinitionArn, a CreationTime and HumanLoopStatus=InProgress; actual: HTTP 501.
3. POST /human-loops again with the same HumanLoopName — expect HTTP 409 ConflictException; actual: HTTP 501.
4. GET /human-loops?FlowDefinitionArn=arn:aws:sagemaker:us-east-1:123456789012:flow-definition/demo-flow&MaxResults=1 — expect HumanLoopSummaries containing demo-loop-1 and a NextToken only if more loops exist; actual: HTTP 501.
5. POST /human-loops/stop with {"HumanLoopName":"demo-loop-1"} — expect HTTP 200, then GET /human-loops/demo-loop-1 shows HumanLoopStatus=Stopped; actual: HTTP 501.
6. DELETE /human-loops/demo-loop-1 — expect HTTP 200, then GET /human-loops/demo-loop-1 returns HTTP 404 ResourceNotFoundException and ListHumanLoops no longer lists it; actual: HTTP 501.
7. GET /human-loops/does-not-exist and DELETE /human-loops/does-not-exist — expect HTTP 404 ResourceNotFoundException; actual: HTTP 501.
8. POST /human-loops with HumanLoopName="Bad_Name" (violates pattern ^[a-z0-9](-*[a-z0-9])*$) — expect HTTP 400 ValidationException; actual: HTTP 501.
