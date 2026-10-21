Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 7b44f3f32d1f58d662874eb8ca0f546e296cee00

# Bootstrap the service-com-amazonaws-iotjobsdataplane-iotlaserthingjobmanagerexternalservice emulator

_Filed: 2026-10-21T21:08:43Z_
_Operation: service-com-amazonaws-iotjobsdataplane-iotlaserthingjobmanagerexternalservice | Class: stub_

> _Note: this issue was identified against the code base at commit `7b44f3f32d1f58d662874eb8ca0f546e296cee00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every handler under `src/ops/` is an unmodified scaffold (`//! SCAFFOLDED, NOT IMPLEMENTED:` + `Err(crate::scaffolded::Scaffolded::not_implemented(..))`) and answers HTTP 501, so a device agent cannot run a single job. Implement this minimal, self-contained happy path (restJson1, sigv4 name `iot-jobs-data`), all 5 modeled ops, in this order:

1. **StartCommandExecution** — `POST /command-executions`. Input `targetArn` (required, the thing ARN, e.g. `arn:aws:iot:us-east-1:111122223333:thing/laser-1`), `commandArn` (required), optional `parameters` (map<string,CommandParameterValue>), `executionTimeoutSeconds` (range 1..), `clientToken` (@idempotencyToken, len 1..64, pattern `^[\x21-\x7E]+$`). Must MINT and PERSIST a new job execution for the thing named by `targetArn` in state with `status = QUEUED`, `queuedAt = now`, `versionNumber = 1`, `executionNumber = 1`, a `jobDocument` built from `commandArn` + `parameters`, and return the stored `executionId` (len 1..64, pattern `^[a-zA-Z0-9_-]+$`). Same `clientToken` replayed ⇒ same `executionId`, no second execution.
2. **GetPendingJobExecutions** — `GET /things/{thingName}/jobs`. Returns `queuedJobs` and `inProgressJobs` as lists of `JobExecutionSummary{jobId, queuedAt, startedAt, lastUpdatedAt, versionNumber, executionNumber}` computed by partitioning the persisted executions for that thing on `status` (QUEUED vs IN_PROGRESS), oldest-queued first. Must be empty lists for a thing with no executions (200, not 404).
3. **StartNextPendingJobExecution** — `PUT /things/{thingName}/jobs/$next`. Picks the oldest QUEUED execution for the thing, transitions it to `IN_PROGRESS`, sets `startedAt`/`lastUpdatedAt = now`, merges input `statusDetails`, records `stepTimeoutInMinutes` (used to compute `approximateSecondsBeforeTimedOut`), increments `versionNumber`, persists, and returns the stored `execution` (`JobExecution` incl. `jobDocument`). A subsequent GetPendingJobExecutions must show the job moved from `queuedJobs` to `inProgressJobs`.
4. **DescribeJobExecution** — `GET /things/{thingName}/jobs/{jobId}`. Reads the persisted execution (no mutation) and returns `execution` with the CURRENT `status`, `statusDetails`, `versionNumber`, `executionNumber`, `queuedAt`/`startedAt`/`lastUpdatedAt`. `includeJobDocument=false` (default) omits `jobDocument`; `includeJobDocument=true` includes it. `executionNumber` selects a specific execution. Unknown `thingName`/`jobId` ⇒ `ResourceNotFoundException` (404). `jobId = $next` resolves to the thing's next QUEUED/IN_PROGRESS execution.
5. **UpdateJobExecution** — `POST /things/{thingName}/jobs/{jobId}`. Applies the required `status` (QUEUED|IN_PROGRESS|SUCCEEDED|FAILED|REJECTED|CANCELED|REMOVED|TIMED_OUT) plus `statusDetails`/`stepTimeoutInMinutes` to the persisted execution, increments `versionNumber`, sets `lastUpdatedAt`. `expectedVersion` mismatch ⇒ `InvalidStateTransitionException` (409) and NO state change. Returns `executionState` (`JobExecutionState{status, statusDetails, versionNumber}`) only when `includeJobExecutionState=true`, and `jobDocument` only when `includeJobDocument=true`. After a terminal status (SUCCEEDED/FAILED/...), the execution must disappear from both lists of GetPendingJobExecutions, and DescribeJobExecution must report the terminal status.

