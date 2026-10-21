Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4

# Bootstrap the mediaconvert emulator

_Filed: 2026-10-21T21:48:19Z_
_Operation: service-com-amazonaws-mediaconvert-mediaconvert | Class: missing_

> _Note: this issue was identified against the code base at commit `a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All 34 MediaConvert operations in `services/service-com-amazonaws-mediaconvert-mediaconvert/src/ops/` are unmodified scaffolds that answer HTTP 501 `NotImplemented` and hold no state, so no client workflow works at all. Implement a FIRST minimal, stateful slice: the transcoding-job lifecycle on the primary resources Queue and Job (restJson1, URI prefix `/2017-08-29`).

Happy path, in order (every response must be computed from persisted state — no canned, hardcoded or echoed-back responses, no random IDs that are never stored):

1. **CreateQueue** — `POST /2017-08-29/queues` -> 201. Input: `Name` (required), `Description`, `PricingPlan` (ON_DEMAND|RESERVED, default ON_DEMAND), `Status` (ACTIVE|PAUSED, default ACTIVE), `ConcurrentJobs`, `Tags`. Output: `Queue` with `Name`, `Arn` (`arn:aws:mediaconvert:<region>:<account>:queues/<Name>`), `CreatedAt`/`LastUpdated` (epoch seconds), `Status`, `PricingPlan`, `Type` (CUSTOM for user-created; the built-in `Default` queue is SYSTEM), `SubmittedJobsCount`/`ProgressingJobsCount`. Re-creating an existing name -> `ConflictException` (409).
2. **CreateJob** — `POST /2017-08-29/jobs` -> 201. Input: `Role` (required), `Settings` (JobSettings, required), `Queue` (defaults to `Default`), `Priority` (-50..50), `UserMetadata`, `Tags`, `ClientRequestToken` (@idempotencyToken), `StatusUpdateInterval`, `BillingTagsSource`. Must mint a new `Id`, store the job, and return `Job` with the SUBMITTED `Status`, `Arn` (`arn:aws:mediaconvert:<region>:<account>:jobs/<Id>`), `CreatedAt`, `Queue` resolved to the actually requested/default queue, and `Role`/`Settings`/`UserMetadata`/`Priority` exactly as submitted. Unknown `Queue` -> `NotFoundException` (404); missing `Role`/`Settings` -> `BadRequestException` (400). Repeating the same `ClientRequestToken` must return the SAME job, not a second one.
3. **GetJob** — `GET /2017-08-29/jobs/{Id}` -> 200. Must return the stored `Job` for `Id` with the same `Settings`, `Role`, `Queue`, `UserMetadata`, `CreatedAt` and current `Status` that CreateJob persisted. Unknown `Id` -> `NotFoundException` (404).
4. **ListJobs** — `GET /2017-08-29/jobs` -> 200. Must return jobs from the store, honouring `queue` and `status` query filters, `order` (ASCENDING|DESCENDING by `CreatedAt`, default DESCENDING), `maxResults` (1..20) and the opaque `nextToken` round-trip (`NextToken` set only when more pages remain).
5. **CancelJob** — `DELETE /2017-08-29/jobs/{Id}` -> 202, empty body. Must flip the stored job's `Status` to CANCELED so a subsequent GetJob/ListJobs observes it. Unknown `Id` -> `NotFoundException` (404); cancelling a job that is already COMPLETE/CANCELED -> `ConflictException` (409).

## State Model

Per-service state in `src/state.rs`, reached via `crate::state::service_state(&*self.engine)` (one shared engine; never construct a new engine in a handler):

- `queues: BTreeMap<String /*Name*/, QueueRecord>` where `QueueRecord { name, arn, description: Option<String>, pricing_plan: PricingPlan, status: QueueStatus, queue_type: Type, concurrent_jobs: Option<i32>, created_at: SystemTime, last_updated: SystemTime, tags: BTreeMap<String,String> }`. Seeded at startup with the built-in `Default` queue (Type=SYSTEM, Status=ACTIVE, PricingPlan=ON_DEMAND).
- `jobs: BTreeMap<String /*Id*/, JobRecord>` where `JobRecord { id, arn, role, settings: JobSettings, queue: String, job_template: Option<String>, priority: Option<i32>, status: JobStatus, created_at: SystemTime, status_update_interval: Option<StatusUpdateInterval>, billing_tags_source: Option<BillingTagsSource>, user_metadata: BTreeMap<String,String>, client_request_token: Option<String>, timing: Timing, tags: BTreeMap<String,String> }`.
- `idempotency: BTreeMap<String /*ClientRequestToken*/, String /*job Id*/>` so CreateJob is idempotent.
- Derived, never stored twice: a queue's `SubmittedJobsCount`/`ProgressingJobsCount` are computed by counting `jobs` whose `queue` matches and whose `status` is SUBMITTED/PROGRESSING.

Done = an integration test doing CreateQueue -> CreateJob (into that queue) -> GetJob (settings/role/queue/metadata match what was submitted) -> ListJobs (filtered by queue and status, paginated) -> CancelJob -> GetJob shows CANCELED, passing with correct bodies, not just 2xx. All other operations (job templates, presets, policy, tags, Probe, DescribeEndpoints, SearchJobs/StartJobsQuery, certificates, versions) stay 501 for now and will be filed as individual gap issues.

## Evidence

OPERATIONS.md for service-com-amazonaws-mediaconvert-mediaconvert lists all 34 operations with routes/fields: CreateQueue `POST /2017-08-29/queues -> 201` (Name required, output Queue), CreateJob `POST /2017-08-29/jobs -> 201` (Role + Settings required, ClientRequestToken @idempotencyToken, output Job), GetJob `GET /2017-08-29/jobs/{Id} -> 200` (output Job), ListJobs `GET /2017-08-29/jobs -> 200` (@paginated inputToken=NextToken/outputToken=NextToken/items=Jobs, maxResults 1..20, queue/status/order query params), CancelJob `DELETE /2017-08-29/jobs/{Id} -> 202` (no output members); all declare NotFoundException 404 / ConflictException 409 / BadRequestException 400. Smithy shapes `Job` (Id, Arn, Status JobStatus, CreatedAt epoch-seconds, Queue, Role required, Settings required, UserMetadata, Priority, Timing) and `Queue` (Name required, Arn, CreatedAt/LastUpdated epoch-seconds, Status, PricingPlan, Type, SubmittedJobsCount, ProgressingJobsCount) give the exact response fields; the Queue doc states jobs without a queue go to the default queue. `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 34 handler files (create_job.rs, get_job.rs, list_jobs.rs, cancel_job.rs, create_queue.rs included), so there is no behavior and no test coverage today.

## Steps to Reproduce

1. Start the emulator and point the AWS CLI/SDK at it with `--endpoint-url`.
2. `aws mediaconvert create-queue --name test-queue` -> expect 201 with Queue.Name=test-queue, an Arn, CreatedAt and Status=ACTIVE; currently HTTP 501 NotImplemented.
3. `aws mediaconvert create-job --role arn:aws:iam::123456789012:role/MediaConvertRole --queue test-queue --settings file://settings.json --user-metadata k=v` -> expect 201 with a stored Job{Id, Status=SUBMITTED, Queue=test-queue}.
4. `aws mediaconvert get-job --id <Id>` -> expect the same Settings, Role, Queue, UserMetadata and CreatedAt that were submitted.
5. `aws mediaconvert list-jobs --queue test-queue --status SUBMITTED --max-results 1` -> expect the job listed, and a NextToken only if more pages exist.
6. `aws mediaconvert cancel-job --id <Id>` -> expect 202, then `get-job --id <Id>` must report Status=CANCELED.
7. `aws mediaconvert get-job --id doesnotexist` -> expect NotFoundException (HTTP 404), not 501.
