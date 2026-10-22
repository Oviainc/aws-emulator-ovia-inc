Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 16fab4a727e8e34e48fe5cd80cb0080f7932bd9f

# Bootstrap the MWAA Serverless (AmazonMWAAServerless) emulator

_Filed: 2026-10-21T23:18:02Z_
_Operation: service-com-amazonaws-mwaaserverless-amazonmwaaserverless | Class: missing_

> _Note: this issue was identified against the code base at commit `16fab4a727e8e34e48fe5cd80cb0080f7932bd9f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 15 operations in this crate is an unmodified scaffold returning HTTP 501 `NotImplemented` and holding no state, so no client workflow works at all. Implement a FIRST, SMALL stateful slice covering the core workflow lifecycle of the primary resource (`workflow`) and its runs. Scope — exactly these 6 operations, in this order:

1. **CreateWorkflow** (`POST /workflows`) — input: `Name` (required, 1..255, `^[a-zA-Z0-9]+[a-zA-Z0-9.\-_]*$`), `DefinitionS3Location` (required: `Bucket`, `ObjectKey`, optional `VersionId`), `RoleArn` (required IAM role ARN), optional `ClientToken` (@idempotencyToken), `Description`, `EncryptionConfiguration`, `LoggingConfiguration`, `EngineVersion`, `NetworkConfiguration`, `Tags`, `TriggerMode`. Must MINT and PERSIST a workflow: generate `WorkflowArn` matching the modeled pattern `arn:aws:airflow-serverless:<region>:<account>:workflow/<name>-<10 alnum>`, a 32-char lowercase-hex-ish `WorkflowVersion` (`^[a-z0-9]{32}$`), `CreatedAt`, `RevisionId`, `WorkflowStatus=READY`, `IsLatestVersion=true`. A second CreateWorkflow with the same `Name` must return `ConflictException` (409); the same `ClientToken`+identical input must return the first result rather than create a duplicate.
2. **GetWorkflow** (`GET /workflows/{WorkflowArn}`) — must read the stored record back and return `WorkflowArn`, `Name`, `Description`, `CreatedAt`, `ModifiedAt`, `WorkflowVersion`, `WorkflowStatus`, `DefinitionS3Location`, `RoleArn`, `EngineVersion`, `LoggingConfiguration`, `NetworkConfiguration`, `TriggerMode` — the exact values supplied at create (plus `CodeSnapshottedAt`). An unknown/deleted ARN must return `ResourceNotFoundException` (404). The optional `workflowVersion` query must select that stored version.
3. **ListWorkflows** (`GET /workflows`) — must return a `WorkflowSummary` per persisted workflow (`WorkflowArn` required, `WorkflowVersion`, `Name`, `Description`, `CreatedAt`, `ModifiedAt`, `WorkflowStatus`, `TriggerMode`), honoring `MaxResults` (default 20) and round-tripping `NextToken` for the remainder (@paginated items=Workflows).
4. **StartWorkflowRun** (`POST /workflows/{WorkflowArn}/runs`) — input `WorkflowArn` (label), optional `ClientToken`, `OverrideParameters` (map<string,document>), `WorkflowVersion`. Must create and persist a run under that workflow: mint `RunId` (`^[a-zA-Z0-9]+[a-zA-Z0-9.\-_]*$`), set `Status` to a modeled `WorkflowRunStatus` (e.g. `QUEUED`/`RUNNING`) and `StartedAt`. Unknown workflow ARN -> `ResourceNotFoundException` (404). Repeating the same `ClientToken` must not create a second run.
5. **GetWorkflowRun** (`GET /workflows/{WorkflowArn}/runs/{RunId}`) — must return the persisted run: `WorkflowArn`, `WorkflowVersion`, `RunId`, `RunType=ON_DEMAND` (for StartWorkflowRun-created runs), the `OverrideParameters` exactly as supplied, and a `RunDetail` (`WorkflowRunDetail`) carrying `RunId`, `RunType`, `CreatedAt`, `StartedOn`, `RunState` (and `CompletedOn`/`Duration`/`ErrorMessage` left null while the run is not complete). Unknown `RunId` -> `ResourceNotFoundException` (404). Optional nicety in the same slice: **ListWorkflowRuns** / **StopWorkflowRun** reusing the same run store (`StopWorkflowRun` setting `Status=STOPPED`), but these may stay stubbed if the slice gets long.
6. **DeleteWorkflow** (`DELETE /workflows/{WorkflowArn}`) — must remove the persisted workflow (and its runs) and echo `WorkflowArn` + `WorkflowVersion` from the record it deleted. A following GetWorkflow must return `ResourceNotFoundException` (404), and ListWorkflows must no longer include it. Deleting an unknown ARN -> `ResourceNotFoundException` (404).

NEVER STUBS: each of these must compute its response from persisted state held in the shared engine (`crate::state::service_state(&*self.engine)`), not from canned constants or echoed input. Minting an ARN/RunId without storing anything, or returning a hardcoded `READY` workflow for any ARN, is a failure of this issue. `ValidationException` (400) must be returned for inputs violating the modeled length/pattern constraints (e.g. a `Name` with a leading `-`, a `WorkflowArn` that does not match the ARN pattern).

## State Model

In `src/state.rs` (the per-service state reached through the shared engine):

- `workflows: HashMap<String /* WorkflowArn */, Workflow>`
- `workflow_names: HashMap<String /* Name */, String /* WorkflowArn */>` — for the CreateWorkflow conflict check
- `Workflow { arn: String, name: String, description: Option<String>, role_arn: String, definition_s3_location: DefinitionS3Location { bucket, object_key, version_id }, code: Option<Code>, encryption_configuration: Option<..>, logging_configuration: Option<..>, engine_version: Option<..>, network_configuration: Option<..>, trigger_mode: Option<String>, status: WorkflowStatus /* READY | DELETING */, created_at: SystemTime, modified_at: SystemTime, code_snapshotted_at: SystemTime, current_version: String /* 32 chars */, revision_id: String, versions: Vec<WorkflowVersionRecord>, tags: BTreeMap<String,String>, runs: BTreeMap<String /* RunId */, WorkflowRun> }`
- `WorkflowVersionRecord { version: String, created_at: SystemTime, is_latest: bool, snapshot: /* the definition + config as of that version */ }`
- `WorkflowRun { run_id: String, workflow_arn: String, workflow_version: String, run_type: RunType /* ON_DEMAND | SCHEDULED */, status: WorkflowRunStatus, override_parameters: BTreeMap<String, Document>, created_at: SystemTime, started_on: SystemTime, completed_on: Option<SystemTime>, modified_at: SystemTime, duration: Option<i32>, error_message: Option<String>, task_instances: Vec<String> }`
- `idempotency: HashMap<String /* ClientToken */, IdempotentResult>` — so repeated CreateWorkflow/StartWorkflowRun with one token replay the first response

Operations outside this slice (UpdateWorkflow, ListWorkflowVersions, ListWorkflowRuns, StopWorkflowRun, ListTaskInstances, GetTaskInstance, TagResource, UntagResource, ListTagsForResource) stay at 501 and will be filed as individual gap issues later — the tag/task-instance stores above are named so they can be layered on without reshaping state.

## Evidence

Spec: `services/service-com-amazonaws-mwaaserverless-amazonmwaaserverless/OPERATIONS.md` (generated from `smithy-model/com-amazonaws-mwaaserverless-amazonmwaaserverless.json`) lists all 15 operations with these inputs/outputs/errors: CreateWorkflow (`POST /workflows`, required Name/DefinitionS3Location/RoleArn, outputs WorkflowArn/CreatedAt/RevisionId/WorkflowStatus/WorkflowVersion/IsLatestVersion, errors incl. ConflictException 409), GetWorkflow (`GET /workflows/{WorkflowArn}`, @readonly, ResourceNotFoundException 404), ListWorkflows (`GET /workflows`, @paginated items=Workflows, MaxResults default 20), StartWorkflowRun (`POST /workflows/{WorkflowArn}/runs`, @idempotent with @idempotencyToken ClientToken, outputs RunId/Status/StartedAt), GetWorkflowRun (`GET /workflows/{WorkflowArn}/runs/{RunId}`, output RunDetail: WorkflowRunDetail), DeleteWorkflow (`DELETE /workflows/{WorkflowArn}`). Shape detail from smithy_query for WorkflowSummary, WorkflowRunSummary, WorkflowRunDetail and DefinitionS3Location (fields named in the State Model above). Implementation: `grep -rn '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-mwaaserverless-amazonmwaaserverless/src/ops/` matches all 15 handler files (create_workflow.rs, get_workflow.rs, list_workflows.rs, start_workflow_run.rs, get_workflow_run.rs, delete_workflow.rs, update_workflow.rs, list_workflow_versions.rs, list_workflow_runs.rs, stop_workflow_run.rs, list_task_instances.rs, get_task_instance.rs, tag_resource.rs, untag_resource.rs, list_tags_for_resource.rs), each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` = HTTP 501. No test asserts any of this behavior (only the generated `tests/state_roundtrip.rs` example).