Every response must be computed from persisted state held in the shared engine via `crate::state::service_state(&*self.engine)` — no canned, zeroed or echoed bodies, and no `InMemoryEngine::new()` inside a handler. Delete the `SCAFFOLDED, NOT IMPLEMENTED` marker line from each file you implement.

## State Model

```
ServiceState {
  // thingName -> job executions for that thing, insertion/queue ordered
  things: Map<String /*thingName*/, Vec<JobExecutionRecord>>,
  // clientToken -> executionId, for StartCommandExecution idempotency
  idempotency: Map<String /*clientToken*/, String /*executionId*/>,
  next_execution_seq: u64,
}

JobExecutionRecord {
  job_id: String,              // == executionId minted by StartCommandExecution
  thing_name: String,          // parsed from targetArn (thing/<name>)
  status: JobExecutionStatus,  // QUEUED | IN_PROGRESS | SUCCEEDED | FAILED | REJECTED | CANCELED | REMOVED | TIMED_OUT
  status_details: Map<String, String>,
  queued_at: i64,              // epoch seconds
  started_at: Option<i64>,
  last_updated_at: i64,
  version_number: i64,         // starts at 1, +1 on every device update
  execution_number: i64,
  step_timeout_minutes: Option<i64>,
  execution_timeout_seconds: Option<i64>,
  job_document: String,        // from commandArn + parameters, len 0..32768
  command_arn: String,
}
```

Nothing outside these 5 operations is in scope; the long tail (timeout-driven TIMED_OUT sweeps, CertificateValidationException, ThrottlingException/ServiceUnavailableException injection, TerminalStateException on `$next`) is filed separately later.

## Evidence

OPERATIONS.md for this crate lists exactly 5 operations with the routes/fields quoted above (DescribeJobExecution `GET /things/{thingName}/jobs/{jobId}`; GetPendingJobExecutions `GET /things/{thingName}/jobs`; StartCommandExecution `POST /command-executions`; StartNextPendingJobExecution `PUT /things/{thingName}/jobs/$next`; UpdateJobExecution `POST /things/{thingName}/jobs/{jobId}`). The Smithy model (smithy_query on JobExecution/JobExecutionState/JobExecutionSummary) defines the output fields named in the State Model (jobId, thingName, status, statusDetails, queuedAt, startedAt, lastUpdatedAt, approximateSecondsBeforeTimedOut, versionNumber, executionNumber, jobDocument) and documents that `versionNumber` is incremented on each device update. All five handler files (src/ops/describe_job_execution.rs, get_pending_job_executions.rs, start_command_execution.rs, start_next_pending_job_execution.rs, update_job_execution.rs) are unmodified scaffolds returning 501; no tests exist beyond the generated tests/state_roundtrip.rs.

## Steps to Reproduce

1. POST /command-executions with {"targetArn":"arn:aws:iot:us-east-1:111122223333:thing/laser-1","commandArn":"arn:aws:iot:us-east-1:111122223333:command/reboot","parameters":{...},"clientToken":"tok-1"} -> 200 with an executionId (currently 501).
2. Repeat the identical POST with clientToken "tok-1" -> same executionId, still exactly one execution for the thing.
3. GET /things/laser-1/jobs -> queuedJobs contains a summary whose jobId == that executionId, inProgressJobs empty.
4. PUT /things/laser-1/jobs/$next with {"statusDetails":{"step":"download"},"stepTimeoutInMinutes":10} -> execution returned with status IN_PROGRESS, startedAt set, versionNumber 2, jobDocument present.
5. GET /things/laser-1/jobs -> the job is now in inProgressJobs and absent from queuedJobs.
6. GET /things/laser-1/jobs/<executionId>?includeJobDocument=true -> execution with status IN_PROGRESS, statusDetails {"step":"download"}, versionNumber 2 and the job document.
7. POST /things/laser-1/jobs/<executionId> with {"status":"SUCCEEDED","expectedVersion":2,"includeJobExecutionState":true} -> executionState {status: SUCCEEDED, versionNumber: 3}.
8. Repeat the same POST with expectedVersion 2 -> InvalidStateTransitionException (409) and state unchanged.
9. GET /things/laser-1/jobs -> both lists empty; GET /things/laser-1/jobs/<executionId> -> status SUCCEEDED.
10. GET /things/unknown-thing/jobs/does-not-exist -> ResourceNotFoundException (404).
