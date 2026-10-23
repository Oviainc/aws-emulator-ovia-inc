Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot-jobs-data.illegal-status-transitions-rejected-queued-to-succeeded-allowed iot-jobs-data.stale-expected-version-rejected-leaves-execution-unchanged iot-jobs-data.status-details-replaced-not-merged-and-kept-when-omitted iot-jobs-data.start-next-returns-in-progress-before-next-queued
Issue-Shape-Id: com.amazonaws.iotjobsdataplane#IotLaserThingJobManagerExternalService
Issue-Key: iot-jobs-data.illegal-status-transitions-rejected-queued-to-succeeded-allowed|_rejection#19|AssertionError|raise AssertionError(f"expected a rejection but the call SUCCEEDED, returning {result!r}")
Issue-Key: iot-jobs-data.stale-expected-version-rejected-leaves-execution-unchanged|run#14|AssertionError|assert stale_code == "InvalidRequestException", (
Issue-Key: iot-jobs-data.status-details-replaced-not-merged-and-kept-when-omitted|run#15|AssertionError|assert replaced["statusDetails"] == {"stage": "verify"}, (
Issue-Key: iot-jobs-data.start-next-returns-in-progress-before-next-queued|run#28|AssertionError|assert again["versionNumber"] == first["versionNumber"], (
Issue-Commit: 1bf7f94f5369037d1be2743fe86a59b83a69434a

# IoT Jobs Data: 4 divergences — UpdateJobExecution accepts illegal backward transitions, misclassifies the expectedVersion guard and merges statusDetails; StartNextPendingJobExecution re-stamps an already-started execution

