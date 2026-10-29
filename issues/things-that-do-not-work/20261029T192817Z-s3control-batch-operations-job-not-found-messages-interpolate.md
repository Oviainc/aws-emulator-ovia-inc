Issue-Source: trace-sweep-divergence
Issue-Trace: s3c-batch-job-not-found-is-a-400-invalid-request-not-a-404
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3c-batch-job-not-found-is-a-400-invalid-request-not-a-404 s3control-batch-job-absent-id-envelopes
Issue-Shape-Id: com.amazonaws.s3control#AWSS3ControlServiceV20180820
Issue-Key: s3control|DescribeJob|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: s3control|DescribeJob|4xx->4xx|$.ErrorResponse.HostId
Issue-Key: s3control|DescribeJob|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: s3control|UpdateJobPriority|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: s3control|UpdateJobPriority|4xx->4xx|$.ErrorResponse.HostId
Issue-Key: s3control|UpdateJobPriority|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: s3control|UpdateJobStatus|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: s3control|UpdateJobStatus|4xx->4xx|$.ErrorResponse.HostId
Issue-Key: s3control|UpdateJobStatus|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: s3control|GetJobTagging|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: s3control|GetJobTagging|4xx->4xx|$.ErrorResponse.HostId
Issue-Key: s3control|GetJobTagging|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: s3control|PutJobTagging|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: s3control|PutJobTagging|4xx->4xx|$.ErrorResponse.HostId
Issue-Key: s3control|PutJobTagging|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: s3control|DeleteJobTagging|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: s3control|DeleteJobTagging|4xx->4xx|$.ErrorResponse.HostId
Issue-Key: s3control|DeleteJobTagging|4xx->4xx|$.ErrorResponse.RequestId
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# s3control Batch Operations job-not-found messages interpolate the job id

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeJob

## Description
6 net-new root causes, one per Batch Operations job operation, all the same fault: the job-not-found refusal is built by appending the requested job id to AWS's fixed message, and its envelope is missing RequestId/HostId. Status (400) and error code match in every cluster.

Shared AWS note: Smithy: DescribeJob/UpdateJobPriority/UpdateJobStatus/GetJobTagging/PutJobTagging/DeleteJobTagging all declare `NotFoundException` with httpError 400 (defaulted), i.e. a missing job is a 400 on this surface. The message is recorded behavior only: exactly `Job not found` — fixed text with no variable part; the id the caller asked about is not interpolated. Every refusal also carries `<RequestId>` and `<HostId>` (per-run values, required to be present and service-shaped).

--- Cause 1 of 6: DescribeJob ---
AWS: 400 with Message `Job not found` (Smithy: DescribeJob errors NotFoundException httpError 400) plus RequestId/HostId.
Emulator: `Job not found: <jobId>`, and no RequestId/HostId.
Why: a client matching AWS's message does not recognize the refusal, and the emulator leaks a request value into text AWS keeps fixed; with no request id there is nothing to correlate.
Fix: the shared job-lookup refusal construction in the s3control crate plus the error envelope serializer.
Traces: s3c-batch-job-not-found-is-a-400-invalid-request-not-a-404#task#2 (3 steps in cluster).

--- Cause 2 of 6: UpdateJobPriority ---
AWS: 400 `Job not found` plus RequestId/HostId.
Emulator: `Job not found: <jobId>`, no RequestId/HostId.
Why: as cause 1.
Fix: same shared refusal, reached from the UpdateJobPriority handler.
Traces: s3c-batch-job-not-found-is-a-400-invalid-request-not-a-404#task#4 (2 steps in cluster).

--- Cause 3 of 6: UpdateJobStatus ---
AWS: 400 `Job not found` plus RequestId/HostId.
Emulator: `Job not found: <jobId>`, no RequestId/HostId.
Why: as cause 1.
Fix: same shared refusal, reached from the UpdateJobStatus handler.
Traces: s3c-batch-job-not-found-is-a-400-invalid-request-not-a-404#task#5 (2 steps in cluster).

--- Cause 4 of 6: GetJobTagging ---
AWS: 400 `Job not found` plus RequestId/HostId.
Emulator: `Job not found: <jobId>`, no RequestId/HostId.
Why: as cause 1.
Fix: same shared refusal, reached from the GetJobTagging handler.
Traces: s3control-batch-job-absent-id-envelopes#task#5.

--- Cause 5 of 6: PutJobTagging ---
AWS: 400 `Job not found` plus RequestId/HostId.
Emulator: `Job not found: <jobId>`, no RequestId/HostId.
Why: as cause 1.
Fix: same shared refusal, reached from the PutJobTagging handler.
Traces: s3control-batch-job-absent-id-envelopes#task#6.

--- Cause 6 of 6: DeleteJobTagging ---
AWS: 400 `Job not found` plus RequestId/HostId.
Emulator: `Job not found: <jobId>`, no RequestId/HostId.
Why: as cause 1.
Fix: same shared refusal, reached from the DeleteJobTagging handler.
Traces: s3control-batch-job-absent-id-envelopes#task#7.

## Evidence
Replay of wire corpus simparity-wire-aws-v1 against the emulator. All six clusters report, at HTTP 400 with the correct code:
`$.ErrorResponse.Error.Message: value_mismatch (expected 'Job not found', got 'Job not found: 00000000-0000-0000-0000-000000000000')`, `$.ErrorResponse.RequestId: missing`, `$.ErrorResponse.HostId: missing`.
- DescribeJob — s3c-batch-job-not-found-is-a-400-invalid-request-not-a-404#task#2 (3 affected steps).
- UpdateJobPriority — same fixture task#4 (2 affected steps).
- UpdateJobStatus — same fixture task#5 (2 affected steps).
- GetJobTagging — s3control-batch-job-absent-id-envelopes#task#5.
- PutJobTagging — same fixture task#6.
- DeleteJobTagging — same fixture task#7.
The job id in the recording is the fixture's probe value; the requirement is that the message does not contain it at all. Recorded RequestId/HostId literals are per-run values; only their presence is required.

## Steps to Reproduce
1. Replay s3c-batch-job-not-found-is-a-400-invalid-request-not-a-404#task#2: GET /v20180820/jobs/<id that was never created> with x-amz-account-id — the emulator answers 400 with `Job not found: <id>` and no RequestId/HostId; the recording has exactly `Job not found`.
2. Same fixture task#4/#task#5: POST /v20180820/jobs/<absent id>/priority and /status — same divergence.
3. Replay s3control-batch-job-absent-id-envelopes#task#5/#task#6/#task#7: GET/PUT/DELETE /v20180820/jobs/<absent id>/tagging — same divergence.
