Issue-Source: trace-sweep-divergence
Issue-Trace: mgn-account-bootstrap
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mgn-account-bootstrap
Issue-Shape-Id: com.amazonaws.mgn#ApplicationMigrationService
Issue-Key: mgn|InitializeService|4xx->2xx|$
Issue-Key: mgn|InitializeService|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: mgn|InitializeService|4xx->2xx|status
Issue-Commit: b4ac6d961cdb10c212ccdfac1172ee263c3e4aa3

# mgn InitializeService returns 204 success instead of modeled ValidationException failure

> _Note: this issue was identified against the code base at commit `b4ac6d961cdb10c212ccdfac1172ee263c3e4aa3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InitializeService

## Description
Single cause (Cause 1 of 1): mgn InitializeService can only ever succeed in the emulator; the modeled client-error path is not implemented at all.

AWS: Smithy (com.amazonaws.mgn, ApplicationMigrationService) models InitializeService with http {method: POST, uri: /InitializeService, code: 204} and errors AccessDeniedException (httpError 403) and ValidationException (httpError 400). ValidationException is a client error structure with members message, code, reason (ValidationExceptionReason, values unknownOperation|cannotParse|fieldValidationFailed|other) and fieldList. The recorded response for this step is HTTP 400 carrying header x-amzn-errortype: ValidationException and an awsJson-style body with __type com.amazon.mgn#ValidationException, reason OTHER, and the message "Failed to create SLR or instance profiles" (recorded behavior only for the exact wording - the message is an example, not a requirement; the status 400, the errortype header and the ValidationException shape with its reason member are the requirements).

Emulator: the request is answered HTTP 204 with an empty body and no x-amzn-errortype header. Replay reports: "status: expected 400, got 204"; "$: empty_body (expected {'message': 'Failed to create SLR or instance profiles', 'reason': 'OTHER', '__type': 'com.amazon.mgn#ValidationException'}, got {})"; "$headers.x-amzn-errortype: missing (expected 'ValidationException', got None)". So the handler returns the unconditional 204 success response and never raises ValidationException.

Why: a client doing account bootstrap through InitializeService is told initialization succeeded when service-linked-role / instance-profile setup did not complete. SDKs key failure handling off the 4xx status and the x-amzn-errortype header, so no exception is raised, the error reason is never surfaced, no retry or remediation path runs, and every later migration call proceeds against an account that was never initialized - a silent-success divergence rather than a cosmetic one.

Fix: belongs in the mgn InitializeService handler and its error serializer. The handler must be able to conclude in failure (driven by the initialization state it actually attempts to establish) and emit the modeled ValidationException instead of always returning the 204 success code; the error serializer must set HTTP 400, the x-amzn-errortype: ValidationException response header, and a body carrying __type com.amazon.mgn#ValidationException with the reason member populated from ValidationExceptionReason (plus message/code/fieldList when known).

Traces: fixture mgn-account-bootstrap, step task#1 (the only member of this cluster).

## Evidence
Fixture mgn-account-bootstrap step task#1, operation InitializeService (service mgn), verdict mismatch.

Expected (recorded AWS response):
  HTTP 400
  header x-amzn-errortype: ValidationException
  body: {"__type": "com.amazon.mgn#ValidationException", "reason": "OTHER", "message": "Failed to create SLR or instance profiles"}   <- message shown as an example

Actual (emulator):
  HTTP 204
  no x-amzn-errortype header
  empty body ({})

Replay detail verbatim:
  status: expected 400, got 204
  $: empty_body (expected {'message': 'Failed to create SLR or instance profiles', 'reason': 'OTHER', '__type': 'com.amazon.mgn#ValidationException'}, got {})
  $headers.x-amzn-errortype: missing (expected 'ValidationException', got None)

Model support: InitializeService errors = [AccessDeniedException httpError 403, ValidationException httpError 400]; success code 204. ValidationException.reason targets ValidationExceptionReason.

## Steps to Reproduce
1. git clone the mockforge-aws-emulator repo (branch mainline) and boot the emulator built from that checkout.
2. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus), or replay fixture mgn-account-bootstrap alone.
3. Observe step task#1: POST /InitializeService against the mgn endpoint.
4. The emulator answers HTTP 204 with an empty body and no x-amzn-errortype header; the recorded AWS response for the same step is HTTP 400 with x-amzn-errortype: ValidationException and a com.amazon.mgn#ValidationException body (reason OTHER).
5. Re-replay mgn-account-bootstrap#task#1 after the fix to confirm the 400 / ValidationException path is produced.
