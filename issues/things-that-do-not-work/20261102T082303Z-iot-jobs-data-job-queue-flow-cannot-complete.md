Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot-jobs-data.start-next-returns-in-progress-before-next-queued
Issue-Shape-Id: com.amazonaws.iotjobsdataplane#IotLaserThingJobManagerExternalService
Issue-Key: iot-jobs-data|CancelJob|NotImplemented
Issue-Commit: 86ceea0f39fca17f15a7090d9d9227e10094079b

# iot-jobs-data job-queue flow cannot complete: CancelJob answers NotImplemented (501) instead of cancelling the job (1 cause)

> _Note: this issue was identified against the code base at commit `86ceea0f39fca17f15a7090d9d9227e10094079b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CancelJob

## Description
Cause 1 of 1 — CancelJob is an unimplemented scaffold stub, so the job-execution queue flow exercised from the iot-jobs-data data plane cannot be driven or unwound.

Failing check: iot-jobs-data.start-next-returns-in-progress-before-next-queued (service token taken from the check id's leading segment, iot-jobs-data; the call that is refused is the iot control-plane operation CancelJob that this data-plane check drives).

AWS: Smithy (services/service-com-amazonaws-iot-awsiotservice/smithy-model/com-amazonaws-iot-awsiotservice.json) models CancelJob as PUT /jobs/{jobId}/cancel with httpLabel jobId (@required, @length 1..64, @pattern ^[a-zA-Z0-9_-]+$), httpQuery force (boolean, default false), optional reasonCode (@length 0..128, @pattern ^[\p{Upper}\p{Digit}_]+$) and comment (@length 0..2028). Success is HTTP 200 with CancelJobResponse {jobArn, jobId, description}. The only modelled failures are InvalidRequestException (400), ThrottlingException (400), ResourceNotFoundException (404), LimitExceededException (410) and ServiceUnavailableException (503); there is no NotImplemented error and no 501 on this operation. Recorded behavior in the check: cancelling a job (force=true, comment supplied) is the documented way to move a job whose per-target executions are still QUEUED/IN_PROGRESS out of a live state so the job can subsequently be deleted — the check's _delete_job helper calls CancelJob and tolerates only ResourceNotFoundException from it.

Emulator: the call is refused with error code "NotImplemented" and message "operation CancelJob is scaffolded but not implemented in this emulator". Fixed text of that body: "operation " + <operation name> + " is scaffolded but not implemented in this emulator" — i.e. the whole message is emulator-authored prose, and the error code is not one of the operation's modelled error shapes. The check aborts on this ClientError (verdict: error), so none of its queue-ordering assertions (StartNextPendingJobExecution re-handing the IN_PROGRESS execution at an unchanged versionNumber/startedAt, and the next QUEUED job becoming reachable only once the first execution is terminal) were ever reached.

Why: a client that creates jobs and dispatches them through the iot-jobs-data plane has no way to cancel one. Because DeleteJob refuses a job that still has non-terminal executions unless it is cancelled first, every such job and its executions leak in emulator state, and no retry/compensation path exists: the client receives an error code (NotImplemented) that is not in the operation's modelled error set, so the SDK exposes it as a generic ClientError rather than one of InvalidRequestException/ResourceNotFoundException/LimitExceededException that client code handles. Any test or tooling that follows the AWS job lifecycle (CreateJob -> dispatch -> CancelJob -> DeleteJob) stalls at the cancel step.

Fix: write the CancelJob handler in the iot service crate's job operations (it is currently the generated scaffold that returns the NotImplemented stub) together with the job/job-execution state transition it owns: validate jobId against its length/pattern, 404 ResourceNotFoundException for an unknown jobId, move the job to CANCELED and — with force=true — transition its live QUEUED/IN_PROGRESS executions to CANCELED (recording reasonCode/comment), and serialize the 200 CancelJobResponse {jobArn, jobId, description}. Note the open issue 20261102T000217Z-bootstrap-service-com-amazonaws-iot-awsiotservice.md names UpdateThing as the thing-registry's unwritten handler and does not cover CancelJob, so this is not covered by it.

Traces: iot-jobs-data.start-next-returns-in-progress-before-next-queued.

## Evidence
Expected (AWS / Smithy model): CancelJob -> HTTP 200, body CancelJobResponse {jobArn, jobId, description}; the job and, with force=true, its non-terminal executions become CANCELED, after which DeleteJob succeeds. Modelled errors only: InvalidRequestException 400, ThrottlingException 400, ResourceNotFoundException 404, LimitExceededException 410, ServiceUnavailableException 503.

Actual (emulator, replay of iot-jobs-data.start-next-returns-in-progress-before-next-queued):
  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CancelJob operation: operation CancelJob is scaffolded but not implemented in this emulator
  (raised from client.py _make_api_call; check verdict = error, so every subsequent assertion in the check was never evaluated)

Message anatomy: fixed text "operation " ... " is scaffolded but not implemented in this emulator" with the operation name as its only variable part — this whole body is emulator prose; AWS emits no such message or error code for CancelJob.

## Steps to Reproduce
1. Boot the emulator and resolve the data endpoint: iot.describe_endpoint(endpointType="iot:Data-ATS"), then point an iot-jobs-data client at it.
2. iot.create_thing(thingName=<thing>); take its thingArn from describe_thing.
3. iot.create_job(jobId=<job>, targets=[thingArn], document='{"op":"first"}') and poll iot-jobs-data get_pending_job_executions(thingName=<thing>) until <job> appears in queuedJobs.
4. Call iot.cancel_job(jobId=<job>, force=True, comment="cleanup").
   Expected: HTTP 200 with {jobArn, jobId}; the job's queued execution becomes CANCELED and iot.delete_job(jobId=<job>, force=True) then succeeds.
   Actual: ClientError (NotImplemented) "operation CancelJob is scaffolded but not implemented in this emulator"; the job and its execution stay live.
5. Replay check iot-jobs-data.start-next-returns-in-progress-before-next-queued to confirm the fix.
