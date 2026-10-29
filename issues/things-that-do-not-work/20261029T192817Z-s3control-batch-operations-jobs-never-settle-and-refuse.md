Issue-Source: trace-sweep-divergence
Issue-Trace: s3c-batch-job-client-request-token-is-bound-to-the-request
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3c-batch-job-client-request-token-is-bound-to-the-request s3c-batch-job-not-found-is-a-400-invalid-request-not-a-404 s3control-batch-job-create-accepts-an-unresolvable-role s3control-batch-job-terminal-state-refuses-status-change-not-tagging
Issue-Shape-Id: com.amazonaws.s3control#AWSS3ControlServiceV20180820
Issue-Key: s3control|CreateJob|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: s3control|CreateJob|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: s3control|CreateJob|4xx->4xx|$.ErrorResponse.HostId
Issue-Key: s3control|CreateJob|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: s3control|DescribeJob|2xx->2xx|$
Issue-Key: s3control|DescribeJob|2xx->2xx|$.DescribeJobResult.Job.FailureReasons
Issue-Key: s3control|DescribeJob|2xx->2xx|$.DescribeJobResult.Job.ProgressSummary
Issue-Key: s3control|DescribeJob|2xx->2xx|$.DescribeJobResult.Job.Status
Issue-Key: s3control|DescribeJob|2xx->2xx|$.DescribeJobResult.Job.TerminationDate
Issue-Key: s3control|DescribeJob|2xx->2xx|$.DescribeJobResult.Job.ManifestGenerator.S3JobManifestGenerator.Filter
Issue-Key: s3control|UpdateJobStatus|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: s3control|ListJobs|2xx->2xx|$.ListJobsResult.Jobs
Issue-Key: s3control|UpdateJobPriority|4xx->2xx|$.ErrorResponse
Issue-Key: s3control|UpdateJobPriority|4xx->2xx|status
Issue-Key: s3control|UpdateJobStatus|4xx->2xx|$.ErrorResponse
Issue-Key: s3control|UpdateJobStatus|4xx->2xx|status
Issue-Related: 20261029T192817Z-s3control-batch-operations-job-not-found-messages-interpolate.md
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# s3control Batch Operations jobs never settle and refuse nothing on priority, status or listing

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeJob

## Description
7 net-new root causes on the s3control Batch Operations surface: jobs never reach a terminal state, DescribeJob's projection is short, two refusals carry the wrong code/message, a terminal job accepts mutations AWS forbids, and ListJobs returns a job the recording does not list. Related open issues 20261025T000830Z-s3control-an-s3-batch-operations-s3putobjecttagging-job-never-reaches.md, 20261025T031335Z-s3control-an-s3-batch-operations-s3putobjecttagging-job-never-reaches-d4a116.md and 20261029T184212Z-s3control-an-s3-batch-operations-job-created-with-an.md describe the non-terminal-job family from the behavioral corpus; none of them carries the wire replay keys below, and the wire corpus still diverges.

--- Cause 1 of 7: the duplicate-ClientRequestToken refusal uses the wrong code and message ---
AWS: recorded behavior only for the wire code and text (Smithy: CreateJob errors include IdempotencyException httpError 400, and ClientRequestToken is @idempotencyToken, @required, length 1..64). The recorded refusal is HTTP 400 with Code `InvalidRequest` and Message `Duplicate ClientRequestToken` — fixed text, with neither the token nor the existing job id interpolated — plus RequestId/HostId.
Emulator: answers 400 with Code `IdempotencyException` and Message `ClientRequestToken <token> was already used to create job <jobId>, with different job parameters.`, and omits RequestId/HostId.
Why: a client branching on `InvalidRequest` does not recognize the collision, and the message leaks the token and a generated job id into text AWS keeps fixed.
Fix: the CreateJob idempotency-token branch's error construction plus the shared envelope serializer.
Traces: s3c-batch-job-client-request-token-is-bound-to-the-request#task#4 (5 steps in cluster).

