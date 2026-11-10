Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: amplify.stopjob-completed-job-rejected
Issue-Shape-Id: com.amazonaws.amplify#Amplify
Issue-Key: amplify.stopjob-completed-job-rejected|run#16|AssertionError|assert err == (
Issue-Commit: 11f4c8cad091e31e006092cf5180555ad0976c5e

# Amplify StopJob refuses an already-terminal job with a non-AWS message ('Job <id> is not in progress.')

> _Note: this issue was identified against the code base at commit `11f4c8cad091e31e006092cf5180555ad0976c5e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StopJob

## Description
One net-new root cause in the amplify service, grounded in a failing behavioral check.

Cause 1 of 1: StopJob on a job that has already reached a terminal status is refused with the right status and the right error code but the WRONG message, and the emulator's message interpolates a value AWS's message does not contain.

  AWS: Smithy (services/service-com-amazonaws-amplify-amplify/smithy-model/com-amazonaws-amplify-amplify.json) models StopJob -> DELETE /apps/{appId}/branches/{branchName}/jobs/{jobId}/stop, success 200 with `jobSummary` @required, documented as "Stops a job that is in progress for a branch of an Amplify app"; modeled errors BadRequestException (HTTP 400, "A request contains unexpected data"), NotFoundException (404), UnauthorizedException (401), LimitExceededException (429), InternalFailureException (500). The model does not state the message text, so the message is recorded behavior only: stopping a job whose status is already terminal (CANCELLED / SUCCEED / FAILED) is refused with HTTP 400, error code `BadRequestException`, message "Unable to cancel job. The given job is already in a terminal state." That message is ENTIRELY fixed text — it names neither the jobId, the branch, nor the status, so nothing in it comes from the request or from state.

  Emulator: same HTTP 400 and same `BadRequestException` code, but message "Job 1 is not in progress." The fixed part of the emulator's template is "Job " ... " is not in progress." with the request's `jobId` interpolated (the recorded run's `1` is just that run's generated job id and is not part of any requirement). The refusal is produced in services/service-com-amazonaws-amplify-amplify/src/ops/stop_job.rs:189-194 from the shared helper `crate::state::job_not_in_progress_message(&input.job_id)` in services/service-com-amazonaws-amplify-amplify/src/state.rs. The decision to refuse is correct (the handler settles the clock first, then refuses a terminal job and leaves it untouched) — only the wording is invented.

  Why: `BadRequestException` is an untyped catch-all that Amplify reuses for every 400 on this surface (wrong job type, unconnected repository, missing upload, not-cancellable job), so the MESSAGE is the only discriminator a client has. A client that branches on "is already in a terminal state" (to stop polling, to treat the cancel as a no-op, or to surface the real reason to a user) sees an unknown string and falls through to generic-error handling; log/alarm matching on AWS's literal never fires. The emulator additionally teaches callers that this message carries the job id, which AWS's does not, so any parser written against the emulator breaks on real AWS.

  Fix: the StopJob handler's terminal-state refusal path (services/service-com-amazonaws-amplify-amplify/src/ops/stop_job.rs, the `is_terminal_job_status` branch) and the message helper it calls in src/state.rs must emit AWS's fixed literal for this error class instead of a per-job template. If that helper is shared with another operation whose AWS message really is job-scoped, this call site needs its own literal rather than the shared one.

  Traces: amplify.stopjob-completed-job-rejected (group overly-permissive). Context, not coverage: the check previously failed EARLIER in the same flow, at the CreateDeployment zipUploadUrl PUT, which the now-fixed issue 20261029T185952Z-amplify-manual-deployment-data-plane-the-zipuploadurl-handed.md carried; that fix let the flow proceed and exposed this message divergence, which no open issue carries (the open amplify issues cover ListBackendEnvironments/ListWebhooks/UntagResource stubs, the wire job/deployment-start cluster 20261029T215038Z, and the hosting-edge customRules timeout 20261029T122518Z — none of them touch StopJob's refusal text). This is not a REOPENS: the call being refused here is the CORRECT behavior, and the previously fixed issue named different operations (CreateDeployment, StartJob).

## Evidence
Failing check: [mismatch] amplify.stopjob-completed-job-rejected
Suite detail: `AssertionError: ('BadRequestException', 'Job 1 is not in progress.')` raised at check_amplify_stopjob_completed_job_rejected.py:44 (`assert err == (...)`).

The positive controls in the same check PASS, so the divergence is isolated to the refusal text: StopJob on the in-flight job returned a jobSummary with status in (CANCELLING, CANCELLED), and GetJob polling then settled the job to CANCELLED.

Expected (real AWS, recorded): HTTP 400, Error.Code `BadRequestException`, Error.Message `Unable to cancel job. The given job is already in a terminal state.` — all fixed text, no part of it taken from the request or state.

Actual (emulator): HTTP 400, Error.Code `BadRequestException`, Error.Message `Job 1 is not in progress.` — fixed text `Job ` + <the request's jobId> + ` is not in progress.`; `1` is the per-run generated job id and is not itself the defect.

## Steps to Reproduce
1. amplify.create_app(name=<unique>) -> appId; amplify.create_branch(appId=appId, branchName=<unique>).
2. d = amplify.create_deployment(appId=appId, branchName=branchName); HTTP PUT a zip containing index.html to d['zipUploadUrl'] (expect 200/204).
3. amplify.start_deployment(appId=appId, branchName=branchName, jobId=d['jobId']).
4. amplify.stop_job(appId=appId, branchName=branchName, jobId=d['jobId']) -> expect 200 with jobSummary.status in (CANCELLING, CANCELLED). (Emulator agrees.)
5. Poll amplify.get_job(appId, branchName, jobId=d['jobId']) until job.summary.status is terminal -> expect CANCELLED. (Emulator agrees.)
6. Call amplify.stop_job(appId=appId, branchName=branchName, jobId=d['jobId']) AGAIN, now that the job is terminal.
   Observed: 400 BadRequestException 'Job <jobId> is not in progress.'
   Expected: 400 BadRequestException 'Unable to cancel job. The given job is already in a terminal state.'
7. amplify.get_job(...) -> the job must still report CANCELLED (the rejected call changes nothing).

Replay: run_behavioral_suite and re-run check amplify.stopjob-completed-job-rejected.
