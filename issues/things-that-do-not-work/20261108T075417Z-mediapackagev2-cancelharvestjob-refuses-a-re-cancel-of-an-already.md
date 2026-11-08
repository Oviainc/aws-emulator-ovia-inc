Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: mediapackagev2.cancel-harvest-job-unknown-job-name-not-found-v2
Issue-Shape-Id: com.amazonaws.mediapackagev2#mediapackagev2
Issue-Key: mediapackagev2|CancelHarvestJob|ValidationException
Issue-Commit: d0ce8acb2d50b38f212ecb6be47b8b7720e8c1b4

# CancelHarvestJob refuses a re-cancel of an already-CANCELLED harvest job with 400 ValidationException instead of succeeding idempotently

> _Note: this issue was identified against the code base at commit `d0ce8acb2d50b38f212ecb6be47b8b7720e8c1b4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CancelHarvestJob

## Description
REOPENS 20261029T210107Z-mediapackagev2-harvest-job-resourcenotfoundexception-message-wording-diverges-from.md: its fix left the call refused.

That issue claimed CancelHarvestJob fixed (its harvest-job not-found path). The not-found wording now matches — the v1 check mediapackagev2.cancel-harvest-job-unknown-job-name-not-found passes — but the behavioral check that also exercises the cancel of REAL jobs now dies on a CancelHarvestJob call that AWS answers 200 and the emulator refuses 400. (The open issue 20261029T201731Z-mediapackagev2-five-rejection-messages-worded-differently-from-aws.md owns this check id for a MESSAGE-wording cause on the not-found path; it does not cover the refusal described here, which is a different call in the same check and a different code path — the terminal-status guard.)

Cause 1 of 1: an already-CANCELLED harvest job cannot be cancelled again.
  AWS: CancelHarvestJob is marked @idempotent in the model (Smithy: CancelHarvestJob, PUT /channelGroup/{ChannelGroupName}/channel/{ChannelName}/originEndpoint/{OriginEndpointName}/harvestJob/{HarvestJobName}, code 200, idempotent: true; output CancelHarvestJobResponse is empty; errors AccessDeniedException 403, ConflictException 409, InternalServerException 500, ResourceNotFoundException 404, ThrottlingException 429, ValidationException 400). The model publishes HarvestJobStatus values QUEUED | IN_PROGRESS | CANCELLED | COMPLETED | FAILED and a ValidationExceptionType value HARVEST_JOB_INELIGIBLE_FOR_CANCELLATION, so SOME statuses are indeed ineligible — but recorded behavior shows CANCELLED is not one of them: re-cancelling a job that is already CANCELLED is an idempotent no-op that returns the empty 200 response and leaves the job's Status at CANCELLED (a following GetHarvestJob still reports CANCELLED). The ineligible-for-cancellation refusal is for a job that has FINISHED (COMPLETED/FAILED), i.e. a status the cancel cannot reach, not for the status cancel itself produces.
  Emulator: the second CancelHarvestJob on the same job is refused HTTP 400, Error.Code ValidationException, Message "Validation error encountered at request.harvestJobName: A harvest job can be cancelled only before it reaches a terminal status. Verify the status of the harvest job and then try again" (ValidationExceptionType HARVEST_JOB_INELIGIBLE_FOR_CANCELLATION). In that message the only fixed text AWS would own is the requirement sentence; the member path prefix "Validation error encountered at request.harvestJobName" is the variable part taken from the request's rejected member. The emulator emits this because the handler's eligibility test treats CANCELLED as just another terminal status alongside FAILED and COMPLETED, so the state the operation itself writes immediately makes the operation refuse itself.
  Why: a client that retries a cancel — exactly what the @idempotent trait tells SDKs, orchestrators and cleanup/teardown loops they may do after a timeout, a dropped connection or an at-least-once workflow step — gets a 400 ValidationException on the retry instead of the empty 200 AWS returns, and a 400 ValidationException is non-retryable, so the retry is surfaced to the caller as a permanent input error even though the requested end state (job CANCELLED) already holds. Error-handling code written against AWS cannot distinguish "this job finished and can never be cancelled" from "this job is already cancelled", because the emulator answers both with the same refusal.
  Fix: the CancelHarvestJob handler's eligibility check — the cancel path must treat an already-CANCELLED job as satisfied (return the empty success response, leave Status/ModifiedAt/version/ETag untouched so the no-op does not rotate the entity tag) and reserve the HARVEST_JOB_INELIGIBLE_FOR_CANCELLATION ValidationException for jobs that reached COMPLETED or FAILED. The shared harvest-job terminal-status predicate in the service state is the wrong test to gate cancellation on, since it folds in the status cancellation produces.
  Traces: mediapackagev2.cancel-harvest-job-unknown-job-name-not-found-v2.

## Evidence
Behavioral suite simparity-behavioral-aws-v1: mediapackagev2 56/66 (85%), 9 mismatched, 1 errored, 0 skipped, bootstrap_signal=false. Nine of the ten failures are marked OWNED by 20261029T201731Z-mediapackagev2-five-rejection-messages-worded-differently-from-aws.md and were skipped; this one — the only [error] verdict — is not owned and not marked ALREADY FILED.

Check mediapackagev2.cancel-harvest-job-unknown-job-name-not-found-v2 ends with an uncaught ClientError escaping an unguarded call:
  botocore.errorfactory.ValidationException: An error occurred (ValidationException) when calling the CancelHarvestJob operation: Validation error encountered at request.harvestJobName: A harvest job can be cancelled only before it reaches a terminal status. Verify the status of the harvest job and then try again

The check's shape pins where this happens. Its setup helper creates a harvest job and cancels it, then polls GetHarvestJob and asserts the job reached CANCELLED — that assertion is reached and passes, so the FIRST cancel succeeded. The later unguarded call is the deliberate idempotency control, documented in the check as "Re-cancelling a CANCELLED job is an idempotent no-op, so it is not the refusal boundary" and "re-cancelling a CANCELLED job succeeds and leaves it CANCELLED":
    mp.cancel_harvest_job(ChannelGroupName=..., ChannelName=..., OriginEndpointName=..., HarvestJobName=terminal_job)   # recorded: 200, empty body
    status_after = mp.get_harvest_job(...)["Status"]                                                                   # recorded: "CANCELLED"
  expected: HTTP 200, empty CancelHarvestJobResponse, job Status unchanged at CANCELLED
  actual:   HTTP 400, Error.Code ValidationException, ValidationExceptionType HARVEST_JOB_INELIGIBLE_FOR_CANCELLATION, Message as quoted above; the exception aborts the check before its two closing status assertions run.

Emulator source backing the diagnosis (file/line citations only):
  services/service-com-amazonaws-mediapackagev2-mediapackagev2/src/ops/cancel_harvest_job.rs:225-234 — `if job.is_terminal() { return Err(... validation_failed("harvestJobName", INELIGIBLE_FOR_CANCELLATION_REQUIREMENT, VALIDATION_HARVEST_JOB_INELIGIBLE_FOR_CANCELLATION)) }`, with the requirement text at lines 95-97.
  services/service-com-amazonaws-mediapackagev2-mediapackagev2/src/state.rs:1589-1593 — `is_terminal()` returns true for HARVEST_JOB_CANCELLED as well as HARVEST_JOB_FAILED and HARVEST_JOB_COMPLETED, and cancel_harvest_job.rs:236 is the line that sets that very CANCELLED status.

Smithy evidence: CancelHarvestJob idempotent=true, http {PUT, code 200}, output shape CancelHarvestJobResponse (no members); HarvestJobStatus = QUEUED|IN_PROGRESS|CANCELLED|COMPLETED|FAILED; ValidationExceptionType includes HARVEST_JOB_INELIGIBLE_FOR_CANCELLATION. The model states the operation is idempotent and does not say CANCELLED is ineligible; the exact status set that IS ineligible is recorded behavior only.

## Steps to Reproduce
Boot the emulator from this checkout and replay the mediapackagev2 behavioral check mediapackagev2.cancel-harvest-job-unknown-job-name-not-found-v2 (suite simparity-behavioral-aws-v1), or reproduce directly with botocore against the emulator endpoint:

1. CreateChannelGroup, then CreateChannel in it, then CreateOriginEndpoint (ContainerType='TS', HlsManifests=[{'ManifestName':'index'}]).
2. PutOriginEndpointPolicy granting mediapackagev2:HarvestObject/GetObject to mediapackagev2.amazonaws.com, and create an S3 bucket with a policy letting the harvester write to it (only needed so CreateHarvestJob is accepted).
3. CreateHarvestJob with HarvestedManifests={'HlsManifests':[{'ManifestName':'index'}]}, ScheduleConfiguration={'StartTime': now-10min, 'EndTime': now-5min} and Destination={'S3Destination': {...}}.
4. CancelHarvestJob on that job -> 200. Poll GetHarvestJob until Status == 'CANCELLED' (it is).
5. Call CancelHarvestJob on the SAME job a second time (the idempotent retry). Expected: HTTP 200 with an empty body, and a following GetHarvestJob still reporting Status 'CANCELLED'. Observed: HTTP 400 ValidationException / HARVEST_JOB_INELIGIBLE_FOR_CANCELLATION with Message "Validation error encountered at request.harvestJobName: A harvest job can be cancelled only before it reaches a terminal status. Verify the status of the harvest job and then try again".
6. Contrast: let a job's scheduled window elapse so it reaches COMPLETED/FAILED and cancel it — that refusal is correct and must stay.