--- Cause 2 of 7: a created job never reaches a terminal status and reports no failure ---
AWS: recorded behavior only — a job created with a role that cannot be assumed settles: DescribeJob eventually reports `Status: Failed`, a populated `FailureReasons` entry (FailureCode `AccessDenied`, a FailureReason naming the role from the request and the trust requirement for batchoperations.s3.amazonaws.com), a `TerminationDate` and a `ProgressSummary`.
Emulator: the job stays `Status: Suspended` with `SuspendedCause: AwaitingConfirmation`, no FailureReasons and no TerminationDate; the fixture's poll is capped at 60s without ever seeing a terminal status.
Why: a client waiting for a terminal status waits forever, and the single most common Batch Operations failure (an unassumable job role) is never reported, so no remediation path is exercised.
Fix: the Batch Operations job execution/state machine in the s3control crate — advance a created job to a terminal status and fail it with a FailureReason when its role cannot be assumed.
Traces: s3c-batch-job-client-request-token-is-bound-to-the-request#task#6 (3 steps in cluster).

--- Cause 3 of 7: DescribeJob omits FailureReasons, ProgressSummary and the manifest generator's Filter ---
AWS: recorded behavior only — DescribeJob's JobDescriptor always carries `FailureReasons` (empty when there are none), `ProgressSummary` (NumberOfTasksFailed/NumberOfTasksSucceeded/TotalNumberOfTasks and Timers.ElapsedTimeInActiveSeconds) and, when the job was created with one, `ManifestGenerator.S3JobManifestGenerator.Filter`.
Emulator: omits all three.
Why: a client that polls ProgressSummary to report progress, or reads FailureReasons to explain a failure, sees unset fields; the filter it submitted is not echoed, so it cannot confirm what the job will act on.
Fix: the DescribeJob response projection over the job's state.
Traces: s3c-batch-job-not-found-is-a-400-invalid-request-not-a-404#task#11 (2 steps in cluster).

--- Cause 4 of 7: an out-of-set RequestedJobStatus yields the wrong code and message ---
AWS: recorded behavior only for the wire text (Smithy: UpdateJobStatus.RequestedJobStatus is @required, an enum, bound to httpQuery `requestedJobStatus`; errors include BadRequestException/JobStatusException at httpError 400). The recorded refusal is 400 with Code `InvalidRequest` and Message `Request invalid` — fixed text, no interpolation — plus RequestId/HostId.
Emulator: answers 400 with Code `ValidationException` and Message `1 validation error detected: Value 'Complete' at 'requestedJobStatus' failed to satisfy constraint: Member must satisfy enum value set: [Cancelled, Ready]`, and omits RequestId/HostId.
Why: the client gets a code AWS never sends on this surface, so error handling keyed on InvalidRequest misses, and the message leaks the submitted value and the enum set.
Fix: the UpdateJobStatus input validation's error mapping plus the shared envelope serializer.
Traces: s3c-batch-job-not-found-is-a-400-invalid-request-not-a-404#task#12 (2 steps in cluster).

--- Cause 5 of 7: ListJobs returns a job the recorded listing does not ---
AWS: recorded behavior only — at this point in the fixture the listing's `Jobs` is empty (the job created with an unresolvable role is not listed under the queried scope).
Emulator: returns a `Jobs.member` entry for that job (JobId, Description, Operation, Priority, `Status: Suspended`, CreationTime).
Why: a client enumerating jobs sees one AWS does not list under that scope, so job inventories and cleanup loops act on a job that should not be there; combined with cause 2 the listed job never changes status.
Fix: the ListJobs scoping/filter logic over the job state.
Traces: s3control-batch-job-create-accepts-an-unresolvable-role#task#5.

--- Cause 6 of 7: UpdateJobPriority on a terminal job is accepted ---
AWS: recorded behavior only — once a job is in a terminal state its priority cannot be changed: HTTP 400 with Code `InvalidRequest` and Message `Requested job status forbidden` (fixed text), plus RequestId/HostId.
Emulator: answers 200 and applies the change.
Why: a client believes it re-prioritized a finished job; the emulator's job state can be mutated in ways AWS forbids, so state-machine assumptions built on AWS do not hold.
Fix: the UpdateJobPriority handler's state-transition guard over the job state.
Traces: s3control-batch-job-terminal-state-refuses-status-change-not-tagging#task#7.

