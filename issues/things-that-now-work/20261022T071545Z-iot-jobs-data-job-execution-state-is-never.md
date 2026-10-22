Issue-Source: trace-sweep-divergence
Issue-Trace: iotjobsdata-job-execution-queue-poll-and-start
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iotjobsdata-job-execution-queue-poll-and-start iotjobsdata-absent-thing-permissive-queue-reads iotjobsdata-job-execution-update-and-terminal-refusals
Issue-Shape-Id: com.amazonaws.iotjobsdataplane#IotLaserThingJobManagerExternalService
Issue-Key: iot-jobs-data|GetPendingJobExecutions|2xx->2xx|$.queuedJobs[*]
Issue-Key: iot-jobs-data|GetPendingJobExecutions|2xx->2xx|$.queuedJobs[*].executionNumber
Issue-Key: iot-jobs-data|StartNextPendingJobExecution|2xx->2xx|$
Issue-Key: iot-jobs-data|GetPendingJobExecutions|2xx->2xx|$.inProgressJobs[*]
Issue-Key: iot-jobs-data|DescribeJobExecution|2xx->4xx|$.execution
Issue-Key: iot-jobs-data|DescribeJobExecution|2xx->4xx|status
Issue-Key: iot-jobs-data|DescribeJobExecution|2xx->4xx|$
Issue-Key: iot-jobs-data|UpdateJobExecution|2xx->4xx|$
Issue-Key: iot-jobs-data|UpdateJobExecution|2xx->4xx|status
Issue-Key: iot-jobs-data|UpdateJobExecution|2xx->4xx|$.executionState
Issue-Key: iot-jobs-data|UpdateJobExecution|2xx->4xx|$.jobDocument
Issue-Key: iot-jobs-data|DescribeJobExecution|4xx->4xx|$.message
Issue-Key: iot-jobs-data|DescribeJobExecution|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: iot-jobs-data|DescribeJobExecution|4xx->4xx|status
Issue-Key: iot-jobs-data|UpdateJobExecution|4xx->4xx|$.message
Issue-Key: iot-jobs-data|UpdateJobExecution|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: iot-jobs-data|UpdateJobExecution|4xx->4xx|status
Issue-Commit: d6cf4ead3dce27ec3b8bcbc3d4a389da0fa571ea

# IoT Jobs Data job-execution state is never tracked: queue reads, start, describe and update all miss the execution

