Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 9fd9433686ca60725b6b64638320351edab7ca3b

# Bootstrap the AmplifyBackend emulator

_Filed: 2026-10-21T21:53:09Z_
_Operation: service-com-amazonaws-amplifybackend-amplifybackend | Class: missing_

> _Note: this issue was identified against the code base at commit `9fd9433686ca60725b6b64638320351edab7ca3b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate is a fresh scaffold: all 31 handlers under `services/service-com-amazonaws-amplifybackend-amplifybackend/src/ops/` carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and answer HTTP 501, and `src/state.rs` holds no fields (`// TODO: add service-specific state fields`). Nothing is persisted, so no client workflow can run.

Implement a FIRST SMALL SLICE: the backend-environment lifecycle plus the async job record that every mutating AmplifyBackend call returns. Protocol is `restJson1` (sdkId `AmplifyBackend`, signing name `amplifybackend`). Six operations, in order:

1. **CreateBackend** — `POST /backend`. Input: `AppId` (required), `AppName` (required), `BackendEnvironmentName` (required), `ResourceConfig`, `ResourceName`. Must create the app record if absent, create the named backend environment (error if it already exists is NOT required for this slice), record a job, and return `AppId`, `BackendEnvironmentName`, `JobId` (freshly generated and STORED), `Operation` = `CreateBackend`, `Status` (e.g. `COMPLETED`/`SUCCESS` for the emulator). `Error` must be absent on success.
2. **CreateBackendAuth** — `POST /backend/{AppId}/auth`. Input: `AppId` (label, required), `BackendEnvironmentName` (required), `ResourceName` (required), `ResourceConfig` (`CreateBackendAuthResourceConfig`, required). Must store the full `ResourceConfig` verbatim under (AppId, BackendEnvironmentName, ResourceName) so `GetBackendAuth` can return it, and must record a job with `Operation` = `CreateBackendAuth`. Must return `NotFoundException` (404) when the app/environment was never created.
3. **GetBackend** — `POST /backend/{AppId}/details`. Input: `AppId` (label, required), `BackendEnvironmentName` (optional). Output must be computed from state: `AppId`, `AppName` as given at create time, `BackendEnvironmentName` (echo only when the environment exists), and `BackendEnvironmentList` = the list of environment names actually created for the app. Unknown `AppId` → `NotFoundException` (404).
4. **GetBackendAuth** — `POST /backend/{AppId}/auth/{BackendEnvironmentName}/details`. Input: labels + `ResourceName` (required). Must return the stored `ResourceConfig` (`CreateBackendAuthResourceConfig`) and `ResourceName`; unknown app/env/resource → `NotFoundException` (404). No canned config.
5. **GetBackendJob** — `GET /backend/{AppId}/job/{BackendEnvironmentName}/{JobId}`. Must look up the job stored by step 1/2 and return its `JobId`, `Operation`, `Status`, `CreateTime`, `UpdateTime` (and `Error` when the job failed). Unknown `JobId` → `NotFoundException` (404).
6. **DeleteBackend** — `POST /backend/{AppId}/environments/{BackendEnvironmentName}/remove`. Must remove the environment and its auth/api/storage resources from state, record a job with `Operation` = `DeleteBackend`, and return `JobId`/`Status`. A following `GetBackend` must no longer list the environment, and `GetBackendAuth` for it must 404.

Every one of these must compute its response from persisted state — no canned bodies, no echoing inputs, no random `JobId` that is never stored. Timestamps must be real and `UpdateTime` >= `CreateTime`. Missing required members and unknown paths must produce the modeled `BadRequestException` (400) / `NotFoundException` (404) shapes, not 501 and not 200.

## State Model

In `ComAmazonawsAmplifybackendAmplifybackendState`:

- `apps: HashMap<String /*AppId*/, BackendApp>`
- `BackendApp { app_id: String, app_name: String, environments: HashMap<String /*BackendEnvironmentName*/, BackendEnvironment>, jobs: HashMap<String /*JobId*/, BackendJob>, next_job_seq: u64 }`
- `BackendEnvironment { name: String, resource_config: Option<ResourceConfig>, resource_name: Option<String>, auth: HashMap<String /*ResourceName*/, CreateBackendAuthResourceConfig>, api: HashMap<String, BackendAPIResourceConfig>, storage: HashMap<String, StorageConfig>, created_at: SystemTime }`
- `BackendJob { job_id: String, app_id: String, backend_environment_name: String, operation: String, status: String, error: Option<String>, create_time: SystemTime, update_time: SystemTime }`

Job IDs are generated once in the create/delete handlers and inserted into `BackendApp.jobs`; `GetBackendJob` reads only from that map. Deleting an environment removes its `BackendEnvironment` entry (jobs are retained so a job lookup after delete still resolves).

All remaining operations (CloneBackend, CreateBackendAPI/Storage/Config, Create/Get/DeleteToken, Import*, Update*, ListBackendJobs, ListS3Buckets, RemoveAllBackends, RemoveBackendConfig, GenerateBackendAPIModels, GetBackendAPIModels, ...) stay 501 for now and will be filed as individual gap issues.

## Evidence

`services/service-com-amazonaws-amplifybackend-amplifybackend/OPERATIONS.md` documents the routes, required members and outputs cited above (CreateBackend lines 33-38, CreateBackendAuth 47-52, GetBackend 117-122, GetBackendAuth 138-143, GetBackendJob 145-150, DeleteBackend 75-80) and the common error set `BadRequestException 400 / NotFoundException 404 / TooManyRequestsException 429 / GatewayTimeoutException 504`. `grep '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 31 operation files, and `src/state.rs` declares `ComAmazonawsAmplifybackendAmplifybackendState` with no fields, proving zero persisted state. No tests exercise any operation (only the generated `tests/state_roundtrip.rs`).

## Steps to Reproduce

1. POST /backend with {"AppId":"d1abc2","AppName":"demo","BackendEnvironmentName":"staging"} — expect 200 with a JobId; actual: HTTP 501 NotImplemented.
2. POST /backend/d1abc2/auth with BackendEnvironmentName=staging, ResourceName=authdemo and a CreateBackendAuthResourceConfig — expect 200 with a JobId.
3. GET /backend/d1abc2/job/staging/{JobId} — expect the stored job with Operation=CreateBackendAuth and CreateTime/UpdateTime.
4. POST /backend/d1abc2/details — expect AppName=demo and BackendEnvironmentList containing "staging".
5. POST /backend/d1abc2/auth/staging/details with ResourceName=authdemo — expect the exact ResourceConfig submitted in step 2.
6. POST /backend/d1abc2/environments/staging/remove — expect 200 with a JobId; then POST /backend/d1abc2/details must no longer list "staging" and POST /backend/d1abc2/auth/staging/details must return NotFoundException (404).
7. POST /backend/unknownapp/details — expect NotFoundException (404), not 501.
