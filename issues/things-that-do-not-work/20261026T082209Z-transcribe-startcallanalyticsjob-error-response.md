Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: b827135cf833dfec17047e08babbbec809d0cc25

# A StartCallAnalyticsJob carrying DataAccessRoleArn is refused LimitExceededException at the concurrency limit instead of being queued

_Filed: 2026-10-26T08:22:09Z_
_Operation: StartCallAnalyticsJob | Class: error-response_

> _Note: this issue was identified against the code base at commit `b827135cf833dfec17047e08babbbec809d0cc25`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`StartCallAnalyticsJob`'s own modelled documentation states that this family has job queueing, and names the request member that opts into it: passing `DataAccessRoleArn` turns "you are over the concurrent job limit" from a refusal into an accepted job that waits in a queue. The handler never reads `DataAccessRoleArn` for this purpose: it counts the jobs in flight and, at the ceiling, returns the modelled `LimitExceededException` for EVERY start, including one that passed a `DataAccessRoleArn`. So a caller that did exactly what the operation documents as the way to be queued gets an error instead of a `QUEUED` job.

The crate's own `CallAnalyticsJobRecord` documentation acknowledges the documented queue and dismisses it with a statement the model contradicts — "which no member of the request opts into" — while the model sentence names `DataAccessRoleArn` as that member. The record therefore carries no `queued_until` instant and the family's `advance_clock` has no `QUEUED -> IN_PROGRESS` edge, even though `CallAnalyticsJobStatus` models `QUEUED` and the batch transcription family in this same crate already implements exactly that path (`STATUS_QUEUED`, `queued_until`, `start_time: None`).

## Evidence

Smithy model, `smithy_query(query="doc", shape="StartCallAnalyticsJob")` — the operation's own documentation:
  "Job queuing is available for Call Analytics jobs. If you pass a DataAccessRoleArn in your request and you exceed your Concurrent Job Limit, your job will automatically be added to a queue to be processed once your concurrent job count is below the limit."

`CallAnalyticsJobStatus` models the state such a job would be in — `smithy_query(query="shape", shape="CallAnalyticsJobStatus")`: `["QUEUED", "IN_PROGRESS", "FAILED", "COMPLETED"]`.

Handler — the refusal is unconditional; `input.data_access_role_arn` is only ever stored on the record (line 398), never consulted here. services/service-com-amazonaws-transcribe-transcribe/src/ops/start_call_analytics_job.rs:355-372:
```
        // The job quota is a CONCURRENCY one, per account per Region as it is for every
        // job family here: only the jobs this REGION is still working on count against
        // `crate::state::MAX_CONCURRENT_CALL_ANALYTICS_JOBS`, ...
        if crate::state::jobs_in_flight(&mut store.call_analytics_jobs, &region, now)
            >= crate::state::MAX_CONCURRENT_CALL_ANALYTICS_JOBS
        {
            return Err(StartCallAnalyticsJobErrorModeled::LimitExceededException(
```

The crate's stated reason for not queueing, which the model sentence above contradicts — services/service-com-amazonaws-transcribe-transcribe/src/state.rs:3220-3227:
```
/// `StartCallAnalyticsJob` has no deferred-execution member
/// (`smithy_query(query="operation", operation="StartCallAnalyticsJob")`) — the queue
/// it documents is entered only when an account "exceeds your Concurrent Job Limit",
/// which no member of the request opts into — so a job this emulator accepts is
/// processing from the moment it is started and has exactly one clock-owed
/// transition, `IN_PROGRESS` -> `COMPLETED`, like the Medical Scribe family
/// ([`MedicalScribeJobRecord`]); a start made while that concurrent limit is already
/// reached is refused instead ([`MAX_CONCURRENT_CALL_ANALYTICS_JOBS`]).
```

The machinery the crate already has for the documented queue, in a sibling family — services/service-com-amazonaws-transcribe-transcribe/src/ops/start_transcription_job.rs:518-538 (`let (status, start_time, queued_until, completion_due) = if queued { (crate::state::STATUS_QUEUED.to_string(), None, Some(starts_at), ...) }`) and services/service-com-amazonaws-transcribe-transcribe/src/state.rs:2591-2600 (`pub fn advance_clock(&mut self, now: SystemTime) { if self.status == STATUS_QUEUED { ... self.status = STATUS_IN_PROGRESS.to_string(); ...`).

Test coverage: nothing pins the documented case. The quota suite's Call Analytics row deliberately builds a request WITHOUT a role — services/service-com-amazonaws-transcribe-transcribe/tests/all/job_concurrency_quota.rs:119-127: "/// A Call Analytics start, the family's minimal request plus the varying media." with `json!({"CallAnalyticsJobName": name, "MediaFormat": format, "Media": {"MediaFileUri": uri}})` — so `a_start_at_the_concurrency_limit_is_refused_until_a_job_finishes` (tests/all/job_concurrency_quota.rs:301-342) asserts the refusal only for a role-less start, and the role-carrying rows in tests/all/call_analytics_jobs.rs:90,112,135 are all started with the family idle.

## Steps to Reproduce

1. Start `state::MAX_CONCURRENT_CALL_ANALYTICS_JOBS` Call Analytics jobs in one Region and do not wait for them to complete (each stays IN_PROGRESS for JOB_TRANSITION_DELAY = 500 ms).
2. Within that window call `StartCallAnalyticsJob` with `CallAnalyticsJobName="queued-call"`, `Media={"MediaFileUri":"s3://example-media/call.flac"}` and `DataAccessRoleArn="arn:aws:iam::111122223333:role/CallAnalyticsAccess"`.
3. Observed: HTTP 400 `LimitExceededException` ("You have reached the limit of N concurrent Call Analytics jobs ..."), and `GetCallAnalyticsJob` for "queued-call" answers `BadRequestException` — nothing was stored.
4. Expected per the operation's documentation sentence: because the request passed a `DataAccessRoleArn`, the start is accepted and the job is added to a queue — readable as `CallAnalyticsJobStatus=QUEUED` with no `StartTime` — and begins processing once the concurrent job count falls below the limit, exactly as a `StartTranscriptionJob` with `JobExecutionSettings.AllowDeferredExecution=true` is modelled to behave in the sibling family.
5. Contrast: the identical request WITHOUT `DataAccessRoleArn` is correctly refused, so the two paths are currently indistinguishable.
