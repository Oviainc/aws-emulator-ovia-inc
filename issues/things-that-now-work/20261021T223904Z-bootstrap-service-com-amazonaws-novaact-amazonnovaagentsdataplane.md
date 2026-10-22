Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 543596051071af06ccfbb58d47ce453a1db8e676

# Bootstrap the Nova Act (AmazonNovaAgentsDataPlane) emulator

_Filed: 2026-10-21T22:39:04Z_
_Operation: service-com-amazonaws-novaact-amazonnovaagentsdataplane | Class: stub_

> _Note: this issue was identified against the code base at commit `543596051071af06ccfbb58d47ce453a1db8e676`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 16 operations in `src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client can do anything with this service. Implement a FIRST, SMALL stateful slice: the create -> use -> read -> delete lifecycle of the primary resource chain (workflow definition -> workflow run -> session -> act). Protocol is `restJson1`, sigv4 name `nova-act`.

Happy path to implement, in order (fields from OPERATIONS.md):

1. **CreateWorkflowDefinition** — `PUT /workflow-definitions` -> 201. Input: `name` (required, 1..40, `^[a-zA-Z0-9_-]{1,40}$`), `description` (1..4000), `exportConfig` (WorkflowExportConfig), `clientToken` (@idempotencyToken). Output: `status` = `ACTIVE`. Must INSERT a definition record (name, arn, createdAt=now, description, exportConfig, status) keyed by name; a second create of the same name with a different body is a `ConflictException` (409), while the same `clientToken` replays the first result (@idempotent).
2. **GetWorkflowDefinition** — `GET /workflow-definitions/{workflowDefinitionName}` -> 200. Output must be read back from the stored record: `name`, `arn` (must match `^arn:aws:nova-act:<region>:<account>:workflow-definition/<name>$`), `createdAt`, `description`, `exportConfig`, `status`. Unknown name -> `ResourceNotFoundException` (404).
3. **CreateWorkflowRun** — `PUT /workflow-definitions/{workflowDefinitionName}/workflow-runs` -> 201. Input: `modelId` (required, 1..100), `clientInfo` (ClientInfo, required), `logGroupName`, `clientToken`. Output: `workflowRunId` — a freshly generated lowercase UUID matching `^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$` — plus `status` = `RUNNING`. The run must be STORED under its parent definition (generating a UUID without storing it is a failure). Parent definition absent -> `ResourceNotFoundException` (404).
4. **GetWorkflowRun** — `GET /workflow-definitions/{workflowDefinitionName}/workflow-runs/{workflowRunId}` -> 200. Output computed from the stored run: `workflowRunArn` (`.../workflow-definition/<name>/workflow-run/<runId>`), `workflowRunId`, `status`, `startedAt` (the creation time), `endedAt` (only once terminal), `modelId` (exactly what CreateWorkflowRun stored), `logGroupName`. Unknown definition or run id -> `ResourceNotFoundException` (404).
5. **CreateSession** — `PUT /workflow-definitions/{workflowDefinitionName}/workflow-runs/{workflowRunId}/sessions` -> 201. Output: `sessionId` (generated UUID), stored under the run; `clientToken` replay returns the same `sessionId`. Unknown definition/run -> 404.
6. **CreateAct** — `PUT /workflow-definitions/{name}/workflow-runs/{runId}/sessions/{sessionId}/acts` -> 201. Input: `task` (required, 1..10000), `toolSpecs` (list<ToolSpec>, max 100), `clientToken`. Output: `actId` (generated UUID) and `status` = `RUNNING`; the act (task, toolSpecs, status, createdAt) must be stored under the session so later reads/updates see it. Unknown definition/run/session -> 404.
7. **DeleteWorkflowDefinition** — `DELETE /workflow-definitions/{workflowDefinitionName}` -> 202, output `status` = `DELETING`. Must actually remove the definition (and its runs/sessions/acts) so a following GetWorkflowDefinition returns `ResourceNotFoundException` (404). Idempotent delete of an already-absent name -> 404 per the modeled error set.

Every response above must be computed from persisted state held in the shared engine via `crate::state::service_state(&*self.engine)` — no canned bodies, no echoing the request, no UUID generated without a stored record. Required output members (`status`, `arn`, `createdAt`, `workflowRunId`, `sessionId`, `actId`) must reflect what was stored. Enforce the modeled @httpLabel/@length/@pattern constraints (names 1..40 `^[a-zA-Z0-9_-]{1,40}$`, ids as lowercase UUIDs, `clientToken` 33..256) with `ValidationException` (400), and resolve parentage at every level so a child created under a missing parent is a 404 rather than a success.

## State Model

In `src/state.rs` (the per-service store reached through the shared engine):

- `workflow_definitions: BTreeMap<String /* name */, WorkflowDefinition>`
- `WorkflowDefinition { name: String, arn: String, created_at: SystemTime, description: Option<String>, export_config: Option<WorkflowExportConfig>, status: WorkflowDefinitionStatus /* ACTIVE | DELETING */, runs: BTreeMap<String /* workflowRunId */, WorkflowRun>, client_tokens: HashMap<String, String> }`
- `WorkflowRun { run_id: String /* uuid */, arn: String, status: WorkflowRunStatus /* RUNNING | SUCCEEDED | FAILED | TIMED_OUT | DELETING */, started_at: SystemTime, ended_at: Option<SystemTime>, model_id: String, log_group_name: Option<String>, client_info: ClientInfo, sessions: BTreeMap<String /* sessionId */, Session> }`
- `Session { session_id: String /* uuid */, created_at: SystemTime, acts: BTreeMap<String /* actId */, Act>, insertion_seq: u64 }`
- `Act { act_id: String /* uuid */, task: String, tool_specs: Vec<ToolSpec>, status: ActStatus /* PENDING_CLIENT_ACTION | PENDING_HUMAN_ACTION | RUNNING | SUCCEEDED | FAILED | TIMED_OUT */, error: Option<ActError>, created_at: SystemTime, steps: Vec<ActStep> }`
- `idempotency: HashMap<(OperationName, ClientToken), StoredResponse>` so @idempotent creates replay their first response instead of creating a duplicate.

Maps keep insertion/creation order (or carry `insertion_seq`) so the later List* operations can honour `sortOrder`, `maxResults` and `nextToken` without a state redesign.

Out of scope for this first slice (stay 501, to be filed as individual gap issues later): ListWorkflowDefinitions, ListWorkflowRuns, ListSessions, ListActs, UpdateWorkflowRun, UpdateAct, DeleteWorkflowRun, InvokeActStep, ListModels.

## Evidence

`grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-novaact-amazonnovaagentsdataplane/src/ops/` matches all 16 handler files (create_workflow_definition.rs, get_workflow_definition.rs, create_workflow_run.rs, get_workflow_run.rs, create_session.rs, create_act.rs, delete_workflow_definition.rs, ... ), each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` => HTTP 501 with zero persisted state. Routes, required/optional members, status codes, patterns and modeled errors cited above are taken verbatim from the crate's OPERATIONS.md (generated from smithy-model/com-amazonaws-novaact-amazonnovaagentsdataplane.json): restJson1, sigv4 `nova-act`; CreateWorkflowDefinition PUT /workflow-definitions -> 201 (output `status` required); GetWorkflowDefinition GET /workflow-definitions/{workflowDefinitionName} -> 200 (output name/arn/createdAt/status required); CreateWorkflowRun PUT .../workflow-runs -> 201 (`modelId` + `clientInfo` required, output workflowRunId/status required); GetWorkflowRun GET .../workflow-runs/{workflowRunId} -> 200 (workflowRunArn/workflowRunId/status/startedAt/modelId required); CreateSession PUT .../sessions -> 201 (`sessionId` required); CreateAct PUT .../acts -> 201 (`task` required, output actId/status required); DeleteWorkflowDefinition DELETE -> 202 with `status` required. ResourceNotFoundException (404), ConflictException (409) and ValidationException (400) are declared on these operations in the model. No tests exercise any of this (tests/ holds only the generated state_roundtrip.rs example).

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/`aws nova-act` client at it (restJson1, signing name `nova-act`).
2. CreateWorkflowDefinition with name=`demo-workflow`, description=`bootstrap demo` — expect 201 and `status=ACTIVE`; today: HTTP 501 NotImplemented.
3. GetWorkflowDefinition for `demo-workflow` — expect 200 echoing the STORED name, description, a well-formed `arn` ending in `workflow-definition/demo-workflow`, a `createdAt` timestamp and `status=ACTIVE`.
4. CreateWorkflowRun on `demo-workflow` with modelId=`nova-act-v1` and clientInfo — expect 201, a lowercase-UUID `workflowRunId` and `status=RUNNING`.
5. GetWorkflowRun with that id — expect 200 with the same `workflowRunId`, `modelId=nova-act-v1`, `status=RUNNING`, a `startedAt`, and a `workflowRunArn` containing both the definition name and the run id.
6. CreateSession on that run — expect 201 and a UUID `sessionId`; repeat the call with the SAME clientToken and expect the identical `sessionId` (idempotency, no duplicate session).
7. CreateAct on that session with task=`search for a product` — expect 201, a UUID `actId` and a `status` from ActStatus.
8. CreateWorkflowRun / CreateSession / CreateAct against a non-existent definition, run or session — expect ResourceNotFoundException (HTTP 404), not a success or 501.
9. DeleteWorkflowDefinition `demo-workflow` — expect 202 with `status=DELETING`; then GetWorkflowDefinition for it must return ResourceNotFoundException (HTTP 404), proving the delete mutated real state.
