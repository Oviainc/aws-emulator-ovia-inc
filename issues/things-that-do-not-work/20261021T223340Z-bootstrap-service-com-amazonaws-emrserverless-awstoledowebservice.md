Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 103148eb649c3bac8e519c843b1d31d8637412c4

# Bootstrap the EMR Serverless emulator

_Filed: 2026-10-21T22:33:40Z_
_Operation: service-com-amazonaws-emrserverless-awstoledowebservice | Class: missing_

> _Note: this issue was identified against the code base at commit `103148eb649c3bac8e519c843b1d31d8637412c4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 22 operations in this crate is an unmodified scaffold: each `src/ops/*.rs` carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, i.e. HTTP 501 `NotImplemented`, and the service holds no state. Implement a FIRST, SMALL stateful slice: the application lifecycle plus one job run, so a realistic client script works end-to-end.

### Happy path to implement (in order)

1. **CreateApplication** — `POST /applications`. Input: `name`, `releaseLabel` (required), `type` (required, e.g. `SPARK`/`HIVE`), `clientToken` (required, @idempotencyToken), plus optional `initialCapacity`, `maximumCapacity`, `tags`, `autoStartConfiguration`, `autoStopConfiguration`, `architecture`. Must mint a fresh `applicationId` matching `^[0-9a-z]+$` (len 1..64), build `arn` = `arn:aws:emr-serverless:<region>:<account>:/applications/<applicationId>` (the model's ARN pattern), persist the whole record with `state=CREATED`, `createdAt`/`updatedAt` = now, and return `{applicationId, name, arn}`. Repeating the same `clientToken` must return the SAME applicationId (the op is `@idempotent`), not a new one.
2. **GetApplication** — `GET /applications/{applicationId}`. Must return the stored `Application` structure, reflecting exactly what CreateApplication/UpdateApplication persisted: `applicationId`, `name`, `arn`, `releaseLabel`, `type`, `state`, `stateDetails`, `initialCapacity`, `maximumCapacity`, `createdAt`, `updatedAt`, `tags`, `autoStart/autoStopConfiguration`, `architecture`. Unknown id → `ResourceNotFoundException` (HTTP 404), never 501 or an empty 200.
3. **StartApplication** — `POST /applications/{applicationId}/start`. Must transition the persisted record `CREATED|STOPPED → STARTED` (modelling `STARTING` is optional for the first slice) and bump `updatedAt`; a following GetApplication must observe `state=STARTED`. Unknown id → 404.
4. **StartJobRun** — `POST /applications/{applicationId}/jobruns`. Input: `clientToken` (required), `executionRoleArn` (required), `jobDriver`, `configurationOverrides`, `name`, `mode`, `executionTimeoutMinutes`, `tags`. Must reject a job run against an unknown application with `ResourceNotFoundException` (404). Must mint a `jobRunId` (`^[0-9a-z]+$`), build `arn` = `.../applications/<applicationId>/jobruns/<jobRunId>`, persist a JobRun with `state=SUBMITTED` (or `RUNNING`), `stateDetails`, `createdBy`, `createdAt`, `updatedAt`, `executionRole`, `releaseLabel` copied from the parent application, `jobDriver`, `mode`, `attempt=1`, and return `{applicationId, jobRunId, arn}`. Same `clientToken` twice → same `jobRunId`.
5. **GetJobRun** — `GET /applications/{applicationId}/jobruns/{jobRunId}`. Must return the persisted `JobRun` (all required members above) for the stored ids; unknown application or job run → `ResourceNotFoundException` (404).
6. **ListJobRuns** — `GET /applications/{applicationId}/jobruns`. Must return `jobRuns` as `JobRunSummary` records derived from the stored job runs of that application only (`id`, `applicationId`, `arn`, `name`, `mode`, `createdBy`, `createdAt`, `updatedAt`, `executionRole`, `state`, `stateDetails`, `releaseLabel`), honouring `maxResults` with an opaque `nextToken` (`^[A-Za-z0-9_=-]+$`) and the `states` filter.
7. **ListApplications / DeleteApplication** (closing the lifecycle) — ListApplications must return `ApplicationSummary` for every stored application (`id`, `name`, `arn`, `releaseLabel`, `type`, `state`, `createdAt`, `updatedAt`, `architecture`) with `maxResults`/`nextToken`/`states` honoured; DeleteApplication must remove the record so GetApplication then returns 404 and ListApplications no longer lists it. Real EMR Serverless refuses to delete a STARTED application — returning `ValidationException` in that case is the correct behaviour.

**NEVER STUBS:** every one of these must compute its response from persisted state. Hardcoded bodies, echoing the input back, or minting an id without storing the record are failures — a `CreateApplication` followed by `GetApplication`/`ListApplications` must return exactly what was created, and a `StartJobRun` followed by `GetJobRun`/`ListJobRuns` must return exactly the job that was submitted.

## State Model

In `src/state.rs` (reached by handlers through `crate::state::service_state(&*self.engine)`, one shared store per engine):

```
EmrServerlessState {
    applications: HashMap<String /*applicationId*/, Application>,
    // clientToken -> applicationId, for @idempotentcy of CreateApplication
    create_app_tokens: HashMap<String, String>,
    next_id_seed: u64,   // source of lowercase-alnum ids
}