> _Note: this issue was identified against the code base at commit `d6cf4ead3dce27ec3b8bcbc3d4a389da0fa571ea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPendingJobExecutions

## Description
The IoT Jobs Data Plane emulator answers all five modeled operations, but it holds no job-execution state: an execution created by an IoT job that targets a thing is never visible to the data plane, so every read returns an empty envelope and every per-execution write is refused as not-found. Nine distinct causes, all in iot-jobs-data (sdkId "IoT Jobs Data Plane", restJson1), are bundled here because they share that single root cause; a fixer must close ALL NINE before submitting.

Cause 1 of 9: GetPendingJobExecutions never reports a QUEUED execution.
AWS: GET /things/{thingName}/jobs returns 200 with queuedJobs/inProgressJobs, each a JobExecutionSummaryList entry carrying jobId, queuedAt, startedAt, lastUpdatedAt, versionNumber and executionNumber (Smithy: GetPendingJobExecutions output members queuedJobs/inProgressJobs -> JobExecutionSummaryList; http GET /things/{thingName}/jobs, code 200). The recording shows the thing's newly targeted job appearing as queuedJobs[0] with versionNumber 1 and executionNumber 1.
Emulator: 200 but queuedJobs is empty; the step polled 15 times and queuedJobs[0] never appeared.
Why: a device agent polling for work never sees the job it was targeted with, so it loops forever and no job is ever executed.
Fix: the GetPendingJobExecutions handler and the service's job-execution state — executions created by job targeting must be registered per thing.
Traces: iotjobsdata-job-execution-queue-poll-and-start#task#4.

Cause 2 of 9: StartNextPendingJobExecution returns an empty body and never returns the started execution.
AWS: PUT /things/{thingName}/jobs/$next promotes the next pending (QUEUED/IN_PROGRESS) execution to IN_PROGRESS and returns it in the response's execution member (Smithy: StartNextPendingJobExecution output {execution: JobExecution}, code 200).
Emulator: 200 with a literally empty JSON body in every recorded call, including the call against the thing that does have a queued execution, so no execution is ever started or returned.
Why: the device-side "get my next job" primitive yields nothing, so an agent can never obtain a job document or move an execution to IN_PROGRESS.
Fix: the StartNextPendingJobExecution handler plus the job-execution state it must select from, and its serializer's execution member.
Traces: iotjobsdata-absent-thing-permissive-queue-reads#task#3 (same cluster: iotjobsdata-job-execution-queue-poll-and-start#task#5, iotjobsdata-job-execution-update-and-terminal-refusals#task#5).

Cause 3 of 9: GetPendingJobExecutions never reports an IN_PROGRESS execution either.
AWS: after an execution has been started, that execution appears under inProgressJobs with its bumped versionNumber (Smithy: GetPendingJobExecutions output member inProgressJobs -> JobExecutionSummaryList). Recording: inProgressJobs[0] for the started job with versionNumber 2, executionNumber 1.
Emulator: 200 with inProgressJobs empty; inProgressJobs[0] missing.
Why: a client reconciling which jobs it has already started sees none and cannot distinguish started from pending work.
Fix: same handler and state as Cause 1, for the in-progress partition.
Traces: iotjobsdata-job-execution-queue-poll-and-start#task#6.

Cause 4 of 9: DescribeJobExecution 404s for an execution that exists and is IN_PROGRESS.
AWS: GET /things/{thingName}/jobs/{jobId} returns 200 with execution:JobExecution (status, statusDetails, queuedAt/startedAt/lastUpdatedAt, approximateSecondsBeforeTimedOut, versionNumber, executionNumber, and jobDocument when includeJobDocument is set) (Smithy: DescribeJobExecution output {execution: JobExecution}, code 200; member includeJobDocument httpQuery includeJobDocument).
Emulator: 404 and no execution member.
Why: the agent cannot read its own execution state or job document, so it cannot act on the job.
Fix: the DescribeJobExecution handler's lookup against job-execution state.
Traces: iotjobsdata-job-execution-queue-poll-and-start#task#7 (same cluster: #task#8).

Cause 5 of 9: DescribeJobExecution still 404s while a just-created job execution is polled for.
AWS: once a job targeting the thing exists, DescribeJobExecution for that thing/jobId answers 200 (same modeled output as Cause 4).
Emulator: 404 ResourceNotFoundException for 30 consecutive poll attempts (example message: "no job execution <jobId> for thing <thingName>").
Why: the execution never becomes eventually consistent, so no amount of client retry makes the job visible.
Fix: job-execution state must be created when a job targets a thing, and DescribeJobExecution must see it.
Traces: iotjobsdata-job-execution-update-and-terminal-refusals#setup#4.

Cause 6 of 9: UpdateJobExecution is refused 404 for a live execution.
AWS: POST /things/{thingName}/jobs/{jobId} with a required status transition returns 200 (Smithy: UpdateJobExecution input status required -> JobExecutionStatus; output {executionState, jobDocument}, code 200).
Emulator: 404 ResourceNotFoundException.
Why: a device can never report progress; the whole job lifecycle is unreachable.
Fix: the UpdateJobExecution handler's lookup and status-transition application in job-execution state.
Traces: iotjobsdata-job-execution-update-and-terminal-refusals#task#6.

Cause 7 of 9: UpdateJobExecution does not return executionState/jobDocument on the terminal-status update.
AWS: when includeJobExecutionState/includeJobDocument are set, the 200 response carries executionState (status, statusDetails, versionNumber) and jobDocument (Smithy: UpdateJobExecution output members executionState -> JobExecutionState, jobDocument @length 0..32768). Recording: executionState {status SUCCEEDED, statusDetails, versionNumber 4} and the job document.
Emulator: 404; both members absent.
Why: a client that relies on the echoed post-update state to confirm the transition gets nothing and treats the job as lost.
Fix: the UpdateJobExecution handler's optional-include projection plus the version counter in job-execution state.
Traces: iotjobsdata-job-execution-update-and-terminal-refusals#task#7.

Cause 8 of 9: a terminal execution must be refused 410 TerminalStateException by DescribeJobExecution, not 404 ResourceNotFoundException.
AWS: DescribeJobExecution models TerminalStateException with httpError 410 for an execution that has reached a terminal state (Smithy: DescribeJobExecution errors include TerminalStateException, errorKind client, httpError 410). Recorded message example: "Job Execution has reached terminal state. It is neither IN_PROGRESS nor QUEUED".
Emulator: 404 with x-amzn-errortype ResourceNotFoundException.
Why: the status and error code are the contract — a client cannot tell "this execution finished" from "no such execution", so retry/cleanup logic branches wrongly.
Fix: the DescribeJobExecution handler's terminal-state branch and its error serializer (status 410, error type TerminalStateException).
Traces: iotjobsdata-job-execution-update-and-terminal-refusals#task#8.

Cause 9 of 9: re-updating a finished execution must be refused 409 InvalidStateTransitionException, not 404 ResourceNotFoundException.
AWS: UpdateJobExecution models InvalidStateTransitionException with httpError 409 (Smithy: UpdateJobExecution errors include InvalidStateTransitionException, errorKind client, httpError 409). Recorded message example: "Job <jobId> for <thingName> has already finished with status SUCCEEDED".
Emulator: 404 with x-amzn-errortype ResourceNotFoundException.
Why: a client retrying an update after a terminal transition sees not-found instead of a conflict and may recreate or abandon work.
Fix: the UpdateJobExecution handler's state-machine guard and its error serializer (status 409, error type InvalidStateTransitionException).
Traces: iotjobsdata-job-execution-update-and-terminal-refusals#task#9.

No open issue covers iot-jobs-data today (the only open IoT-family fidelity issues are iotsitewise ones and a bootstrap issue for iotthingsgraph), and no cluster here was reported as stub-blocked or owned.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200; state_reset_failures 0), all nine causes in service iot-jobs-data:
1) GetPendingJobExecutions queue-poll-and-start#task#4: expected 200 queuedJobs[0]={jobId, queuedAt, startedAt:null, lastUpdatedAt, versionNumber 1, executionNumber 1}; got 200 with queuedJobs[0] missing (poll: executionNumber not in [1] after 15 attempts).
2) StartNextPendingJobExecution absent-thing-permissive-queue-reads#task#3: expected body carrying the execution member, got {} (empty_body); 3 steps affected, including the thing with a queued execution.
3) GetPendingJobExecutions queue-poll-and-start#task#6: expected inProgressJobs[0]={jobId, ..., versionNumber 2, executionNumber 1}; got inProgressJobs[0] missing.
4) DescribeJobExecution queue-poll-and-start#task#7: expected 200 with execution {status IN_PROGRESS, statusDetails {stage: download}, versionNumber 2, executionNumber 1, jobDocument '{"operation":"reboot"}'}; got 404, execution missing (2 steps affected).
5) DescribeJobExecution update-and-terminal-refusals#setup#4: expected 200; got 404 {__type ResourceNotFoundException, message 'no job execution ... for thing ...'} after 30 poll attempts.
6) UpdateJobExecution update-and-terminal-refusals#task#6: expected 200 {executionState, jobDocument}; got 404 ResourceNotFoundException.
7) UpdateJobExecution update-and-terminal-refusals#task#7: expected 200 with executionState {status SUCCEEDED, statusDetails {stage: verify}, versionNumber 4} and jobDocument '{"operation":"install"}'; got 404 with both members missing.
8) DescribeJobExecution update-and-terminal-refusals#task#8: expected status 410 (TerminalStateException, message example 'Job Execution has reached terminal state. It is neither IN_PROGRESS nor QUEUED'); got 404 with x-amzn-errortype ResourceNotFoundException.
9) UpdateJobExecution update-and-terminal-refusals#task#9: expected status 409 (InvalidStateTransitionException, message example 'Job <jobId> for <thing> has already finished with status SUCCEEDED'); got 404 with x-amzn-errortype ResourceNotFoundException.
Model basis: services/service-com-amazonaws-iotjobsdataplane-iotlaserthingjobmanagerexternalservice/smithy-model/com-amazonaws-iotjobsdataplane-iotlaserthingjobmanagerexternalservice.json.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1/3/4: replay fixture iotjobsdata-job-execution-queue-poll-and-start — create an IoT job targeting the thing, then GET /things/{thingName}/jobs (steps task#4 and task#6) and GET /things/{thingName}/jobs/{jobId}?includeJobDocument=true (task#7).
3. Cause 2: replay iotjobsdata-absent-thing-permissive-queue-reads step task#3 — PUT /things/{thingName}/jobs/$next and inspect the response body.
4. Causes 5-9: replay iotjobsdata-job-execution-update-and-terminal-refusals — setup#4 (DescribeJobExecution poll), task#6 and task#7 (POST /things/{thingName}/jobs/{jobId} status updates with includeJobExecutionState/includeJobDocument), task#8 (DescribeJobExecution after the terminal transition, expect 410 TerminalStateException), task#9 (UpdateJobExecution after the terminal transition, expect 409 InvalidStateTransitionException).
