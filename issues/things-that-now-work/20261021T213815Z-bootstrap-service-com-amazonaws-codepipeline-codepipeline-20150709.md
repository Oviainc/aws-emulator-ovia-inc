Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 09acb2538adf9fea9bce04c8d04105f42be236df

# Bootstrap the CodePipeline emulator

_Filed: 2026-10-21T21:38:15Z_
_Operation: service-com-amazonaws-codepipeline-codepipeline-20150709 | Class: missing_

> _Note: this issue was identified against the code base at commit `09acb2538adf9fea9bce04c8d04105f42be236df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 44 operations in `services/service-com-amazonaws-codepipeline-codepipeline-20150709/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client can do anything with this service. Implement a FIRST, SMALL slice: the pipeline-definition lifecycle plus one execution, served entirely from persisted state.

Wire protocol (OPERATIONS.md): `awsJson1_1`, `POST /` with `X-Amz-Target: CodePipeline_20150709.<OperationName>`, sigv4 signing name `codepipeline`.

### Happy path to implement (in order)

1. **CreatePipeline** — input `pipeline` (PipelineDeclaration: required `name`, `roleArn`, `stages`; optional `artifactStore`/`artifactStores`, `version`, `pipelineType`, `executionMode`, `variables`, `triggers`) and optional `tags`. Must store the declaration under its `name`, assign `version = 1`, record `created`/`updated` timestamps and a pipeline ARN (`arn:aws:codepipeline:<region>:<account>:<name>`), and return the stored `pipeline` (with `version` populated) plus `tags`. Re-creating an existing name must fail with `PipelineNameInUseException`; a declaration with no `stages` or a missing `roleArn` must fail with `InvalidStructureException`/`ValidationException` rather than succeed.
2. **GetPipeline** — input `name` (required) and optional `version`. Must return the `pipeline` declaration exactly as stored (same stages/actions the caller sent, not an echo of the current request) plus `metadata` {`pipelineArn`, `created`, `updated`}. Unknown name -> `PipelineNotFoundException`; a `version` that was never created -> `PipelineVersionNotFoundException`.
3. **ListPipelines** — input `nextToken`, `maxResults` (1..1000). Must return one `PipelineSummary` {`name`, `version`, `pipelineType`, `executionMode`, `created`, `updated`} per stored pipeline, honour `maxResults` and return/accept an opaque `nextToken` (invalid token -> `InvalidNextTokenException`).
4. **StartPipelineExecution** — input `name` (required), optional `variables`, `clientRequestToken` (@idempotencyToken), `sourceRevisions`. Must create and persist an execution record with a freshly generated UUID matching `^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`, status `InProgress`, `startTime`/`lastUpdateTime`, trigger type `StartPipelineExecution`, and return that `pipelineExecutionId`. Unknown pipeline -> `PipelineNotFoundException`. Repeating the same `clientRequestToken` must return the same execution id, not a new one.
5. **GetPipelineExecution** — input `pipelineName` + `pipelineExecutionId` (both required). Must look the execution up in state and return `pipelineExecution` {`pipelineName`, `pipelineVersion`, `pipelineExecutionId`, `status`, `artifactRevisions`/`variables` as stored}. Unknown id -> `PipelineExecutionNotFoundException`; unknown pipeline -> `PipelineNotFoundException`. Also make **ListPipelineExecutions** return the stored `PipelineExecutionSummary` list (newest first) for the pipeline, honouring `maxResults`/`nextToken`.
6. **DeletePipeline** — input `name` (required). Must remove the pipeline (and its executions) from state; a subsequent GetPipeline must fail with `PipelineNotFoundException` and ListPipelines must no longer include it.

No canned, hardcoded or echoed responses: each of these must compute its response from the shared store reached via `crate::state::service_state(&*self.engine)` (see README.md "State"). A random execution id that is not stored, or a GetPipeline that re-serialises the request body, counts as a failure of this issue.

## State Model

In `src/state.rs`, on the service state struct:

