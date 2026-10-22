Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: comprehend.start-job-then-stop-workflow-reaches-stopped comprehend.stop-after-terminal-sentiment-job-state-conflict-rejected comprehend.describe-sentiment-detection-job-not-found comprehend.create-endpoint-desired-inference-units-below-minimum-rejected
Issue-Shape-Id: com.amazonaws.comprehend#Comprehend_20171127
Issue-Key: comprehend.start-job-then-stop-workflow-reaches-stopped|run#7|AssertionError|assert stop_resp["JobStatus"] == "STOP_REQUESTED", stop_resp
Issue-Key: comprehend.stop-after-terminal-sentiment-job-state-conflict-rejected|_error#15|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: comprehend.describe-sentiment-detection-job-not-found|run#17|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: comprehend.create-endpoint-desired-inference-units-below-minimum-rejected|run#8|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# Comprehend async-job and endpoint control plane: StopSentimentDetectionJob skips STOP_REQUESTED and accepts a stop on a terminal job, and not-found / validation messages diverge (4 causes)

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StopSentimentDetectionJob

## Description
Four distinct root causes in Comprehend's implemented control-plane handlers (the sentiment-detection job family and CreateEndpoint). All live in services/service-com-amazonaws-comprehend-comprehend-20171127.

Cause 1 of 4 — StopSentimentDetectionJob reports the terminal state immediately instead of the intermediate STOP_REQUESTED state.
AWS: Smithy: StopSentimentDetectionJob output members JobId (JobId, length 1..32) and JobStatus (JobStatus enum); modeled errors InvalidRequestException (400), JobNotFoundException (404), InternalServerException (500). Recorded behavior: stopping a job that is still running answers HTTP 200 with JobStatus 'STOP_REQUESTED'; the job then settles asynchronously into the terminal 'STOPPED' state, observable via DescribeSentimentDetectionJob.
Emulator: the first stop of a freshly started, still-running job answers HTTP 200 with JobStatus 'STOPPED' — the STOP_REQUESTED state is never produced, so the stop is modeled as instantaneous.
Why: a client that waits for STOP_REQUESTED before polling for STOPPED (or that treats STOPPED-on-first-stop as 'this job was already dead') mis-sequences its workflow, and the two-phase cancellation contract AWS exposes cannot be exercised at all.
Fix: the StopSentimentDetectionJob handler and the job state machine in the service's state module — the stop response must return STOP_REQUESTED for a non-terminal job and the stored job must transition to STOPPED subsequently.
Traces: comprehend.start-job-then-stop-workflow-reaches-stopped.

Cause 2 of 4 — StopSentimentDetectionJob accepts a stop request against an already-terminal job instead of refusing it.
AWS: Smithy: StopSentimentDetectionJob models InvalidRequestException (HTTP 400, errorKind client). Recorded behavior: once the job is in the terminal STOPPED state, a further StopSentimentDetectionJob on the same JobId is rejected with HTTP 400 InvalidRequestException and the job's status is unchanged (recorded message example: 'Job is already in terminal state.').
Emulator: the repeat stop answers HTTP 200 with {'JobId': '...', 'JobStatus': 'STOPPED'} — no precondition on the job's terminal state is enforced, so an invalid request succeeds.
Why: this is an overly-permissive acceptance: error handling for double-cancel races is never exercised, and a client cannot distinguish 'I stopped it' from 'it was already finished', which AWS signals precisely via InvalidRequestException.
Fix: the StopSentimentDetectionJob handler's precondition check — reject a stop on a job already in a terminal state (STOPPED/COMPLETED/FAILED) with 400 InvalidRequestException and leave stored state untouched.
Traces: comprehend.stop-after-terminal-sentiment-job-state-conflict-rejected.

