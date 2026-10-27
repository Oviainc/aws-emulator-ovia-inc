Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 65fd8797851bb5c5937d1ea0792f5c718c73df47

# Jobs only WAITING in the queue count against the concurrency ceiling, so a start is refused LimitExceededException while nothing is being processed

_Filed: 2026-10-27T22:23:09Z_
_Operation: StartTranscriptionJob | Class: error-response_

> _Note: this issue was identified against the code base at commit `65fd8797851bb5c5937d1ea0792f5c718c73df47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The job-family concurrency ceiling is decided by `crate::state::admit_start`, which compares `jobs_in_flight(..) < ceiling`. `jobs_in_flight` counts EVERY record whose status is not terminal, and `is_terminal_job_status` is true only for `COMPLETED`/`FAILED` — so a record in `STATUS_QUEUED` (a job the service has accepted but is NOT working on yet) is counted as occupying one of the ceiling's slots.

That contradicts the crate's own statement of the rule in two places: `jobs_waiting_in_queue`'s doc says a queued job is in flight "without occupying one of the slots a family's concurrency ceiling bounds", and `StartTranscriptionJob`'s own handler comment says "a waiting job holds no running slot, so it is bounded by the queue's own depth". If a waiting job holds no running slot, it must not be what makes a later start exceed the ceiling.

Because of this, a start that opted into nothing (no `JobExecutionSettings.AllowDeferredExecution`) is refused with `LimitExceededException` purely on the strength of records that are only WAITING — i.e. a valid request is turned into a quota error while the Region is processing fewer jobs than the ceiling (in the extreme, none at all, since a queued record leaves the queue only one `JOB_TRANSITION_DELAY` after its own start while the records that filled the ceiling complete earlier). The same code path serves `StartCallAnalyticsJob`, the other family whose start documents a queue.

The fix is to count only the records the service is actually working on (the non-terminal, non-`QUEUED` ones) against the ceiling, leaving the queue bounded by its own depth exactly as the comments describe; nothing about the queue-admission path or its depth bound needs to change.

## Evidence

The rule as the crate states it — services/service-com-amazonaws-transcribe-transcribe/src/state.rs:313-317 (doc of `jobs_waiting_in_queue`):
  "A queued job is in flight ([`jobs_in_flight`]) without occupying one of the slots a
   family's concurrency ceiling bounds, so for a family whose start documents a queue it
   is this count, not that ceiling, that caps how many records one caller can hold
   unfinished at once"
and services/service-com-amazonaws-transcribe-transcribe/src/ops/start_transcription_job.rs:536-538:
  "a waiting job holds no running slot, so it is bounded by the queue's
   own depth, [`crate::state::MAX_QUEUED_TRANSCRIPTION_JOBS`]"

The code that breaks it — state.rs:290-305 (`jobs_in_flight`, which counts every non-terminal record):
  "    for record in jobs.values_in_mut(region) {
           record.materialise_due_transitions(now);
           if record
               .status()
               .is_some_and(|status| !is_terminal_job_status(status))
           {
               in_flight += 1;"
with state.rs:172-174:
  "pub fn is_terminal_job_status(status: &str) -> bool {
       status == STATUS_COMPLETED || status == STATUS_FAILED
   }"
(so `STATUS_QUEUED`, state.rs:148, counts as in flight), and state.rs:371-374 (`admit_start`):
  "    if jobs_in_flight(jobs, region, now) < ceiling {
           return StartAdmission::Process;
       }"
which start_transcription_job.rs:547-564 renders as `LimitExceededException` when it answers `Refuse`.

Existing coverage: tests/all/transcription_job_queueing.rs only fills the ceiling with RUNNING jobs (`fill_to_the_ceiling`, line 227) and then checks the queue's own depth (`the_queue_itself_has_a_bound`, line 402); no test pins that a WAITING record may exhaust the ceiling for a later non-deferred start, so no stated reason justifies the divergence.

## Steps to Reproduce

1. Start `crate::state::MAX_CONCURRENT_TRANSCRIPTION_JOBS` transcription jobs quickly so the Region is at its ceiling (as tests/all/transcription_job_queueing.rs `fill_to_the_ceiling` does).
2. Start that many more jobs carrying `JobExecutionSettings` with `AllowDeferredExecution: true` and a `DataAccessRoleArn`; each is accepted in `QUEUED` (the queue's depth is `MAX_QUEUED_TRANSCRIPTION_JOBS`).
3. Let the clock complete the first batch (one `JOB_TRANSITION_DELAY`) while the queued records, whose `QUEUED -> IN_PROGRESS` instant is one delay after their own later start, are still `QUEUED` — `ListTranscriptionJobs` with `Status: QUEUED` shows them and no job is `IN_PROGRESS`.
4. Call `StartTranscriptionJob` with no `JobExecutionSettings` at all.
5. Observe `LimitExceededException` ("You have reached the limit of N concurrent transcription jobs…") even though no job is being processed: `jobs_in_flight` counted the waiting records, contradicting "a waiting job holds no running slot".
