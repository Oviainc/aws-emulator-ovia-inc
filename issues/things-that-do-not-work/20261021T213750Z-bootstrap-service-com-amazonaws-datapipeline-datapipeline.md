Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 09acb2538adf9fea9bce04c8d04105f42be236df

# Bootstrap the AWS Data Pipeline emulator

_Filed: 2026-10-21T21:37:50Z_
_Operation: service-com-amazonaws-datapipeline-datapipeline | Class: missing_

> _Note: this issue was identified against the code base at commit `09acb2538adf9fea9bce04c8d04105f42be236df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 19 operations in `services/service-com-amazonaws-datapipeline-datapipeline/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no Data Pipeline client can do anything against the emulator. Implement a FIRST, SMALL, stateful slice: the pipeline lifecycle happy path. Protocol is `awsJson1_1`, `POST /` with `X-Amz-Target: DataPipeline.<OperationName>` (README/OPERATIONS.md).

### Happy path to implement (in order)
1. **CreatePipeline** — input `name` (required), `uniqueId` (required), `description`, `tags` (0..10 `Tag{key,value}`). Output `pipelineId` (required): mint a realistic id (e.g. `df-0123456789ABCDEFGHIJ`) and PERSIST the pipeline record. Idempotency: a repeat call with the same `(name, uniqueId)` must return the SAME `pipelineId` rather than creating a second pipeline (Data Pipeline's documented `uniqueId` idempotency contract); a different `name` with an already-used `uniqueId` is `InvalidRequestException`.
2. **PutPipelineDefinition** — input `pipelineId` (required), `pipelineObjects` (required, list of `PipelineObject{id, name, fields: [Field{key, stringValue|refValue}]}`), `parameterObjects`, `parameterValues`. Store the definition under the pipeline (replacing any previous one) and return `errored=false` with empty `validationErrors`/`validationWarnings` for a well-formed definition. Unknown `pipelineId` → `PipelineNotFoundException`; a deleted one → `PipelineDeletedException`.
3. **GetPipelineDefinition** — input `pipelineId` (required), `version`. Must return exactly the `pipelineObjects`, `parameterObjects` and `parameterValues` that were stored by step 2 — read back from state, not echoed from the current request.
4. **DescribePipelines** — input `pipelineIds` (required). Output `pipelineDescriptionList` of `PipelineDescription{pipelineId, name, description, tags, fields}` built from the stored record. `fields` must carry the read-only metadata fields the model documents: `@pipelineState`, `@userId`, `@accountId`, `@creationTime` (state `PENDING` before a definition is put/activated, `SCHEDULED` after ActivatePipeline if you wire it later).
5. **ListPipelines** — input `marker`. Output `pipelineIdList` of `PipelineIdName{id, name}` for every live pipeline plus `hasMoreResults` and `marker`; page off the persisted map (a fixed page size with an opaque marker is fine, but `hasMoreResults=false` and no marker when the last page is returned).
6. **DeletePipeline** — input `pipelineId` (required). Remove the pipeline and its definition from state. Afterwards `DescribePipelines`/`GetPipelineDefinition`/`PutPipelineDefinition` on that id must fail (`PipelineNotFoundException`, or `PipelineDeletedException` if you keep a tombstone) and `ListPipelines` must no longer list it.

No canned or echoed responses: each of these six must compute its response from persisted state, and the `pipelineId` must be a key in a real store, not a random string that is thrown away. Modeled errors must serialize as awsJson1_1 errors with HTTP 400 (`InvalidRequestException`, `PipelineNotFoundException`, `PipelineDeletedException`) / 500 (`InternalServiceError`).

### State Model
In `src/state.rs` (reached via `crate::state::service_state(&*self.engine)`):
- `pipelines: HashMap<String /* pipelineId */, Pipeline>`
- `unique_ids: HashMap<(String /* name */, String /* uniqueId */), String /* pipelineId */>` — for CreatePipeline idempotency
- `deleted: HashSet<String>` — tombstones so a deleted id can answer `PipelineDeletedException`
- `next_pipeline_seq: u64` — id minting
- `Pipeline { pipeline_id: String, name: String, unique_id: String, description: Option<String>, tags: Vec<Tag{key,value}>, state: PipelineState /* PENDING | SCHEDULED | DEACTIVATING | INACTIVE */, created_at: SystemTime, definition: Option<PipelineDefinition> }`
- `PipelineDefinition { pipeline_objects: Vec<PipelineObject{id, name, fields: Vec<Field{key, string_value, ref_value}>}>, parameter_objects: Vec<ParameterObject>, parameter_values: Vec<ParameterValue>, version: String }`

All other operations (ActivatePipeline, DeactivatePipeline, SetStatus, QueryObjects, DescribeObjects, EvaluateExpression, ValidatePipelineDefinition, AddTags/RemoveTags, PollForTask/ReportTaskProgress/ReportTaskRunnerHeartbeat/SetTaskStatus) stay stubbed at 501 for now and will be filed as individual gap issues.

## Evidence

OPERATIONS.md for this crate lists all 19 operations with their members: CreatePipeline (in `name`,`uniqueId`,`description`,`tags`; out required `pipelineId`), PutPipelineDefinition (in `pipelineId`,`pipelineObjects`,...; out `validationErrors`,`validationWarnings`,`errored`), GetPipelineDefinition (out `pipelineObjects`,`parameterObjects`,`parameterValues`), DescribePipelines (out required `pipelineDescriptionList`), ListPipelines (@paginated inputToken=marker/outputToken=marker/items=pipelineIdList, out `pipelineIdList`,`marker`,`hasMoreResults`), DeletePipeline (errors `PipelineNotFoundException`, `InvalidRequestException`). Shape detail from the Smithy model: `PipelineDescription{pipelineId,name,fields,description,tags}` whose `fields` doc says "a list of read-only fields that contain metadata about the pipeline: @userId, @accountId, and @pipelineState"; `PipelineObject{id,name,fields}`; `Field{key,stringValue,refValue}`; `PipelineIdName{id,name}`; `Tag{key,value}`. Current state: `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-datapipeline-datapipeline/src/ops/` matches all 19 handler files, each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` → HTTP 501. There are no tests beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. POST / with X-Amz-Target: DataPipeline.CreatePipeline and body {"name":"demo","uniqueId":"demo-1","description":"bootstrap","tags":[{"key":"env","value":"test"}]} — expect 200 with a `pipelineId` like df-...; today 501 NotImplemented
2. Repeat the identical CreatePipeline call — expect the SAME pipelineId returned (uniqueId idempotency), not a second pipeline
3. POST DataPipeline.PutPipelineDefinition with that pipelineId and pipelineObjects=[{"id":"Default","name":"Default","fields":[{"key":"scheduleType","stringValue":"ondemand"}]}] — expect 200 with errored=false and empty validationErrors
4. POST DataPipeline.GetPipelineDefinition with that pipelineId — expect the stored pipelineObjects returned byte-for-byte equivalent to what was put
5. POST DataPipeline.DescribePipelines with pipelineIds=[that id] — expect one PipelineDescription whose name/description/tags match CreatePipeline and whose fields include @pipelineState, @userId, @accountId
6. POST DataPipeline.ListPipelines with no marker — expect pipelineIdList to contain {id, name:"demo"} and hasMoreResults=false
7. POST DataPipeline.DeletePipeline with that pipelineId — expect 200
8. POST DataPipeline.DescribePipelines and DataPipeline.GetPipelineDefinition with the deleted id — expect PipelineNotFoundException/PipelineDeletedException (HTTP 400), and ListPipelines no longer lists it