--- Cause 7 of 7: UpdateJobStatus on a terminal job is accepted ---
AWS: recorded behavior only — a status change out of a terminal state is refused with 400, Code `InvalidRequest`, Message `Requested job status forbidden`, plus RequestId/HostId.
Emulator: answers 200 and applies the change.
Why: as cause 6 — a terminal job can be revived, which AWS never allows.
Fix: the UpdateJobStatus handler's state-transition guard over the job state.
Traces: s3control-batch-job-terminal-state-refuses-status-change-not-tagging#task#8.

## Evidence
Replay of wire corpus simparity-wire-aws-v1 against the emulator:
- CreateJob duplicate token: `Code: expected 'InvalidRequest', got 'IdempotencyException'`; `Message: expected 'Duplicate ClientRequestToken', got 'ClientRequestToken <token> was already used to create job <jobId>, with different job parameters.'`; RequestId/HostId missing — s3c-batch-job-client-request-token-is-bound-to-the-request#task#4 (5 affected steps).
- DescribeJob never terminal: `$: poll capped at 60s` — expected Job.Status `Failed` with FailureReasons {FailureCode 'AccessDenied', FailureReason naming the role and the batchoperations.s3.amazonaws.com trust requirement}, TerminationDate and ProgressSummary; got Status `Suspended`, SuspendedCause `AwaitingConfirmation`, no FailureReasons — same fixture task#6 (3 affected steps).
- DescribeJob projection: `Job.FailureReasons: missing`, `Job.ManifestGenerator.S3JobManifestGenerator.Filter: missing`, `Job.ProgressSummary: missing (expected NumberOfTasksFailed/NumberOfTasksSucceeded/Timers.ElapsedTimeInActiveSeconds/TotalNumberOfTasks)` — s3c-batch-job-not-found-is-a-400-invalid-request-not-a-404#task#11 (2 affected steps).
- UpdateJobStatus bad enum: `Code: expected 'InvalidRequest', got 'ValidationException'`; `Message: expected 'Request invalid', got "1 validation error detected: Value 'Complete' at 'requestedJobStatus' failed to satisfy constraint: Member must satisfy enum value set: [Cancelled, Ready]"` — same fixture task#12 (2 affected steps).
- ListJobs: `$.ListJobsResult.Jobs: type_mismatch (expected '', got {'member': {...'Status': 'Suspended'...}})` — s3control-batch-job-create-accepts-an-unresolvable-role#task#5.
- UpdateJobPriority / UpdateJobStatus on a terminal job: `status: expected 400, got 200` with the expected body `{'Error': {'Code': 'InvalidRequest', 'Message': 'Requested job status forbidden'}, ...}` — s3control-batch-job-terminal-state-refuses-status-change-not-tagging#task#7 and #task#8.

## Steps to Reproduce
1. Replay s3c-batch-job-client-request-token-is-bound-to-the-request#task#4: CreateJob twice with the same ClientRequestToken but different parameters — the emulator answers IdempotencyException with an interpolated message instead of InvalidRequest / `Duplicate ClientRequestToken`.
2. Same fixture task#6: poll DescribeJob for that job — it stays Suspended/AwaitingConfirmation and never reaches Failed with a FailureReason.
3. Replay s3c-batch-job-not-found-is-a-400-invalid-request-not-a-404#task#11 (DescribeJob omits FailureReasons/ProgressSummary/the manifest generator Filter) and #task#12 (POST /v20180820/jobs/{id}/status?requestedJobStatus=Complete returns ValidationException instead of InvalidRequest / `Request invalid`).
4. Replay s3control-batch-job-create-accepts-an-unresolvable-role#task#5: ListJobs returns the suspended job the recording does not list.
5. Replay s3control-batch-job-terminal-state-refuses-status-change-not-tagging#task#7/#task#8: UpdateJobPriority and UpdateJobStatus on a terminal job answer 200 instead of 400 InvalidRequest / `Requested job status forbidden`.
