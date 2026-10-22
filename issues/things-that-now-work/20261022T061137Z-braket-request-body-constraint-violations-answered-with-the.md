Issue-Source: trace-sweep-divergence
Issue-Trace: braket-spending-limit-error-paths
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: braket-spending-limit-error-paths braket-hybrid-job-error-paths
Issue-Shape-Id: com.amazonaws.braket#Braket
Issue-Key: braket|CreateSpendingLimit|4xx->4xx|$.message
Issue-Key: braket|CreateSpendingLimit|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: braket|CreateJob|4xx->4xx|$.message
Issue-Key: braket|CreateJob|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: f95a296744f71657ae3614221270df79d0b4a393

# Braket request-body constraint violations answered with the wrong error type and body

> _Note: this issue was identified against the code base at commit `f95a296744f71657ae3614221270df79d0b4a393`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSpendingLimit

## Description
The POST creates reject a body member that violates its modeled `@pattern` with the generic Smithy constraint failure (`ValidationException` plus a '1 validation error detected: ...' message), where the recorded Braket frontend rejects the body itself with a different error type and a fixed short body. Status (400) matches in both cases; the error TYPE on the wire does not. There are 2 causes below, one per create operation.

Cause 1 of 2: CreateSpendingLimit with `spendingLimit` violating its pattern.
- AWS: Smithy: CreateSpendingLimit, POST /spending-limit, input member `spendingLimit` (String, @length min 1, @pattern ^\\d+(\\.\\d{1,2})?$, required); modeled errors are AccessDeniedException 403, DeviceRetiredException 410, InternalServiceException 500, ThrottlingException 429, ValidationException 400 — `BadRequestException` is NOT in the model's error list for this operation, so this is recorded behavior only: the recording shows the service rejecting the request at the body-parsing layer with HTTP 400, header `x-amzn-errortype: BadRequestException` and the fixed message 'Invalid request body' (that message is a constant in the recording, so it is quoted as an example; the 400 status and the `BadRequestException` error code are the requirement).
- Emulator: HTTP 400 with `x-amzn-errortype: ValidationException` and message "1 validation error detected: Value 'not-a-number' at 'spendingLimit' failed to satisfy constraint: Member must satisfy regular expression pattern: ^\\d+(\\.\\d{1,2})?$" — i.e. the shared Smithy constraint-validation path runs and names a modeled error the recorded service does not use here.
- Why: the error code is what clients dispatch on. An SDK or CLI configured to handle `BadRequestException` from Braket's spending-limit APIs sees an unmodeled-for-it `ValidationException` instead, so its handler misses; conversely code that only inspects `ValidationException.reason` reads a field the real service never sends on this path. Tests and retry classifiers written against AWS fail against the emulator even though both answer 400.
- Fix: the request-body rejection path for CreateSpendingLimit (its serializer/error channel, ahead of the handler) — emit the body-level rejection the recording shows (400 with error type `BadRequestException` and the short invalid-body message) rather than the generic Smithy constraint ValidationException.
- Traces: braket-spending-limit-error-paths#task#1 (expected `$headers.x-amzn-errortype` 'BadRequestException', got 'ValidationException'; `$.message` expected 'Invalid request body').

Cause 2 of 2: CreateJob with `jobName` violating its pattern.
- AWS: Smithy: CreateJob, POST /job, input member `jobName` (String, @length 1..50, @pattern ^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,50}$, required); modeled errors include ValidationException 400. Recorded behavior only: the recording answers an invalid `jobName` with HTTP 400 and the same fixed body message 'Invalid request body', and its `x-amzn-errortype` header does not match the emulator's bare `ValidationException` token (the fixture asserts that header's shape, which the emulator's value fails). As in Cause 1, the status and the error type are the requirement; the message text is an example.
- Emulator: HTTP 400 with `x-amzn-errortype: ValidationException` and message "1 validation error detected: Value 'invalid_job_name_!!' at 'jobName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,50}$".
- Why: same client impact as Cause 1 — the error type a caller branches on differs from AWS for every malformed CreateJob body, and the response body carries a different shape of detail than the real service, so code that parses the invalid-body response misreads it.
- Fix: same component as Cause 1, applied to CreateJob's body-rejection path, so both POST creates share one body-level rejection behavior instead of falling through to the generic constraint validator. (Note: CreateJob's handler itself is still a stub — see the stub issue already filed for it — but this divergence is in the body-validation layer that runs before the handler, so it is independent of that work.)
- Traces: braket-hybrid-job-error-paths#task#1 (`$.message` expected 'Invalid request body'; `$headers.x-amzn-errortype` shape mismatch, got 'ValidationException').

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from mainline:

1) fixture braket-spending-limit-error-paths, step task#1, CreateSpendingLimit, spendingLimit='not-a-number'
   expected: HTTP 400, $headers.x-amzn-errortype = 'BadRequestException', $.message = 'Invalid request body' (message is an example)
   actual:   HTTP 400, $headers.x-amzn-errortype = 'ValidationException', $.message = "1 validation error detected: Value 'not-a-number' at 'spendingLimit' failed to satisfy constraint: Member must satisfy regular expression pattern: ^\\d+(\\.\\d{1,2})?$"
   divergence_key: service=braket|CreateSpendingLimit|status=400->400|keys=$.message,$headers.x-amzn-errortype

2) fixture braket-hybrid-job-error-paths, step task#1, CreateJob, jobName='invalid_job_name_!!'
   expected: HTTP 400, $.message = 'Invalid request body', x-amzn-errortype asserted by shape
   actual:   HTTP 400, $headers.x-amzn-errortype = 'ValidationException', $.message = "1 validation error detected: Value 'invalid_job_name_!!' at 'jobName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,50}$"
   divergence_key: service=braket|CreateJob|status=400->400|keys=$.message,$headers.x-amzn-errortype

Model evidence (services/service-com-amazonaws-braket-braket/smithy-model/com-amazonaws-braket-braket.json): CreateSpendingLimit models ValidationException(400), AccessDeniedException(403), DeviceRetiredException(410), ThrottlingException(429), InternalServiceException(500) and no BadRequestException; the recorded response nevertheless carries x-amzn-errortype BadRequestException with status 400.

## Steps to Reproduce
1. Boot the emulator from mainline and replay the wire corpus (replay_wire_corpus).
2. Cause 1: POST /spending-limit with a valid clientToken and deviceArn and spendingLimit='not-a-number' (fixture braket-spending-limit-error-paths, step task#1). Inspect the response's x-amzn-errortype header: it reads 'ValidationException' where the recording has 'BadRequestException'.
3. Cause 2: POST /job with jobName='invalid_job_name_!!' and otherwise valid members (fixture braket-hybrid-job-error-paths, step task#1). Inspect the x-amzn-errortype header and the message body: the emulator returns the generic Smithy constraint text instead of the recorded invalid-body response.
4. After the fix, re-replay braket-spending-limit-error-paths#task#1 and braket-hybrid-job-error-paths#task#1; both must match the recorded error type and body.
