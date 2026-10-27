Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 5b028d5a37462f071f2cc1a7c0f151a69190cfd2

# A start with JobExecutionSettings.AllowDeferredExecution=true is refused LimitExceededException instead of being queued

_Filed: 2026-10-25T00:40:41Z_
_Operation: StartTranscriptionJob | Class: error-response_

> _Note: this issue was identified against the code base at commit `5b028d5a37462f071f2cc1a7c0f151a69190cfd2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`StartTranscriptionJob` applies its concurrency ceiling unconditionally: if the family already has `MAX_CONCURRENT_TRANSCRIPTION_JOBS` jobs in flight, the handler answers `LimitExceededException` and stores nothing, whatever the request asked for. The model documents the opposite for a request that opts into job queueing: the whole point of `JobExecutionSettings.AllowDeferredExecution` is that such a request is QUEUED when the concurrent request limit is exceeded, and that `LimitExceededException` is what the caller gets when the flag is NOT set.

The handler already computes the flag a few lines later (`let queued = input.job_execution_settings...allow_deferred_execution.unwrap_or(false)`, start_transcription_job.rs:499-503) to decide the job's initial status, so the value is available at the point of refusal — it is simply not consulted there. The consequence is that the one member whose documented purpose is to survive a full concurrency quota is the one member that cannot: a deferred start at the limit fails exactly like a non-deferred one, and nothing of it is stored.

This is a different defect from the already-fixed issue 20261024T202208Z-transcribe-starttranscriptionjob-error-response (which was about terminal records being counted against the ceiling): the counting is now correct (`jobs_in_flight`), but the refusal still ignores deferred execution.

Secondarily, the queueing this emulator does perform is time-based rather than limit-based: a deferred job is QUEUED for `JOB_TRANSITION_DELAY` even when no other job is in flight, and a job that was refused is never queued at all.

## Evidence

Smithy (model doc for `JobExecutionSettings$AllowDeferredExecution`, `smithy_query(query="shape", shape="JobExecutionSettings")`): "Makes it possible to enable job queuing when your concurrent request limit is exceeded. When <code>AllowDeferredExecution</code> is set to <code>true</code>, transcription job requests are placed in a queue until the number of jobs falls below the concurrent request limit. If <code>AllowDeferredExecution</code> is set to <code>false</code> and the number of transcription job requests exceed the concurrent request limit, you get a <code>LimitExceededException</code> error."

Handler, `services/service-com-amazonaws-transcribe-transcribe/src/ops/start_transcription_job.rs:481-493` — the refusal reads only the in-flight count, never the request's deferral flag:
```
        if crate::state::jobs_in_flight(&mut store.transcription_jobs, now)
            >= crate::state::MAX_CONCURRENT_TRANSCRIPTION_JOBS
        {
            return Err(StartTranscriptionJobErrorModeled::LimitExceededException(
```
The comment above it (lines 473-480) justifies only the concurrency reading ("A start made while that many jobs are in flight is refused with the error this operation models for an exceeded quota") and says nothing about deferred execution.

The flag IS read, 6 lines later, for the initial status only — `start_transcription_job.rs:499-503`:
```
        let queued = input
            .job_execution_settings
            .as_ref()
            .and_then(|settings| settings.allow_deferred_execution)
            .unwrap_or(false);
```

Same crate, opposite side (kind B): `src/state.rs:365-368` quotes the very sentence the handler breaks — "the model says the same from the other side, since `JobExecutionSettings$AllowDeferredExecution` queues a request \"until the number of jobs falls below the concurrent request limit\"" — while the handler queues nothing in that situation.

Test coverage: `tests/all/job_concurrency_quota.rs:301-342` pins the current behaviour, and it does so with a DEFERRED request: `family.start_job(&transcribe, "over-the-limit", 0)` uses index 0, and `transcription_request` adds `JobExecutionSettings {AllowDeferredExecution: true, DataAccessRoleArn: ..}` whenever `index.is_multiple_of(5)` (`tests/all/job_concurrency_quota.rs:85-100`), then asserts `refused.refused(&case, 400, "LimitExceededException")` (line 309). The test states no reason for refusing a deferred start specifically — its stated intent is only that "the family's queued-then-processing path is counted against the quota too" (lines 82-84) — so this row needs updating with the fix.

## Steps to Reproduce

1. Start `crate::state::MAX_CONCURRENT_TRANSCRIPTION_JOBS` (100) transcription jobs with distinct names and do not wait for them to complete: POST / with `X-Amz-Target: Transcribe.StartTranscriptionJob`, body {"TranscriptionJobName":"job-N","LanguageCode":"en-US","Media":{"MediaFileUri":"s3://DOC-EXAMPLE-BUCKET/audio.wav"}} — all answer 200 and are IN_PROGRESS
2. POST / with `X-Amz-Target: Transcribe.StartTranscriptionJob` and body {"TranscriptionJobName":"deferred-1","LanguageCode":"en-US","Media":{"MediaFileUri":"s3://DOC-EXAMPLE-BUCKET/audio.wav",},"JobExecutionSettings":{"AllowDeferredExecution":true,"DataAccessRoleArn":"arn:aws:iam::123456789012:role/Admin"}}
3. Observe: the emulator answers HTTP 400 with __type `LimitExceededException` ("You have reached the limit of 100 concurrent transcription jobs for this account...") and stores nothing; the model documents that a request with AllowDeferredExecution=true is instead placed in a queue until the number of jobs falls below the limit
4. POST / with `X-Amz-Target: Transcribe.GetTranscriptionJob` and body {"TranscriptionJobName":"deferred-1"} — the emulator answers BadRequestException (no such job), so the deferred request was dropped rather than queued