Cause 3 of 4 — JobNotFoundException from DescribeSentimentDetectionJob carries a request-interpolated message instead of AWS's fixed sentence.
AWS: Smithy: DescribeSentimentDetectionJob models JobNotFoundException (HTTP 404, errorKind client) for a well-formed JobId that names no job. The message is recorded behavior only, but it is a FIXED sentence independent of the request: 'Could not find specified job.'
Emulator: returns the correct code JobNotFoundException with a message built from the request instead: "The sentiment detection job with id 'cbe4602ff1dd4d5899db5582fc7e8410' was not found."
Why: the status and code are right, so this is the narrowest of the four, but the emulator's form echoes caller-supplied identifiers into an error AWS keeps constant — clients and golden-output tests pinned to AWS's sentence diverge, and the shape of the message differs from every other Comprehend not-found response.
Fix: the DescribeSentimentDetectionJob handler's error construction (and the shared JobNotFoundException constructor it uses) — emit AWS's fixed sentence rather than interpolating the JobId.
Traces: comprehend.describe-sentiment-detection-job-not-found.

Cause 4 of 4 — CreateEndpoint's range-constraint ValidationException message includes the rejected value where AWS omits it.
AWS: Smithy: CreateEndpoint.DesiredInferenceUnits -> InferenceUnitsInteger, @required, @range min 1. The refusal is a framework validation error built from the request, so its form is a rule: recorded AWS behavior for DesiredInferenceUnits=0 is code ValidationException with message '1 validation error detected: Value at \'desiredInferenceUnits\' failed to satisfy constraint: Member must have value greater than or equal to 1' — the member name and the constraint sentence, with NO value echoed.
Emulator: same code ValidationException, but the message inserts the offending value: '1 validation error detected: Value \'0\' at \'desiredInferenceUnits\' failed to satisfy constraint: Member must have value greater than or equal to 1'.
Why: the validation-message rendering for range constraints differs from AWS's, so clients (and contract tests) that match AWS's exact validation-error form for this member do not match, and the emulator leaks the submitted value in a message AWS keeps value-free.
Fix: the shared request-validation error renderer used by CreateEndpoint for @range violations — render 'Value at \'<member>\' failed to satisfy constraint: ...' without the value for this constraint class.
Traces: comprehend.create-endpoint-desired-inference-units-below-minimum-rejected.

## Evidence
Cause 1: expected stop_sentiment_detection_job on a running job to return JobStatus 'STOP_REQUESTED'; got {'JobId': 'e12afc63db0cc860ae0639fe295fa97f', 'JobStatus': 'STOPPED'} with HTTPStatusCode 200.
Cause 2: expected HTTP 400 InvalidRequestException for a second stop on an already-STOPPED job; the call SUCCEEDED and returned {'JobId': '81215e3083c7e97dc60367079b60d4e9', 'JobStatus': 'STOPPED'} with HTTPStatusCode 200.
Cause 3: DescribeSentimentDetectionJob on a well-formed but nonexistent JobId — code JobNotFoundException is correct; expected message 'Could not find specified job.', got "The sentiment detection job with id 'cbe4602ff1dd4d5899db5582fc7e8410' was not found."
Cause 4: CreateEndpoint with DesiredInferenceUnits=0 — code ValidationException is correct; expected message "1 validation error detected: Value at 'desiredInferenceUnits' failed to satisfy constraint: Member must have value greater than or equal to 1", got "1 validation error detected: Value '0' at 'desiredInferenceUnits' failed to satisfy constraint: Member must have value greater than or equal to 1".

## Steps to Reproduce
1. Boot the emulator; create an S3 bucket, an input object and a Comprehend data-access IAM role.
2. Cause 1: comprehend.start_sentiment_detection_job(...) then comprehend.stop_sentiment_detection_job(JobId=<id>) — assert JobStatus == 'STOP_REQUESTED'.
3. Cause 2: poll describe_sentiment_detection_job until JobStatus == 'STOPPED', then call stop_sentiment_detection_job(JobId=<id>) again — expect HTTP 400 InvalidRequestException, observe HTTP 200.
4. Cause 3: comprehend.describe_sentiment_detection_job(JobId='0'*32 style well-formed id) and compare Error.Message to 'Could not find specified job.'
5. Cause 4: comprehend.create_endpoint(EndpointName='x', DesiredInferenceUnits=0) and compare the ValidationException message to AWS's value-free form.
6. Replay: comprehend.start-job-then-stop-workflow-reaches-stopped, comprehend.stop-after-terminal-sentiment-job-state-conflict-rejected, comprehend.describe-sentiment-detection-job-not-found, comprehend.create-endpoint-desired-inference-units-below-minimum-rejected.