- `pipelines: BTreeMap<String /* pipeline name */, Pipeline>`
- `Pipeline { declaration: PipelineDeclaration /* name, roleArn, stages, artifactStore(s), pipelineType, executionMode, variables, triggers */, version: i32, arn: String, created: SystemTime, updated: SystemTime, tags: Vec<Tag>, versions: BTreeMap<i32, PipelineDeclaration> }`
- `executions: BTreeMap<String /* pipelineExecutionId (uuid) */, PipelineExecutionRecord>`
- `PipelineExecutionRecord { pipeline_execution_id: String, pipeline_name: String, pipeline_version: i32, status: PipelineExecutionStatus /* InProgress | Succeeded | Stopped | Failed */, status_summary: Option<String>, start_time: SystemTime, last_update_time: SystemTime, trigger: ExecutionTrigger, execution_mode: ExecutionMode, variables: Vec<ResolvedPipelineVariable>, source_revisions: Vec<SourceRevision> }`
- `executions_by_pipeline: BTreeMap<String /* pipeline name */, Vec<String /* execution id, newest last */>>`
- `next_execution_token: ` opaque pagination cursors derived from the ordered keys above (index- or key-based), so `nextToken` round-trips deterministically.
- `idempotency: BTreeMap<(String /* pipeline name */, String /* clientRequestToken */), String /* execution id */>`

Everything outside this slice (jobs/PollForJobs, PutJobSuccessResult, approvals, webhooks, custom action types, stage transitions, rules, tagging, GetPipelineState details, retry/rollback) stays 501 and will be filed as individual gap issues later.

## Evidence

OPERATIONS.md for this crate documents all 44 operations, the awsJson1_1 protocol and `X-Amz-Target: CodePipeline_20150709.<Op>` routing, and the exact input/output members and errors quoted above (CreatePipeline L45-49, DeletePipeline L57-61, GetPipeline L99-103, GetPipelineExecution L105-109, ListPipelines L151-156, ListPipelineExecutions L144-149, StartPipelineExecution L263-267). `smithy_query shape=PipelineDeclaration,PipelineSummary,PipelineExecutionSummary,PipelineMetadata` confirms the stored/returned fields (required name/roleArn/stages; version starting at 1 and incremented on update; metadata pipelineArn/created/updated). `grep '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 44 handler files, including create_pipeline.rs, get_pipeline.rs, list_pipelines.rs, start_pipeline_execution.rs, get_pipeline_execution.rs and delete_pipeline.rs — i.e. zero implemented behaviour and zero state today. No tests exist beyond the generated `tests/state_roundtrip.rs` example.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI client at it with endpoint override and region/credentials set.
2. aws codepipeline create-pipeline --cli-input-json '{"pipeline":{"name":"demo","roleArn":"arn:aws:iam::123456789012:role/svc","artifactStore":{"type":"S3","location":"my-bucket"},"stages":[{"name":"Source","actions":[{"name":"Src","actionTypeId":{"category":"Source","owner":"AWS","provider":"S3","version":"1"},"configuration":{"S3Bucket":"my-bucket","S3ObjectKey":"app.zip"},"outputArtifacts":[{"name":"SourceOut"}]}]},{"name":"Build","actions":[{"name":"Bld","actionTypeId":{"category":"Build","owner":"AWS","provider":"CodeBuild","version":"1"},"configuration":{"ProjectName":"p"},"inputArtifacts":[{"name":"SourceOut"}]}]}]}}' — expect 200 with pipeline.version == 1 (currently HTTP 501 NotImplemented).
3. aws codepipeline get-pipeline --name demo — expect the two stages and the roleArn exactly as created, plus metadata.pipelineArn/created/updated.
4. aws codepipeline list-pipelines — expect a summary for 'demo' with version 1 and created/updated timestamps; repeat with --max-results 1 and follow nextToken after creating a second pipeline.
5. aws codepipeline start-pipeline-execution --name demo — expect a UUID pipelineExecutionId; re-run with the same --client-request-token and expect the SAME id.
6. aws codepipeline get-pipeline-execution --pipeline-name demo --pipeline-execution-id <id> — expect status InProgress and pipelineName/pipelineVersion matching the created pipeline; then aws codepipeline list-pipeline-executions --pipeline-name demo shows that execution.
7. aws codepipeline get-pipeline-execution --pipeline-name demo --pipeline-execution-id 00000000-0000-0000-0000-000000000000 — expect PipelineExecutionNotFoundException (HTTP 400).
8. aws codepipeline delete-pipeline --name demo, then aws codepipeline get-pipeline --name demo — expect PipelineNotFoundException and an empty list-pipelines result.
9. aws codepipeline create-pipeline with the same name twice — expect PipelineNameInUseException on the second call.
