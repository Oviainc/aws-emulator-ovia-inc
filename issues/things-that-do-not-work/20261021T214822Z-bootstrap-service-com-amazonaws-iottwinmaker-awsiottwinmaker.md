Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4

# Bootstrap the IoTTwinMaker emulator

_Filed: 2026-10-21T21:48:22Z_
_Operation: service-com-amazonaws-iottwinmaker-awsiottwinmaker | Class: missing_

> _Note: this issue was identified against the code base at commit `a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All 40 handlers in `services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/` are unmodified scaffolds answering HTTP 501 `NotImplemented` and holding zero state, so no client workflow works. Implement a SMALL first slice: the workspace -> entity lifecycle, the core loop of IoT TwinMaker (restJson1, sigv4 name `iottwinmaker`).

Happy path to implement, in order (fields from OPERATIONS.md):

1. **CreateWorkspace** — `POST /workspaces/{workspaceId}`. Input: `workspaceId` (httpLabel, required), `description`, `s3Location`, `role`, `tags`. Output: `arn` (required, must match `^arn:aws:iottwinmaker:<region>:<account>:workspace/<workspaceId>`-shaped pattern), `creationDateTime` (required). Must persist the workspace; a second create of the same id is a `ConflictException` (409).
2. **GetWorkspace** — `GET /workspaces/{workspaceId}`. Output must be read back from the stored record: `workspaceId`, `arn`, `description`, `s3Location`, `role`, `creationDateTime`, `updateDateTime` (all the required ones present). Unknown workspace -> `ResourceNotFoundException` (404). Note the input accepts the workspace id OR its ARN (len 1..2048 pattern allows both) — resolve both to the same record.
3. **CreateEntity** — `POST /workspaces/{workspaceId}/entities`. Input: `entityId` (optional — generate a UUID when absent), `entityName` (required), `description`, `components`, `compositeComponents`, `parentEntityId`, `tags`. Output: `entityId`, `arn`, `creationDateTime`, `state` (return `ACTIVE`). Unknown workspace -> 404; duplicate `entityId` -> 409.
4. **GetEntity** — `GET /workspaces/{workspaceId}/entities/{entityId}`. Every required output member must come from state: `entityId`, `entityName`, `arn`, `status` (Status with `state`=ACTIVE), `workspaceId`, `description`, `components`, `parentEntityId` (`$ROOT` when no parent was given), `hasChildEntities` (computed — true iff some stored entity names this one as parent), `creationDateTime`, `updateDateTime`. Unknown entity or workspace -> `ResourceNotFoundException` (404).
5. **ListEntities** — `POST /workspaces/{workspaceId}/entities-list`. Input: `filters` (ListEntitiesFilter: by `parentEntityId`, `componentTypeId`, `externalId`), `maxResults` (0..200), `nextToken`. Output: `entitySummaries` (EntitySummary per stored entity: entityId, entityName, arn, status, description, parentEntityId, hasChildEntities, creationDateTime, updateDateTime) plus `nextToken`. Must honour `maxResults` and round-trip an opaque `nextToken`; default behaviour (no filter) lists the workspace's root children per the model's filter semantics — at minimum return the entities actually created.
6. **DeleteEntity** — `DELETE /workspaces/{workspaceId}/entities/{entityId}` with `isRecursive` query flag. Output: `state` (`DELETING`). Must remove the entity from state so a following GetEntity is 404 and ListEntities no longer shows it; without `isRecursive`, deleting an entity that has children is a `ValidationException` (400).
7. **DeleteWorkspace** — `DELETE /workspaces/{workspaceId}`, output `message`. Must remove the workspace so GetWorkspace is 404 afterwards.

Every one of these must compute its response from persisted state. No canned bodies, no echoing the request, no generating an ARN/id that is never stored: a create followed by a get/list must return exactly what was created, and a delete must be observable by the next call.

## State Model

In `src/state.rs` (`ComAmazonawsIottwinmakerAwsiottwinmakerState`, reached via `crate::state::service_state(&*self.engine)`):

- `workspaces: BTreeMap<String /*workspaceId*/, Workspace>`
  - `Workspace { workspace_id: String, arn: String, description: Option<String>, s3_location: Option<String>, role: Option<String>, linked_services: Vec<String>, tags: BTreeMap<String,String>, creation_date_time: SystemTime, update_date_time: SystemTime, entities: BTreeMap<String /*entityId*/, Entity>, component_types: BTreeMap<String, ComponentType> /* later slice */ }`
- `Entity { entity_id: String, entity_name: String, arn: String, workspace_id: String, description: Option<String>, parent_entity_id: String /* "$ROOT" default */, components: BTreeMap<String, ComponentRecord>, tags: BTreeMap<String,String>, status_state: &'static str /* ACTIVE */, status_error: Option<ErrorDetails>, creation_date_time: SystemTime, update_date_time: SystemTime }`
  - `ComponentRecord { component_name: String, component_type_id: Option<String>, description: Option<String>, properties: BTreeMap<String, PropertyRecord>, status_state: String }`
- `arn_index: BTreeMap<String /*arn*/, ResourceRef>` so GetWorkspace-by-ARN (and later Tag/UntagResource) resolve without a scan.
- `hasChildEntities` is DERIVED by scanning `entities` for `parent_entity_id == entityId` — never stored as a stale flag.

The remaining 33 operations (component types, scenes, property values, sync jobs, metadata transfer jobs, ExecuteQuery, tagging, pricing plan) stay 501 for now and will be filed as individual gap issues.

## Evidence

`services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/OPERATIONS.md` lists all 40 operations with their routes, input/output members and errors — CreateWorkspace (`POST /workspaces/{workspaceId}` -> arn + creationDateTime), GetWorkspace (`GET /workspaces/{workspaceId}`, id-or-ARN label), CreateEntity (`POST /workspaces/{workspaceId}/entities` -> entityId/arn/creationDateTime/state), GetEntity (`GET /workspaces/{workspaceId}/entities/{entityId}` with required status/hasChildEntities/parentEntityId), ListEntities (`POST /workspaces/{workspaceId}/entities-list`, @paginated inputToken=nextToken/outputToken=nextToken, maxResults 0..200), DeleteEntity (`DELETE .../entities/{entityId}` with isRecursive query), DeleteWorkspace (`DELETE /workspaces/{workspaceId}`); documented errors include ResourceNotFoundException (404), ConflictException (409), ValidationException (400). `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 40 files in this crate, including create_workspace.rs, get_workspace.rs, create_entity.rs, get_entity.rs, list_entities.rs, delete_entity.rs and delete_workspace.rs — every handler returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501) and no state type holds workspaces or entities. No test exercises this workflow (only the generated `tests/state_roundtrip.rs`).

## Steps to Reproduce

1. CreateWorkspace workspaceId="demoWorkspace", description="demo" — expect 200 with a workspace ARN and creationDateTime (currently 501)
2. GetWorkspace workspaceId="demoWorkspace" — expect the stored description/arn/creationDateTime back; also call it with the ARN returned above and expect the same record
3. CreateEntity workspaceId="demoWorkspace", entityId="pump1", entityName="Pump 1", description="a pump" — expect 200 with entityId="pump1", an entity ARN and state=ACTIVE
4. GetEntity workspaceId="demoWorkspace", entityId="pump1" — expect entityName="Pump 1", description="a pump", parentEntityId="$ROOT", hasChildEntities=false, status.state=ACTIVE, creationDateTime/updateDateTime set
5. ListEntities workspaceId="demoWorkspace", maxResults=10 — expect entitySummaries containing exactly the "pump1" summary
6. CreateEntity entityId="sensor1", parentEntityId="pump1" then GetEntity "pump1" — expect hasChildEntities=true; DeleteEntity "pump1" without isRecursive — expect ValidationException (400)
7. DeleteEntity workspaceId="demoWorkspace", entityId="sensor1" then entityId="pump1" — expect state=DELETING, and a following GetEntity on each to be ResourceNotFoundException (404) with ListEntities returning no summaries
8. DeleteWorkspace workspaceId="demoWorkspace" — expect 200, then GetWorkspace on it to be ResourceNotFoundException (404)
9. GetEntity on a workspace that was never created — expect ResourceNotFoundException (404), not 501 or 200
