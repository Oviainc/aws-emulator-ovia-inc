Issue-Severity: MEDIUM
Issue-Gap-Class: event-propagation
Issue-Commit: a85c524bdc735756e5abfd799d7dcf6de3bb06dc

# A SITEWISE sync job never syncs anything and always settles in ERROR/SYNC_INITIALIZING_ERROR instead of reaching ACTIVE

_Filed: 2026-10-24T07:51:11Z_
_Operation: CreateSyncJob | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `a85c524bdc735756e5abfd799d7dcf6de3bb06dc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A sync job is a workspace's standing link to IoT SiteWise: once created with a role IoT TwinMaker can assume, the service reads the SiteWise asset-model hierarchy and creates/updates the corresponding component types and entities in the workspace, moving the job CREATING -> INITIALIZING -> ACTIVE and reporting the projected resources (with `syncSource` set on the created entities/component types).

- Expected: after CreateSyncJob(syncSource="SITEWISE", syncRole=<assumable role>), polling GetSyncJob reports `status.state` progressing to INITIALIZING and then ACTIVE with no error, and the workspace's entities/component types projected from SiteWise carry `syncSource = SITEWISE`.
- Actual: the job is persisted in CREATING and, 500 ms later, any read materialises the hardcoded terminal failure `status.state = ERROR` with `error.code = SYNC_INITIALIZING_ERROR` and an emulator-authored message ("this emulator synchronises nothing..."). No ACTIVE state is reachable by any input, nothing is ever projected into the workspace, and GetEntity/GetComponentType always report `syncSource: None`. A client that waits for ACTIVE before using the link never proceeds.

## Evidence

Spec: the model declares `SyncJobState` including INITIALIZING and ACTIVE, `SyncJobStatus.error`, and `syncSource` members on EntityResponse/ComponentResponse/GetComponentTypeResponse — all unreachable here. Handler/state: `services/.../src/state.rs` — `SYNC_JOB_ERROR_CODE = "SYNC_INITIALIZING_ERROR"`, `SYNC_JOB_ERROR_MESSAGE`, and `SyncJob::settle()` (state.rs:741-754) which unconditionally writes `SYNC_JOB_STATE_ERROR` once `settles_at` passes; `src/ops/create_sync_job.rs:248-282` persists the job and performs no synchronisation; `src/ops/get_sync_job.rs:231-242` settles then reports that status; `src/ops/get_entity.rs:238` and `src/ops/get_component_type.rs:308` hardcode `sync_source: None`. Tests: `tests/all/sync_job.rs:246-277` ASSERTS the ERROR/SYNC_INITIALIZING_ERROR outcome, so the divergence is locked in rather than covered. (Distinct from open issue 20261022T145710Z, whose CreateSyncJob cause is the 501 scaffold plus synchronous syncRole trust validation.)

## Steps to Reproduce

1. CreateWorkspace("ws-sync") with a role trusting iottwinmaker.amazonaws.com.
2. CreateSyncJob(workspaceId="ws-sync", syncSource="SITEWISE", syncRole=<role arn that iottwinmaker can assume>) -> 200 with state CREATING.
3. Poll GetSyncJob(workspaceId="ws-sync", syncSource="SITEWISE") for a few seconds. Expected: state INITIALIZING then ACTIVE, status.error empty. Actual: state ERROR with error.code SYNC_INITIALIZING_ERROR.
4. ListEntities/GetComponentType on "ws-sync". Expected: resources projected from the SiteWise hierarchy, each carrying syncSource=SITEWISE. Actual: nothing projected; syncSource is never reported.
