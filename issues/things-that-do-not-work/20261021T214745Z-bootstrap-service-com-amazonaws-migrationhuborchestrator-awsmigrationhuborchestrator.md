Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4

# Bootstrap the MigrationHubOrchestrator emulator

_Filed: 2026-10-21T21:47:45Z_
_Operation: service-com-amazonaws-migrationhuborchestrator-awsmigrationhuborchestrator | Class: stub_

> _Note: this issue was identified against the code base at commit `a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 31 operations in this crate is an unmodified scaffold: each `src/ops/*.rs` carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501), and the crate holds no state. Implement a FIRST, SMALL, stateful slice: the migration-workflow lifecycle, which is the service's primary resource.

Happy path to implement, in order (restJson1, sigv4 name `migrationhub-orchestrator`):

1. **CreateWorkflow** — `POST /migrationworkflow/`. Input: `name` (required), `templateId` (required), `inputParameters` (map<string,StepInput>, required), `description`, `applicationConfigurationId`, `stepTargets`, `tags`. Allocate a fresh `id` matching `^[a-zA-Z0-9-]+$` (len 1..100), build `arn` as `arn:aws:migrationhub-orchestrator:<region>:<account>:workflow/<id>`, persist the record, and return `id`/`arn`/`name`/`description`/`templateId`/`adsApplicationConfigurationId`/`workflowInputs` (echoing the stored `inputParameters`)/`stepTargets`/`status` = `CREATING` (or `NOT_STARTED` once creation settles)/`creationTime` (server clock)/`tags` — all read back out of the stored record, not out of the request.
2. **GetWorkflow** — `GET /migrationworkflow/{id}`. Must return exactly what CreateWorkflow persisted: `id`, `arn`, `name`, `description`, `templateId`, `adsApplicationConfigurationId`, `status`, `statusMessage`, `creationTime`, `lastStartTime`, `lastStopTime`, `lastModifiedTime`, `endTime`, `totalSteps`, `completedSteps`, `workflowInputs`, `tags`. An unknown or already-deleted `id` must be `ResourceNotFoundException` (HTTP 404), never a canned 200.
3. **ListWorkflows** — `GET /migrationworkflows`. Returns `migrationWorkflowSummary` (required) built from the stored workflows, honouring the `templateId`, `status`, `name` and `adsApplicationConfigurationName` query filters, plus `maxResults` (0..100) truncation with an opaque `nextToken` that resumes where the previous page stopped (and is omitted on the last page).
4. **StartWorkflow** — `POST /migrationworkflow/{id}/start`. Mutates the stored record: set `status` to `STARTING`/`IN_PROGRESS`, set `lastStartTime` to now, and return `id`, `arn`, `status`, `statusMessage`, `lastStartTime` from that record. 404 for an unknown id.
5. **StopWorkflow** — `POST /migrationworkflow/{id}/stop`. Sets `status` to `PAUSING`/`PAUSED`, sets `lastStopTime` to now, returns `id`, `arn`, `status`, `statusMessage`, `lastStopTime`. 404 for an unknown id.
6. **DeleteWorkflow** — `DELETE /migrationworkflow/{id}`, success status **202**. Removes the workflow from state and returns `id`, `arn`, `status` = `DELETING`/`DELETED`. A subsequent GetWorkflow/ListWorkflows must no longer see it; a second delete of the same id is `ResourceNotFoundException` (404).

NEVER STUBS: each of these six must compute its response from the persisted state. Hardcoded bodies, pure input echoes, or minting an id without storing the record are failures of this issue. A create followed by a get/list must return what was created, and start/stop/delete must be observable through a later get/list.

## State Model

Per-service state reached via `crate::state::service_state(&*self.engine)` behind its lock:

- `workflows: HashMap<String /* workflow id */, Workflow>`
- `Workflow { id: String, arn: String, name: String, description: Option<String>, template_id: String, ads_application_configuration_id: Option<String>, status: MigrationWorkflowStatusEnum, status_message: Option<String>, workflow_inputs: HashMap<String, StepInput>, step_targets: Vec<String>, tags: HashMap<String, String>, creation_time: DateTime, last_start_time: Option<DateTime>, last_stop_time: Option<DateTime>, last_modified_time: Option<DateTime>, end_time: Option<DateTime>, total_steps: Option<i32>, completed_steps: Option<i32>, workflow_bucket: Option<String> }`
- `workflow_order: Vec<String>` — stable creation order so ListWorkflows pages deterministically.
- `list_tokens`: pagination cursors encoding the index into `workflow_order` (or encode the index/last-seen id directly into the `nextToken` string).

Out of scope for this first slice (stay 501 and are filed separately later): templates (CreateTemplate/GetTemplate/UpdateTemplate/DeleteTemplate/ListTemplates and the template step/step-group reads), workflow steps and step groups (Create/Get/Update/Delete/List + RetryWorkflowStep), UpdateWorkflow, ListPlugins, and the tagging operations (TagResource/UntagResource/ListTagsForResource).

## Evidence

Spec: `services/service-com-amazonaws-migrationhuborchestrator-awsmigrationhuborchestrator/OPERATIONS.md` documents the routes, required inputs, outputs, success statuses and errors cited above — CreateWorkflow (`POST /migrationworkflow/`, 200), GetWorkflow (`GET /migrationworkflow/{id}`, @readonly, ResourceNotFoundException 404), ListWorkflows (`GET /migrationworkflows`, @paginated nextToken/migrationWorkflowSummary, maxResults range 0..100), StartWorkflow (`POST /migrationworkflow/{id}/start`), StopWorkflow (`POST /migrationworkflow/{id}/stop`), DeleteWorkflow (`DELETE /migrationworkflow/{id} -> 202`, @idempotent). Implementation: `grep -l '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-migrationhuborchestrator-awsmigrationhuborchestrator/src/ops/` matches all 31 operation files; e.g. `src/ops/create_workflow.rs:180` is `Err(crate::scaffolded::Scaffolded::not_implemented("CreateWorkflow"))` with no state access. No tests exercise any workflow behaviour.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `migrationhub-orchestrator` client at it.
2. CreateWorkflow with name="demo-wf", templateId="tmpl-1", inputParameters={"Source":{"integerValue":1}} — expect 200 with a generated id/arn, status CREATING|NOT_STARTED and a server creationTime; today: HTTP 501 NotImplemented.
3. GetWorkflow with the returned id — expect 200 echoing the persisted name, templateId, workflowInputs, tags and creationTime.
4. ListWorkflows with maxResults=1 (and again with templateId="tmpl-1") — expect the new workflow in migrationWorkflowSummary, pagination honoured via nextToken.
5. StartWorkflow on the id, then GetWorkflow — expect status advanced to STARTING/IN_PROGRESS and lastStartTime set.
6. StopWorkflow on the id, then GetWorkflow — expect status PAUSING/PAUSED and lastStopTime set.
7. DeleteWorkflow on the id — expect HTTP 202 with status DELETING/DELETED; then GetWorkflow on the same id must return ResourceNotFoundException (404) and ListWorkflows must no longer include it.
8. GetWorkflow on a never-created id (e.g. "does-not-exist") — expect 404 ResourceNotFoundException, not 501 and not a canned 200.
