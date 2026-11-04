Issue-Source: trace-sweep-divergence
Issue-Trace: sagemaker-processing-job-workflow
Issue-Step: teardown#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sagemaker-processing-job-workflow
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker|StopProcessingJob|4xx->2xx|$
Issue-Key: sagemaker|StopProcessingJob|4xx->2xx|status
Issue-Commit: e1429efe042986c67eccadb1cbd7890efbf1f6eb

# SageMaker StopProcessingJob accepts an absent processing job with 200

> _Note: this issue was identified against the code base at commit `e1429efe042986c67eccadb1cbd7890efbf1f6eb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StopProcessingJob

## Description
Cause 1 of 1: StopProcessingJob invoked for a processing job name that the account does not have (here, the job of the fixture's workflow is no longer stoppable/present at teardown).

AWS: rejects the call with HTTP 400 and a client error naming the missing job. Smithy: StopProcessingJob input ProcessingJobName (required, @length 1..63, @pattern ^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$), output Unit, modeled error ResourceNotFound (client, httpError 400); the recorded response for this path carries __type 'ValidationException' with message 'Could not find job to update with name: <ProcessingJobName>'. Fixed text of that message is 'Could not find job to update with name: '; the variable part is the ProcessingJobName taken verbatim from the request (recorded behavior only for the choice of ValidationException over the modeled ResourceNotFound — the recording is authoritative here).

Emulator: answers HTTP 200 with an empty JSON body ({}), i.e. it treats stop-if-exists as success and emits no error code and no message at all.

Why: a client cannot distinguish 'the job was stopped' from 'the job name does not exist'. Control loops that stop a job and then branch on the error (retry with a corrected name, treat the job as already reaped, surface a user-facing naming mistake) take the success branch against the emulator and the error branch against AWS, so typo'd or already-deleted job names are silently swallowed and workflows that depend on the 400 never converge.

Fix: the StopProcessingJob handler's precondition check — look the ProcessingJobName up in the processing-job state store before mutating and, when it is absent, serialize the 400 client error (__type plus the message template above) instead of returning an empty 200. The same gate is missing on the stop path only; the sibling bundle 20261029T192819Z-sagemaker-delete-and-stop-paths-lack-not-found.md covers the delete/stop gates of other SageMaker resources (AppImageConfig, FeatureGroup, Endpoint, EndpointConfig, Algorithm, AIRecommendationJob) but does NOT carry a replay key for StopProcessingJob, which is why this is filed separately.

Traces: sagemaker-processing-job-workflow#teardown#2.

## Evidence
Replay of fixture sagemaker-processing-job-workflow, step teardown#2, operation StopProcessingJob (service sagemaker).
Expected (recorded AWS): status 400, body {'__type': 'ValidationException', 'message': 'Could not find job to update with name: <ProcessingJobName>'} — recorded instance used the job name sim2real-probe-sagemaker-procjob-79e1c3bb, which is a per-run value filling the message template, not part of it.
Actual (emulator): status 200, body {} (empty_body); no __type, no message.
Divergence detail verbatim: "status: expected 400, got 200\n$: empty_body (expected {'__type': 'ValidationException', 'message': 'Could not find job to update with name: sim2real-probe-sagemaker-procjob-79e1c3bb'}, got {})".
Divergence key: service=sagemaker|StopProcessingJob|status=400->200|keys=$,status. Verdict: mismatch. Cluster size: 1 (affected: sagemaker-processing-job-workflow#teardown#2).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Representative binding: fixture sagemaker-processing-job-workflow, step teardown#2.
3. Equivalently by hand: call StopProcessingJob with a ProcessingJobName that was never created (or was already removed) in the emulator's state.
4. Observe HTTP 200 with an empty body; AWS answers HTTP 400 with __type 'ValidationException' and message 'Could not find job to update with name: <ProcessingJobName>'.
