Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 09acb2538adf9fea9bce04c8d04105f42be236df

# Bootstrap the deadline emulator

_Filed: 2026-10-21T21:38:00Z_
_Operation: service-com-amazonaws-deadline-deadline | Class: missing_

> _Note: this issue was identified against the code base at commit `09acb2538adf9fea9bce04c8d04105f42be236df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every handler under `services/service-com-amazonaws-deadline-deadline/src/ops/` is an unmodified scaffold answering HTTP 501 `NotImplemented` and holding no state, so no AWS Deadline Cloud client workflow can run at all. Implement a FIRST minimal, stateful slice covering the primary resource hierarchy farm -> queue -> job:

1. **CreateFarm** — `POST /2023-10-12/farms`. Input: `displayName` (required, 1..100), `description` (default ""), `kmsKeyArn`, `costScaleFactor` (default 1), `tags`, `clientToken` (`X-Amz-Client-Token`). Must mint a new id matching `^farm-[0-9a-f]{32}$`, persist the record (displayName/description/costScaleFactor/kmsKeyArn/tags + createdAt/createdBy) and return `farmId`.
2. **GetFarm** — `GET /2023-10-12/farms/{farmId}`. Must look the farm up in state and return the values stored by CreateFarm: `farmId`, `displayName`, `description`, `kmsKeyArn`, `costScaleFactor`, `createdAt`, `createdBy` (+ `updatedAt`/`updatedBy` once UpdateFarm runs). Unknown `farmId` -> `ResourceNotFoundException` (HTTP 404), not 501 and not a canned farm.
3. **CreateQueue** — `POST /2023-10-12/farms/{farmId}/queues`. Input: `farmId` (label), `displayName` (required), `description`, `defaultBudgetAction` (default `NONE`), `roleArn`, `jobAttachmentSettings`, `jobRunAsUser`, `requiredFileSystemLocationNames`, `allowedStorageProfileIds`, `tags`. Must reject an unknown `farmId` with 404, mint `^queue-[0-9a-f]{32}$`, store the queue under the farm with `status = IDLE`, and return `queueId`.
4. **GetQueue** — `GET /2023-10-12/farms/{farmId}/queues/{queueId}`. Must return the persisted queue: `farmId`, `queueId`, `displayName`, `status`, `defaultBudgetAction`, `description`, `roleArn`, `jobAttachmentSettings`, `jobRunAsUser`, `requiredFileSystemLocationNames`, `allowedStorageProfileIds`, `createdAt`, `createdBy`. 404 when the farm or queue does not exist, or when the queue belongs to a different farm.
5. **CreateJob** — `POST /2023-10-12/farms/{farmId}/queues/{queueId}/jobs` (HTTP 201). Input: `template`, `templateType`, `priority` (required, 0..100), `parameters`, `targetTaskRunStatus` (default `READY`), `maxFailedTasksCount` (default 20), `maxRetriesPerTask` (default 5), `maxWorkerCount`, `storageProfileId`, `nameOverride`, `descriptionOverride`, `attachments`, `tags`. Must validate farm+queue exist (404 otherwise), mint `^job-[0-9a-f]{32}$`, persist the job under that queue with `lifecycleStatus = CREATE_COMPLETE`, `name` = `nameOverride` (or the name from the template), and return `jobId`.
6. **GetJob** — `GET /2023-10-12/farms/{farmId}/queues/{queueId}/jobs/{jobId}`. Must return the persisted job: `jobId`, `name`, `lifecycleStatus`, `lifecycleStatusMessage`, `priority`, `targetTaskRunStatus`, `taskRunStatus`, `maxFailedTasksCount`, `maxRetriesPerTask`, `maxWorkerCount`, `parameters`, `attachments`, `description`, `storageProfileId`, `createdAt`, `createdBy`. 404 for an unknown job id.

Also wire the two list reads that make the slice demoable from a CLI: **ListFarms** (`farms` from state, honouring `maxResults`/`nextToken`) and **ListJobs** (`jobs` scoped to the farm+queue), plus **DeleteQueue** / **DeleteFarm** so the lifecycle closes and a subsequent GetFarm/GetQueue returns 404.

Every one of these must compute its response from persisted state. No canned bodies, no echoing the request back, and no minting an id that is never stored — a create followed by a get/list must return exactly what was created, across separate HTTP requests sharing the one engine-held store.

## State Model

In `src/state.rs` (`ComAmazonawsDeadlineDeadlineState`):

- `farms: HashMap<String /*farmId*/, Farm>`
  - `Farm { farm_id, display_name, description, kms_key_arn: Option<String>, cost_scale_factor: f32, tags: HashMap<String,String>, created_at: DateTime, created_by: String, updated_at: Option<DateTime>, updated_by: Option<String>, queues: HashMap<String /*queueId*/, Queue> }`
- `Queue { queue_id, farm_id, display_name, description, status: QueueStatus /*IDLE*/, default_budget_action, blocked_reason: Option<..>, role_arn: Option<String>, job_attachment_settings: Option<JobAttachmentSettings>, job_run_as_user: Option<JobRunAsUser>, required_file_system_location_names: Vec<String>, allowed_storage_profile_ids: Vec<String>, scheduling_configuration: Option<SchedulingConfiguration>, tags, created_at, created_by, updated_at, updated_by, jobs: HashMap<String /*jobId*/, Job> }`
- `Job { job_id, queue_id, farm_id, name, description: Option<String>, template: Option<String>, template_type: Option<JobTemplateType>, lifecycle_status: JobLifecycleStatus /*CREATE_COMPLETE*/, lifecycle_status_message: String, priority: i32, parameters: HashMap<String, JobParameter>, attachments: Option<Attachments>, storage_profile_id: Option<String>, target_task_run_status, task_run_status: Option<TaskRunStatus>, task_run_status_counts: HashMap<TaskRunStatus, i32>, max_failed_tasks_count, max_retries_per_task, max_worker_count: Option<i32>, source_job_id: Option<String>, tags, created_at, created_by, updated_at, updated_by, started_at, ended_at }`
- `idempotency: HashMap<String /*clientToken*/, String /*created resource id*/>` so a repeated `X-Amz-Client-Token` on CreateFarm/CreateQueue/CreateJob returns the same id instead of a second resource.

Ids are generated as `farm-`/`queue-`/`job-` + 32 lowercase hex chars so they satisfy the modeled label patterns and can be fed straight back into the path-bound reads.

All other operations (fleets, workers, sessions, steps/tasks, budgets, limits, monitors, storage profiles, members, search, tagging, metered products) stay scaffolded at 501 and will be filed as individual gap issues later.

## Evidence

`services/service-com-amazonaws-deadline-deadline/src/ops/` contains ~130 handler files and every one carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (verified on `create_farm.rs` line 4; README.md documents the marker as the census for still-stubbed operations). The routes, fields, defaults, id patterns and error sets quoted above come from the crate's OPERATIONS.md (CreateFarm/GetFarm/DeleteFarm/ListFarms lines 256-294, CreateQueue/GetQueue/DeleteQueue/ListQueues lines 613-651, CreateJob/GetJob/ListJobs lines 747-777), which is rendered from `smithy-model/com-amazonaws-deadline-deadline.json`. Expected: the farm -> queue -> job create/read/list/delete loop works statefully over the REST-JSON `/2023-10-12/...` routes. Actual: every call answers HTTP 501 and nothing is stored. No tests cover these operations (only the generated `tests/state_roundtrip.rs` state-API example exists).

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `deadline` client at it.
2. `CreateFarm` with displayName="test-farm" — expect HTTP 200 and a `farmId` matching ^farm-[0-9a-f]{32}$; actual: HTTP 501 NotImplemented.
3. `GetFarm` with that farmId — expect displayName="test-farm" plus createdAt/createdBy from persisted state.
4. `CreateQueue` under that farm with displayName="test-queue" — expect a `queueId`; then `GetQueue` and expect displayName="test-queue" and status=IDLE.
5. `CreateJob` under that farm+queue with priority=50, templateType=JSON and a minimal template — expect HTTP 201 and a `jobId`.
6. `GetJob` with that jobId — expect name/priority=50/lifecycleStatus=CREATE_COMPLETE as created; `ListJobs` for the queue must include the job.
7. `ListFarms` must include the farm; `DeleteQueue` then `GetQueue` must return ResourceNotFoundException (404), and `DeleteFarm` then `GetFarm` likewise.
8. Call `GetFarm` with a well-formed but unknown farmId (farm-00000000000000000000000000000000) — expect ResourceNotFoundException (HTTP 404), not 501.