> _Note: this issue was identified against the code base at commit `1bf7f94f5369037d1be2743fe86a59b83a69434a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateJobExecution

## Description
The iot-jobs-data (IoT Jobs data plane) emulator answers these operations, but four independent rules of the job-execution state machine diverge from AWS. This issue bundles FOUR causes; all four must be closed before submitting.

Cause 1 of 4: UpdateJobExecution does not refuse illegal backward status transitions.
AWS: Smithy (com.amazonaws.iotjobsdataplane#UpdateJobExecution) declares InvalidStateTransitionException with httpError 409 on this operation; the enum JobExecutionStatus is declared without any reachability rules, so the legal/illegal pairs are recorded behavior only. Recorded behavior (us-west-2, per the check's MEASURED note): from IN_PROGRESS, an update to QUEUED is refused InvalidStateTransitionException / HTTP 409 (recorded message, as an example: "The status of job execution cannot be changed to be QUEUED"), and an update to REJECTED is likewise refused 409 (recorded example message: "Only queued jobs can be rejected. Job was in status IN_PROGRESS"). The refusal is total — status and versionNumber are unchanged afterwards. QUEUED -> SUCCEEDED directly (skipping IN_PROGRESS) is ACCEPTED, so the rule is not a tidy linear chain: it is specifically the BACKWARD moves out of IN_PROGRESS that are refused.
Emulator: the request IN_PROGRESS -> QUEUED with the correct expectedVersion SUCCEEDS with HTTP 200 (empty body), so the execution is rewound and its version advanced. services/service-com-amazonaws-iotjobsdataplane-iotlaserthingjobmanagerexternalservice/src/ops/update_job_execution.rs checks only "status is in the enum" and "record is terminal"; there is no transition-legality check for a live record.
Why: a device (or a buggy client) can rewind an in-flight execution to QUEUED or REJECTED, so the execution is re-dispatched and the work runs twice; a client that relies on the 409 to detect a lost/duplicated dispatch sees success instead and never reconciles.
Fix: the status-transition validation of UpdateJobExecution's handler (the live-record branch of its handle, alongside the existing is_terminal check).
Traces: iot-jobs-data.illegal-status-transitions-rejected-queued-to-succeeded-allowed.

Cause 2 of 4: a mismatched expectedVersion is reported as InvalidStateTransitionException / 409 instead of InvalidRequestException / 400.
AWS: Smithy declares both InvalidRequestException (httpError 400) and InvalidStateTransitionException (httpError 409) on UpdateJobExecution, and says nothing about the expectedVersion member beyond its type (long). Recorded behavior: the optimistic-concurrency guard answers InvalidRequestException / HTTP 400, for a version from the PAST and one from the FUTURE alike (the guard is an exact match, not a floor); the message is built from the request and the stored version, i.e. of the form "Expected version <requested> but found version <stored>". The refused write leaves status, versionNumber and statusDetails untouched.
Emulator: answers InvalidStateTransitionException (HTTP 409). See update_job_execution.rs, the expected_version branch that returns InvalidStateTransitionException with "expectedVersion {expected} does not match the job execution's current version {n}".
Why: a client cannot tell a concurrency conflict from an illegal transition: the status class is wrong (409 vs 400) and the error code is wrong, so retry logic keyed on InvalidRequestException (re-read version, retry) never fires, while logic keyed on InvalidStateTransitionException (give up, the execution finished) fires on a merely stale version.
Fix: the expectedVersion guard in UpdateJobExecution's handler/validation — it must raise the modeled InvalidRequestException (400) and name both versions.
Traces: iot-jobs-data.stale-expected-version-rejected-leaves-execution-unchanged.

Cause 3 of 4: supplied statusDetails is MERGED into the stored map instead of replacing it.
AWS: Smithy types UpdateJobExecution.statusDetails as DetailsMap (string->string) with no merge semantics declared — recorded behavior only. Recorded: a write carrying {"stage":"verify"} over a stored {"stage":"download","attempt":"1"} leaves ONLY {"stage":"verify"} — the map is replaced wholesale and keys no longer sent are dropped; a later write that OMITS statusDetails entirely preserves the stored map.
Emulator: the stored map keeps the dropped key — the update response (and the subsequent read) report {"attempt":"1","stage":"verify"}. update_job_execution.rs calls record.try_merge_status_details(...), i.e. a per-key merge, for both this operation and StartNextPendingJobExecution.
Why: device state details accumulate forever and stale keys from earlier phases are reported as current, so any consumer reading statusDetails to decide where an execution is sees a phase the device already left.
Fix: the statusDetails application in UpdateJobExecution's handler and the shared state helper it calls — replace the whole map when the member is present, preserve it when the member is absent.
Traces: iot-jobs-data.status-details-replaced-not-merged-and-kept-when-omitted.

Cause 4 of 4: StartNextPendingJobExecution mutates an execution it merely re-hands.
AWS: Smithy (StartNextPendingJobExecution, PUT /things/{thingName}/jobs/$next) returns a JobExecution and declares nothing about re-entry — recorded behavior only. Recorded: with one execution already IN_PROGRESS and another QUEUED, a second call returns the SAME in-progress execution with an UNCHANGED versionNumber and an unchanged startedAt, and leaves the other job in queuedJobs; the next queued execution is handed out only after the in-progress one reaches a terminal status.
Emulator: re-hands the right execution but advances its versionNumber on every call (observed: was 2, now 3). start_next_pending_job_execution.rs increments record.version_number unconditionally after record.start(now), with no distinction between starting a QUEUED execution and resuming one already IN_PROGRESS.
Why: the version the device was handed is immediately stale, so the device's next UpdateJobExecution with that expectedVersion is rejected (and, with cause 2, rejected under the wrong code); every re-poll of $next invalidates the outstanding version, making the optimistic-concurrency protocol unusable for a device that polls.
Fix: the resume path of StartNextPendingJobExecution's handler — only a QUEUED->IN_PROGRESS start may advance the version and stamp startedAt/lastUpdatedAt; a re-hand must be a pure read.
Traces: iot-jobs-data.start-next-returns-in-progress-before-next-queued.

## Evidence
Suite: simparity-behavioral-aws-v1, iot-jobs-data 1/9 passed.

Cause 1 — iot-jobs-data.illegal-status-transitions-rejected-queued-to-succeeded-allowed [mismatch]:
  expected: ClientError InvalidStateTransitionException, HTTP 409 for UpdateJobExecution(status="QUEUED", expectedVersion=<live>) on an IN_PROGRESS execution
  actual:   AssertionError: expected a rejection but the call SUCCEEDED, returning {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/json', 'content-length': '42', ...}}}

Cause 2 — iot-jobs-data.stale-expected-version-rejected-leaves-execution-unchanged [mismatch]:
  expected: error code InvalidRequestException, HTTP 400, message of the form "Expected version <n-1> but found version <n>"
  actual:   AssertionError: a stale expectedVersion must be rejected InvalidRequestException, got 'InvalidStateTransitionException'

Cause 3 — iot-jobs-data.status-details-replaced-not-merged-and-kept-when-omitted [mismatch]:
  expected: statusDetails == {'stage': 'verify'} after a write carrying only {'stage': 'verify'} over {'stage': 'download', 'attempt': '1'}
  actual:   AssertionError: a supplied statusDetails must REPLACE the stored map (dropping 'attempt'), got {'attempt': '1', 'stage': 'verify'}

Cause 4 — iot-jobs-data.start-next-returns-in-progress-before-next-queued [mismatch]:
  expected: the second StartNextPendingJobExecution returns job A at the same versionNumber (2) and same startedAt
  actual:   AssertionError: re-handing an already-started execution must not advance its version: was 2, now 3

Code citations (emulator side):
  services/service-com-amazonaws-iotjobsdataplane-iotlaserthingjobmanagerexternalservice/src/ops/update_job_execution.rs:255-271 (version guard -> InvalidStateTransitionException), :278-291 (only terminal records are refused; no transition-legality check), :296 (try_merge_status_details), :314 (version bump)
  services/service-com-amazonaws-iotjobsdataplane-iotlaserthingjobmanagerexternalservice/src/ops/start_next_pending_job_execution.rs:233-237 (record.start + unconditional version_number += 1)
Smithy consulted: com.amazonaws.iotjobsdataplane#UpdateJobExecution (errors InvalidRequestException 400, InvalidStateTransitionException 409, ResourceNotFoundException 404; members status/statusDetails/expectedVersion), #StartNextPendingJobExecution (PUT /things/{thingName}/jobs/$next, output execution: JobExecution), #DescribeJobExecution (TerminalStateException 410).

## Steps to Reproduce
All four use the same setup: iot.create_thing + iot.create_job targeting the thing ARN, then an iot-jobs-data client bound to the host returned by iot.describe_endpoint(endpointType="iot:Data-ATS"), polling GetPendingJobExecutions/DescribeJobExecution until the execution is QUEUED.

Cause 1: StartNextPendingJobExecution(thingName) -> execution at version 2, IN_PROGRESS. Call UpdateJobExecution(thingName, jobId, status="QUEUED", expectedVersion=2). AWS refuses InvalidStateTransitionException / 409 and leaves the record IN_PROGRESS at version 2; the emulator returns 200. Same for status="REJECTED". Control: a still-QUEUED execution accepts status="SUCCEEDED" directly.

Cause 2: StartNextPendingJobExecution(thingName, statusDetails={"phase":"download"}) -> version 2. Call UpdateJobExecution(..., status="SUCCEEDED", expectedVersion=1) and again with expectedVersion=7. AWS: InvalidRequestException / HTTP 400 both times, naming requested and stored version; the emulator answers InvalidStateTransitionException. Then expectedVersion=2 must be accepted and advance to 3.

Cause 3: StartNextPendingJobExecution(thingName, statusDetails={"stage":"download","attempt":"1"}); then UpdateJobExecution(..., status="IN_PROGRESS", statusDetails={"stage":"verify"}, expectedVersion=<v>, includeJobExecutionState=True). Inspect executionState.statusDetails and re-read with DescribeJobExecution: AWS shows {"stage":"verify"} only. Then UpdateJobExecution(..., status="SUCCEEDED", expectedVersion=<v+1>) with statusDetails OMITTED must still report {"stage":"verify"}.

Cause 4: queue two jobs A then B for one thing (distinct queuedAt seconds). First StartNextPendingJobExecution -> A, IN_PROGRESS, version 2. Call StartNextPendingJobExecution again: AWS returns A at version 2 with the same startedAt and leaves B in queuedJobs; the emulator returns A at version 3. After A is driven to SUCCEEDED, the next call must hand out B.