Application {
    application_id: String,       // ^[0-9a-z]+$
    name: Option<String>,
    arn: String,                  // arn:aws:emr-serverless:<region>:<acct>:/applications/<id>
    release_label: String,
    engine_type: String,          // "SPARK" | "HIVE" | ...
    state: ApplicationState,      // CREATING|CREATED|STARTING|STARTED|STOPPING|STOPPED|TERMINATED
    state_details: Option<String>,
    initial_capacity: Map<String, InitialCapacityConfig>,
    maximum_capacity: Option<MaximumAllowedResources>,
    auto_start: Option<AutoStartConfig>,
    auto_stop: Option<AutoStopConfig>,
    network_configuration: Option<NetworkConfiguration>,
    architecture: Option<Architecture>,
    runtime_configuration: Vec<Configuration>,
    tags: HashMap<String, String>,
    created_at: Timestamp,
    updated_at: Timestamp,
    job_runs: HashMap<String /*jobRunId*/, JobRunRecord>,
    job_run_tokens: HashMap<String /*clientToken*/, String /*jobRunId*/>,
    sessions: HashMap<String /*sessionId*/, SessionRecord>,   // later slice
}

JobRunRecord {
    job_run_id: String,           // ^[0-9a-z]+$
    application_id: String,
    arn: String,                  // .../applications/<appId>/jobruns/<jobRunId>
    name: Option<String>,
    created_by: String,           // caller principal ARN from the request context
    execution_role: String,
    execution_timeout_minutes: i64,  // default 720 when unset
    job_driver: Option<JobDriver>,
    configuration_overrides: Option<ConfigurationOverrides>,
    mode: Option<JobRunMode>,     // BATCH | STREAMING
    retry_policy: Option<RetryPolicy>,
    state: JobRunState,           // SUBMITTED|PENDING|SCHEDULED|RUNNING|SUCCESS|FAILED|CANCELLING|CANCELLED|QUEUED
    state_details: String,
    release_label: String,        // inherited from the application
    attempt: i32,
    tags: HashMap<String, String>,
    created_at: Timestamp,
    updated_at: Timestamp,
    started_at: Option<Timestamp>,
    ended_at: Option<Timestamp>,
}
```

List pagination should be a deterministic ordering (creation order / id) with the `nextToken` encoding the offset or last-seen key, so repeated pages neither skip nor duplicate.

Done = an integration test doing CreateApplication → GetApplication → StartApplication → StartJobRun → GetJobRun → ListJobRuns → (CancelJobRun or state transition) → StopApplication → DeleteApplication → GetApplication(404) passes with correct, stateful bodies — not merely 2xx.

Everything else (StartSession/GetSession/TerminateSession/ListSessions/GetSessionEndpoint, GetDashboardForJobRun, GetResourceDashboard, ListJobRunAttempts, UpdateApplication's full config merge, Tag/Untag/ListTagsForResource) stays 501 for now and will be filed as individual gap issues.

## Evidence

`services/service-com-amazonaws-emrserverless-awstoledowebservice/OPERATIONS.md` lists all 22 operations with their routes, members and errors (restJson1, sigv4 name `emr-serverless`); a grep for `^//! SCAFFOLDED, NOT IMPLEMENTED:` under `src/ops/` matches all 22 handler files, so no operation has any behaviour and the service holds zero state. Field names and requiredness above are taken from OPERATIONS.md (CreateApplication/GetApplication/StartApplication/StartJobRun/GetJobRun/ListJobRuns/ListApplications/DeleteApplication) and from the Smithy shapes `Application`, `ApplicationSummary`, `JobRun`, `JobRunSummary` in `smithy-model/com-amazonaws-emrserverless-awstoledowebservice.json`, including the ARN patterns `^arn:(aws[a-zA-Z0-9-]*):emr-serverless:.+:(\d{12}):\/applications\/[0-9a-zA-Z]+$` and the `/jobruns/` variant, the id pattern `^[0-9a-z]+$`, and the documented 404 `ResourceNotFoundException` / 400 `ValidationException` / 409 `ConflictException` failures.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `emr-serverless` client at it.
2. `aws emr-serverless create-application --name demo --release-label emr-7.1.0 --type SPARK --client-token tok-1` → expect 200 with an `applicationId` and a matching `/applications/<id>` ARN; today it returns HTTP 501 NotImplemented.
3. Repeat the identical create-application call with `--client-token tok-1` → expect the SAME applicationId (operation is @idempotent).
4. `aws emr-serverless get-application --application-id <id>` → expect the created name, releaseLabel, type, tags and `state=CREATED`, computed from stored state.
5. `aws emr-serverless start-application --application-id <id>` then get-application again → expect `state=STARTED` and a bumped `updatedAt`.
6. `aws emr-serverless start-job-run --application-id <id> --client-token job-1 --execution-role-arn arn:aws:iam::123456789012:role/EMRServerlessRole --job-driver '{"sparkSubmit":{"entryPoint":"s3://bucket/app.py"}}'` → expect 200 with `{applicationId, jobRunId, arn}`.
7. `aws emr-serverless get-job-run --application-id <id> --job-run-id <jobRunId>` → expect the persisted jobDriver, executionRole, releaseLabel inherited from the application, a valid `state` and `createdAt`.
8. `aws emr-serverless list-job-runs --application-id <id> --max-results 1` → expect exactly one JobRunSummary for that application plus a `nextToken` when more exist.
9. `aws emr-serverless get-job-run --application-id <id> --job-run-id doesnotexist` → expect HTTP 404 ResourceNotFoundException.
10. `aws emr-serverless list-applications` → expect the application listed; then `stop-application`, `delete-application`, and `get-application` → expect HTTP 404 and the application gone from list-applications.
