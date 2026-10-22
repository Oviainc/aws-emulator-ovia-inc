Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 78a47c5beb190b609d38a363e04d7dff9ff2596d

# Bootstrap the omics emulator

_Filed: 2026-10-21T23:33:05Z_
_Operation: service-com-amazonaws-omics-omics | Class: missing_

> _Note: this issue was identified against the code base at commit `78a47c5beb190b609d38a363e04d7dff9ff2596d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The AWS HealthOmics (`omics`, restJson1, sigv4 name `omics`) crate is a fresh scaffold: all 107 operations under `services/service-com-amazonaws-omics-omics/src/ops/` carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and answer HTTP 501 `NotImplemented`, holding zero state. Implement a FIRST stateful slice around the service's primary resource pair — the private workflow and the run started from it. Everything else stays 501 and will be filed as individual gap issues later.

### Happy path to implement (in order)

1. **CreateWorkflow** — `POST /workflow -> 201`. Inputs: `name`, `description`, `engine` (CWL|NEXTFLOW|WDL|WDL_LENIENT), `definitionZip` (blob) or `definitionUri`, `main`, `parameterTemplate` (map<string,WorkflowParameter>), `storageCapacity`, `storageType` (DYNAMIC|STATIC), `accelerators`, `tags`, `requestId` (required, @idempotencyToken). Must mint and PERSIST a new workflow: numeric `id` (pattern `^[0-9]+$`), `arn` (`arn:aws:omics:<region>:<account>:workflow/<id>`), `uuid` (UUID v4 form), `status` (start `CREATING` or `ACTIVE`; a subsequent GetWorkflow must report `ACTIVE`), `tags` echoed from storage, plus `creationTime`. `requestId` must be honored for idempotency: the same `requestId` returns the SAME workflow id rather than creating a second one.
2. **GetWorkflow** — `GET /workflow/{id} -> 200`. Must return the stored record, not a canned body: `id`, `arn`, `uuid`, `status`, `type` (`PRIVATE` for workflows created via CreateWorkflow), `name`, `description`, `engine`, `main`, `definition`, `digest`, `parameterTemplate`, `storageCapacity`, `storageType`, `accelerators`, `creationTime`, `tags`. Unknown `id` -> `ResourceNotFoundException` (HTTP 404), not 501 and not an empty 200.
3. **StartRun** — `POST /run -> 201`. Inputs: `workflowId`, `workflowType` (PRIVATE|READY2RUN), `roleArn` (required), `name`, `priority`, `parameters` (document), `storageCapacity`, `outputUri` (required), `logLevel`, `tags`, `requestId` (required, @idempotencyToken), `retentionMode`, `storageType`, `runGroupId`, `cacheId`/`cacheBehavior`. Must resolve `workflowId` against stored workflows (unknown -> `ResourceNotFoundException` 404) and PERSIST a run with a fresh numeric `id`, `arn`, `uuid`, initial `status` (`PENDING`), `creationTime`, and all echoed-back inputs copied into the stored record. Output: `arn`, `id`, `status`, `uuid`, `tags`, `runOutputUri`. `requestId` idempotency: repeating it returns the same run `id`.
4. **GetRun** — `GET /run/{id} -> 200`. Must read the persisted run and return the values StartRun stored: `id`, `arn`, `uuid`, `status`, `workflowId`, `workflowType`, `roleArn`, `name`, `priority`, `parameters`, `storageCapacity`, `storageType`, `outputUri`, `runOutputUri`, `logLevel`, `retentionMode`, `tags`, `creationTime`, `startTime`/`stopTime` as the status advances. A simple deterministic status progression (PENDING -> STARTING -> RUNNING -> COMPLETED, advanced on read-after-elapsed-time or on each poll) is acceptable, but `status` must come from stored state, never be hardcoded. Unknown `id` -> `ResourceNotFoundException` 404.
5. **ListRuns** — `GET /run -> 200`, @paginated(inputToken=`startingToken`, outputToken=`nextToken`, items=`items`). Must enumerate the persisted runs as `items` (list<RunListItem>: `arn`, `id`, `status`, `workflowId`, `name`, `priority`, `storageCapacity`, `creationTime`, `startTime`, `stopTime`, `storageType`). Must honor the `name`, `status`, `runGroupId` query filters and the `maxResults`/`startingToken` bounds, returning `nextToken` only when a further page exists. A run deleted by DeleteRun must no longer appear.
6. **DeleteRun** and **DeleteWorkflow** — `DELETE /run/{id} -> 202` and `DELETE /workflow/{id} -> 202`, both @idempotent, empty output. Must remove the record from state so a following GetRun/GetWorkflow answers `ResourceNotFoundException` 404 and ListRuns/ListWorkflows no longer include it. Deleting a workflow that still has non-terminal runs should answer `ConflictException` (HTTP 409).

Every one of these must compute its response from persisted state. Hardcoded bodies, echoing the request back without storing it, and minting an id without storing the resource are all failures of this issue.

## State Model

In `src/state.rs` (`ComAmazonawsOmicsOmicsState`, reached through `crate::state::service_state(&*self.engine)`):

- `next_workflow_id: u64`, `next_run_id: u64` — monotonic counters rendered as the decimal id strings the model's `^[0-9]+$` patterns require.
- `workflows: HashMap<String /*id*/, Workflow>` where `Workflow { id, arn, uuid, name, description, engine, main, definition, definition_zip_digest, parameter_template: HashMap<String, WorkflowParameter>, storage_capacity, storage_type, accelerators, type_: "PRIVATE", status, status_message, creation_time, tags: HashMap<String,String> }`.
- `runs: HashMap<String /*id*/, Run>` where `Run { id, arn, uuid, workflow_id, workflow_type, role_arn, name, run_group_id, cache_id, cache_behavior, priority, parameters: Document, storage_capacity, storage_type, output_uri, run_output_uri, log_level, retention_mode, status, status_message, failure_reason, creation_time, start_time, stop_time, tags: HashMap<String,String> }`.
- `idempotency: HashMap<(&'static str /*op*/, String /*requestId*/), String /*resource id*/>` so a replayed `requestId` on CreateWorkflow/StartRun returns the original resource instead of creating a duplicate.
- Insertion order for listing: keep a `workflow_order: Vec<String>` / `run_order: Vec<String>` (or sort by `creation_time` then id) so pagination with `startingToken`/`maxResults` is stable across calls.

Done when a client script can run CreateWorkflow -> GetWorkflow -> StartRun -> GetRun -> ListRuns -> DeleteRun -> DeleteWorkflow end to end with every field round-tripping from state and the 404/409 cases modeled.

## Evidence

1) `services/service-com-amazonaws-omics-omics/OPERATIONS.md` header: "107 operations from `smithy-model/com-amazonaws-omics-omics.json`", protocol `restJson1`, SDK id `Omics`, sigv4 signing name `omics`.
2) Routes, members and errors quoted above are from that file: CreateWorkflow (line 797, `POST /workflow -> 201`, output `arn`/`id`/`status`/`tags`/`uuid`), GetWorkflow (line 804, `GET /workflow/{id} -> 200`, @readonly), DeleteWorkflow (line 819, `DELETE /workflow/{id} -> 202`, @idempotent), ListWorkflows (line 827, @paginated inputToken=startingToken/outputToken=nextToken/items=items), StartRun (line 418, `POST /run -> 201`, required `roleArn`/`outputUri`/`requestId`), GetRun (line 425, `GET /run/{id} -> 200`), DeleteRun (line 433, `DELETE /run/{id} -> 202`), ListRuns (line 441, @paginated with `name`/`status`/`runGroupId`/`maxResults`/`startingToken` query params). All eight declare `ResourceNotFoundException (HTTP 404)` and `ConflictException (HTTP 409)` among their errors.
3) All-stub census: `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-omics-omics/src/ops/` matches every one of the 107 operation files, including `create_workflow.rs`, `get_workflow.rs`, `start_run.rs`, `get_run.rs`, `list_runs.rs`, `delete_run.rs`, `delete_workflow.rs`. Each returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` -> HTTP 501. No tests exist beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `omics` client at it.
2. `aws omics create-workflow --name demo --engine WDL --definition-uri s3://bucket/wf.zip --main main.wdl --request-id req-1` — expect HTTP 201 with a numeric `id`, an `arn`, a `uuid` and a `status`; today returns HTTP 501 NotImplemented.
3. `aws omics get-workflow --id <id>` — expect 200 echoing the stored `name`, `engine`, `main`, `type: PRIVATE`, `status: ACTIVE`, `creationTime`, `tags`.
4. Repeat the create call with `--request-id req-1` — expect the SAME workflow `id` (idempotency), not a second workflow.
5. `aws omics start-run --workflow-id <id> --role-arn arn:aws:iam::123456789012:role/omics --output-uri s3://bucket/out/ --name run1 --request-id run-req-1` — expect 201 with a run `id`, `arn`, `uuid` and `status: PENDING`.
6. `aws omics get-run --id <runId>` — expect 200 with `workflowId` equal to the created workflow, the `roleArn`, `outputUri`, `name` and `priority` exactly as submitted, and a `status` drawn from state.
7. `aws omics list-runs` — expect the run present in `items`; then `aws omics list-runs --max-results 1` with two runs created and expect exactly one item plus a `nextToken` that pages to the second.
8. `aws omics start-run --workflow-id 999999 ...` — expect `ResourceNotFoundException` HTTP 404.
9. `aws omics delete-run --id <runId>` — expect 202; then `get-run --id <runId>` must return `ResourceNotFoundException` 404 and `list-runs` must omit it.
10. `aws omics delete-workflow --id <id>` — expect 202; then `get-workflow --id <id>` must return `ResourceNotFoundException` 404 and `list-workflows` must omit it.