## Steps to Reproduce

1. Start the emulator and point an `airflow-serverless` client at it (awsJson1_0 / REST routes as modeled; sigv4 signing name `airflow-serverless`).
2. CreateWorkflow with Name="demo-wf", DefinitionS3Location={Bucket:"my-bucket",ObjectKey:"dags/demo.py"}, RoleArn="arn:aws:iam::123456789012:role/mwaa-exec", Description="demo" -> expect 200 with a pattern-valid WorkflowArn, a 32-char WorkflowVersion, CreatedAt, WorkflowStatus=READY, IsLatestVersion=true. Actual: HTTP 501 NotImplemented.
3. GetWorkflow with the returned WorkflowArn -> expect Name/Description/RoleArn/DefinitionS3Location identical to the create input, plus CreatedAt/ModifiedAt/WorkflowStatus. Actual: 501.
4. ListWorkflows (MaxResults=20) -> expect exactly one WorkflowSummary whose WorkflowArn and Name match the created workflow. Actual: 501.
5. CreateWorkflow again with Name="demo-wf" -> expect ConflictException (409). Actual: 501.
6. StartWorkflowRun on the WorkflowArn with OverrideParameters={"env":"test"} -> expect 200 with a RunId, Status in {QUEUED,RUNNING,STARTING} and StartedAt. Actual: 501.
7. GetWorkflowRun with that WorkflowArn+RunId -> expect RunId echoed from state, RunType=ON_DEMAND, OverrideParameters={"env":"test"}, and RunDetail with RunId/CreatedAt/StartedOn/RunState. Actual: 501.
8. GetWorkflowRun with RunId="doesnotexist" -> expect ResourceNotFoundException (404). Actual: 501.
9. DeleteWorkflow on the WorkflowArn -> expect 200 echoing WorkflowArn + WorkflowVersion; then GetWorkflow on the same ARN -> expect ResourceNotFoundException (404), and ListWorkflows -> expect an empty Workflows list. Actual: 501 for all three.
