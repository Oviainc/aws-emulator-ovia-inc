Issue-Source: trace-sweep-divergence
Issue-Trace: iotjobsdata-update-job-execution-validation-refusals
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iotjobsdata-update-job-execution-validation-refusals
Issue-Shape-Id: com.amazonaws.iotjobsdataplane#IotLaserThingJobManagerExternalService
Issue-Key: iot-jobs-data|UpdateJobExecution|4xx->4xx|$.message
Issue-Key: iot-jobs-data|UpdateJobExecution|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: 87a15e696e25e083bfb6cdeb9d5ca1a3533a6381

# UpdateJobExecution validation refusals word their error bodies differently from AWS

> _Note: this issue was identified against the code base at commit `87a15e696e25e083bfb6cdeb9d5ca1a3533a6381`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateJobExecution

## Description
Two distinct causes, both in the UpdateJobExecution input-validation path of service-com-amazonaws-iotjobsdataplane-iotlaserthingjobmanagerexternalservice. In both the HTTP status (400) is right and only the error body/headers diverge, so a client that branches on the refusal text or the error-type header reads the emulator's refusal differently from AWS's.

Cause 1 of 2: the enum-constraint ValidationException for UpdateJobExecution$status renders the permitted value set in the wrong order, and the x-amzn-errortype header's error type has the wrong shape.
AWS: UpdateJobExecution$status targets JobExecutionStatus (Smithy: UpdateJobExecution.status required, target enum JobExecutionStatus; modeled errors CertificateValidationException/InvalidRequestException 400, InvalidStateTransitionException 409, ResourceNotFoundException 404, ThrottlingException 429, ServiceUnavailableException 503). For an unmodeled enum value the recording shows HTTP 400 with the standard constraint-validation body: fixed text "1 validation error detected: Value '<value>' at '<memberName>' failed to satisfy constraint: Member must satisfy enum value set: [<permitted values>]", where <value> is the rejected value from the request and <memberName> is the offending input member — both request-derived — and the bracketed value set is FIXED text that AWS emits in the order REMOVED, SUCCEEDED, TIMED_OUT, IN_PROGRESS, QUEUED, CANCELED, FAILED, REJECTED (recorded behavior only: the model declares no ordering for the message, and the Smithy enum's own declaration order is QUEUED, IN_PROGRESS, SUCCEEDED, FAILED, TIMED_OUT, REJECTED, REMOVED, CANCELED). The recording's x-amzn-errortype header for this refusal does NOT match the bare token 'ValidationException' (the replay reports it as a shape mismatch against the recorded header). Hypothesis: AWS decorates the coral error type (e.g. a qualified/suffixed form) rather than emitting the bare exception name.
Emulator: answers 400 with the same fixed sentence but prints the value set in the generated catalog's declaration order, "[QUEUED, IN_PROGRESS, SUCCEEDED, FAILED, TIMED_OUT, REJECTED, REMOVED, CANCELED]", and sets x-amzn-errortype to the bare 'ValidationException'.
Why: a client (or a test harness, or a human reading a device log) that compares the refusal body against AWS's text — or that parses x-amzn-errortype to classify the failure — sees a body/header that no AWS endpoint produces, so an exact-match check passes against AWS and fails against the emulator even though the constraint was enforced correctly.
Fix: the boundary enum validator that formats the enum-value-set constraint message for this operation's input (the shared smithy-server-runtime input validation/serializer path), plus the error-type header it writes for a constraint failure. The permitted-value ordering used in the message must be AWS's, independent of the generated catalog order.
Traces: iotjobsdata-update-job-execution-validation-refusals#task#5.

Cause 2 of 2: the stepTimeoutInMinutes refusal uses the emulator's own invented sentence instead of AWS's wording.
AWS: UpdateJobExecution$stepTimeoutInMinutes is an unconstrained long in the model, documented as "Valid values for this parameter range from 1 to 10080 (1 minute to 7 days). A value of -1 is also valid and will cancel the current step timer" (Smithy: UpdateJobExecution.stepTimeoutInMinutes, target StepTimeoutInMinutes, no @range). The recording shows an out-of-range value refused 400 with the message "Provide valid timeout value, stepTimeoutInMinutes cannot be 0." — fixed text "Provide valid timeout value, stepTimeoutInMinutes cannot be " followed by the rejected value taken from the request and a terminating period. AWS does not enumerate the bound in the message.
Emulator: answers 400 but with its own sentence, "stepTimeoutInMinutes must be between 1 and 10080, or -1 to cancel the step timer, got 0", built from its MIN/MAX/CANCEL constants.
Why: the refusal is correct in status and error class but no AWS-worded message ever reaches the client, so device code or tooling matching AWS's refusal text (or surfacing it to an operator) behaves differently against the emulator.
Fix: the UpdateJobExecution handler's stepTimeoutInMinutes validation branch must emit AWS's message template with the request's rejected value substituted. Note the related open issue "The stepTimeoutInMinutes range branch is implemented twice, once per handler" (20261028T172940Z-iot-jobs-data-plane-updatejobexecution-maintainability.md) — it covers only the duplication of the range branch and explicitly preserves today's message text, so it does NOT cover this wire divergence.
Traces: iotjobsdata-update-job-execution-validation-refusals#task#6, iotjobsdata-update-job-execution-validation-refusals#task#7.

## Evidence
Replay of trace set simparity-wire-aws-v1 (9 fixtures, 98 steps, 95 passed, 3 mismatched, score 96.9%).

Cause 1 — fixture iotjobsdata-update-job-execution-validation-refusals, step task#5, UpdateJobExecution, expected 400 / actual 400:
  $.message: value_mismatch
    expected: "1 validation error detected: Value 'BOGUS_STATUS' at 'status' failed to satisfy constraint: Member must satisfy enum value set: [REMOVED, SUCCEEDED, TIMED_OUT, IN_PROGRESS, QUEUED, CANCELED, FAILED, REJECTED]"
    actual:   "1 validation error detected: Value 'BOGUS_STATUS' at 'status' failed to satisfy constraint: Member must satisfy enum value set: [QUEUED, IN_PROGRESS, SUCCEEDED, FAILED, TIMED_OUT, REJECTED, REMOVED, CANCELED]"
  $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ValidationException')
  Only the ordering of the fixed value-set text and the error-type header diverge; 'BOGUS_STATUS' and 'status' are the request-derived parts and both match.

Cause 2 — fixture iotjobsdata-update-job-execution-validation-refusals, step task#6 (cluster also covers task#7), UpdateJobExecution, expected 400 / actual 400:
  $.message: value_mismatch
    expected: 'Provide valid timeout value, stepTimeoutInMinutes cannot be 0.'
    actual:   'stepTimeoutInMinutes must be between 1 and 10080, or -1 to cancel the step timer, got 0'

Emulator source for cause 2: services/service-com-amazonaws-iotjobsdataplane-iotlaserthingjobmanagerexternalservice/src/ops/update_job_execution.rs:229-242 formats the message from MIN_STEP_TIMEOUT_MINUTES / MAX_STEP_TIMEOUT_MINUTES / CANCEL_STEP_TIMER_MINUTES. The status-enum defense-in-depth branch at the same file:214-225 is not the source of cause 1's text (the boundary validator refuses first and emits the '1 validation error detected' body).

Smithy consulted: UpdateJobExecution (POST /things/{thingName}/jobs/{jobId}); JobExecutionStatus enum values QUEUED, IN_PROGRESS, SUCCEEDED, FAILED, TIMED_OUT, REJECTED, REMOVED, CANCELED; InvalidRequestException (client, httpError 400, member message).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture 'iotjobsdata-update-job-execution-validation-refusals'.
2. Cause 1 — step task#5: POST /things/{thingName}/jobs/{jobId} with a status value outside JobExecutionStatus (e.g. "BOGUS_STATUS"). Observe 400; compare the response body's $.message value-set ordering and the x-amzn-errortype header against the recorded response (AWS orders the set REMOVED, SUCCEEDED, TIMED_OUT, IN_PROGRESS, QUEUED, CANCELED, FAILED, REJECTED and does not send the bare token 'ValidationException').
3. Cause 2 — steps task#6 and task#7: POST the same URI with stepTimeoutInMinutes outside 1..10080 and not -1 (e.g. 0). Observe 400 with 'stepTimeoutInMinutes must be between 1 and 10080, or -1 to cancel the step timer, got 0' instead of AWS's 'Provide valid timeout value, stepTimeoutInMinutes cannot be 0.'
4. Re-replay the fixture after fixing; both clusters (task#5 and task#6/task#7) must pass.
