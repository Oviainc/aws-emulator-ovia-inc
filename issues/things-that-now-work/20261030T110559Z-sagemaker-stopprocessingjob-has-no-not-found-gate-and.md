Issue-Source: trace-sweep-divergence
Issue-Trace: sagemaker-processing-job-workflow
Issue-Step: teardown#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sagemaker-processing-job-workflow
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker|StopProcessingJob|4xx->2xx|$
Issue-Key: sagemaker|StopProcessingJob|4xx->2xx|status
Issue-Commit: 14fcb532a6243119e1963a7bcc0017c1001c0e15

# SageMaker StopProcessingJob has no not-found gate and answers 200 for an absent job

> _Note: this issue was identified against the code base at commit `14fcb532a6243119e1963a7bcc0017c1001c0e15`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StopProcessingJob

## Description
One cause, one root fault: the emulator's StopProcessingJob handler performs no lookup of the named processing job before reporting success, so a stop request naming a job that does not exist in the state store is answered as if it had been stopped.

Cause 1 of 1: StopProcessingJob for a ProcessingJobName that is not present.

AWS: the stop target is resolved first and an unknown job name is refused. Smithy: StopProcessingJob input member ProcessingJobName is required, @length min 1 max 63, pattern ^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$; the operation's output is Unit and its only modeled error is ResourceNotFound (client error, httpError 400). The recording shows the wire form AWS actually serves for this case: HTTP 400 with an awsJson error body whose __type is ValidationException and whose message is 'Could not find job to update with name: <ProcessingJobName>'. In that message 'Could not find job to update with name: ' is fixed text and the trailing value is the ProcessingJobName taken verbatim from the request; nothing else in it varies. (The error code AWS serves here is ValidationException rather than the modeled ResourceNotFound - recorded behavior only, and it is the code a client sees.)

Emulator: HTTP 200 with an empty JSON body ({}), i.e. the same response it serves for a successful stop. No lookup, no refusal, no error code and no message.

Why: a client that stops a job by name cannot distinguish "stopped" from "no such job". A typo'd or stale job name, or a teardown that runs after the job was never created, is reported as success, so the client proceeds to poll or delete a job that does not exist and the real failure surfaces much later, somewhere unrelated. Error-handling paths that catch the ValidationException to decide whether to create the job are never taken against the emulator.

Fix: the StopProcessingJob handler's precondition check - resolve ProcessingJobName in the processing-job state before mutating or returning, and when it does not resolve, refuse with 400 and the ValidationException body above instead of the empty 200 success envelope. This is the same missing-precondition family as the already-open issue 20261029T192819Z-sagemaker-delete-and-stop-paths-lack-not-found.md (DeleteEndpoint / DeleteEndpointConfig / DeleteAlgorithm / DeleteAIRecommendationJob), but that issue does not cover StopProcessingJob and does not carry this replay key, so it is filed separately here.

Traces: sagemaker-processing-job-workflow#teardown#2.

## Evidence
Fixture sagemaker-processing-job-workflow, step teardown#2, operation StopProcessingJob.
Expected (recorded AWS): HTTP 400, body {'__type': 'ValidationException', 'message': 'Could not find job to update with name: <ProcessingJobName>'} - fixed text 'Could not find job to update with name: ' plus the ProcessingJobName from the request (the recording's value was sim2real-probe-sagemaker-procjob-79e1c3bb; that name is request input, not part of the requirement).
Actual (emulator): HTTP 200, body {} (replay detail: "status: expected 400, got 200" and "$: empty_body").
Smithy (services/service-com-amazonaws-sagemaker-sagemaker): StopProcessingJob input StopProcessingJobRequest.ProcessingJobName required, @length 1..63, pattern ^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$; output Unit; errors: ResourceNotFound (client, httpError 400).
Note on the fixture's own history: in the recorded run the preceding CreateProcessingJob (task#1) was refused by AWS for a non-assumable role, which is why no such job existed at teardown. That create divergence is a separate, already-owned cause (20261029T192819Z-sagemaker-create-paths-accept-references-and-documents-aws.md); the defect filed here is independent of it - StopProcessingJob must refuse any name it cannot resolve.

## Steps to Reproduce
1. Replay the wire corpus: fixture sagemaker-processing-job-workflow, step teardown#2.
2. Standalone: issue SageMaker.StopProcessingJob with ProcessingJobName set to a syntactically valid name that was never created (e.g. any name matching ^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$ with no preceding CreateProcessingJob).
3. Observe the emulator answers HTTP 200 with an empty body instead of HTTP 400 with __type 'ValidationException' and message 'Could not find job to update with name: <ProcessingJobName>'.
