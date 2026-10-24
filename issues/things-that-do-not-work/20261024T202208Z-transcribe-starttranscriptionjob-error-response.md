Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 225464d7b482e7fae5afe1559ca2b0fceec44ba9

# Job quotas count COMPLETED jobs, so a start is refused LimitExceededException where AWS's concurrency quota allows it

_Filed: 2026-10-24T20:22:08Z_
_Operation: StartTranscriptionJob | Class: error-response_

> _Note: this issue was identified against the code base at commit `225464d7b482e7fae5afe1559ca2b0fceec44ba9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The per-account job ceilings in `crate::state` are applied to the TOTAL number of stored job records, including terminal (COMPLETED) ones, instead of to the number of jobs actually in flight.

Expected (AWS): the published Amazon Transcribe quota that answers `LimitExceededException` is "Concurrent batch transcription jobs" — a CONCURRENCY limit (100 per account per Region). A job that has reached a terminal status no longer consumes it; finished jobs stay listable (AWS retains job records for 90 days) and a new `StartTranscriptionJob` succeeds. So a caller that starts 100 jobs, polls each to `COMPLETED`, and then starts a 101st gets HTTP 200 and a new job from AWS.

Actual (emulator): `StartTranscriptionJob` refuses with `LimitExceededException` as soon as `store.transcription_jobs.len() >= MAX_TRANSCRIPTION_JOBS` (100), regardless of each record's status. Every stored record — `QUEUED`, `IN_PROGRESS` and `COMPLETED` alike — counts, so once 100 jobs have ever been started and not explicitly deleted, every further start of that family fails permanently. The same counting is used for the other job families (`MAX_MEDICAL_TRANSCRIPTION_JOBS`, `MAX_CALL_ANALYTICS_JOBS`, `MAX_MEDICAL_SCRIBE_JOBS`), and for the Call Analytics family it is unrecoverable in this emulator because `DeleteCallAnalyticsJob` is still a scaffold stub: nothing can free a slot, so after 100 starts `StartCallAnalyticsJob` can never succeed again in the process's lifetime.

The fix is to count only non-terminal records against the ceiling (and/or to bound the store by evicting terminal records), not to remove the quota: the error itself is correctly modelled, it is just raised for requests the service accepts.

## Evidence

Spec: `StartTranscriptionJob` / `StartMedicalTranscriptionJob` / `StartCallAnalyticsJob` / `StartMedicalScribeJob` all model `LimitExceededException` ("You've either sent too many requests or your input file is too long"), i.e. the error exists for an exceeded quota — but the published figure the emulator cites is the CONCURRENT batch-job quota, not a cap on how many job records an account may hold; `ListTranscriptionJobs` is documented to list jobs with status `COMPLETED`, which only makes sense if completed jobs persist without blocking new ones.

Handler/state: `src/ops/start_transcription_job.rs` lines 383-398 (`if store.transcription_jobs.len() >= crate::state::MAX_TRANSCRIPTION_JOBS { .. LimitExceededException .. }`) — the check reads only the map's length, never a record's `status`. Same pattern in `src/ops/start_medical_transcription_job.rs` lines 367-385, `src/ops/start_call_analytics_job.rs` lines 333-348 and `src/ops/start_medical_scribe_job.rs` lines 344-359. `src/state.rs` lines 198-226 state the intent explicitly ("Applying that published figure to the stored jobs keeps the store bounded ... `DeleteTranscriptionJob` is what makes room again"), and line 224-225 concedes that `DeleteCallAnalyticsJob` "is still a scaffold stub", so that family's ceiling cannot be relieved at all.

Tests: `tests/all/client_error_wire_status.rs` lines 421-456 exercise a quota only for `CreateVocabularyFilter` (a family whose published quota IS a total count, so that row is correct). No test starts jobs past the job ceiling, so no test asserts either the current or the AWS behaviour.

## Steps to Reproduce

1. POST / with `X-Amz-Target: Transcribe.StartTranscriptionJob` 100 times, each with a distinct `TranscriptionJobName` and body {"LanguageCode":"en-US","Media":{"MediaFileUri":"s3://DOC-EXAMPLE-BUCKET/audio.wav"}} — all answer 200
2. Wait past the completion transition (~500 ms) and confirm via `Transcribe.ListTranscriptionJobs` {"Status":"COMPLETED"} that all 100 jobs are terminal (no job is in flight)
3. POST / with `X-Amz-Target: Transcribe.StartTranscriptionJob` and a 101st distinct name: AWS answers 200 and creates the job (nothing is concurrently running, so the concurrency quota is free); the emulator answers HTTP 400 with __type `LimitExceededException` and the message "You have reached the limit of 100 transcription jobs for this account..." and stores nothing
4. Repeat the same sequence with `Transcribe.StartCallAnalyticsJob`: after 100 completed Call Analytics jobs every further start is refused, and because `Transcribe.DeleteCallAnalyticsJob` answers 501 (scaffold stub) no slot can be freed, so the operation stays permanently unusable
