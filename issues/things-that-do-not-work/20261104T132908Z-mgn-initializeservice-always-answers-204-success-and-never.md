Issue-Source: trace-sweep-divergence
Issue-Trace: mgn-account-bootstrap
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mgn-account-bootstrap
Issue-Shape-Id: com.amazonaws.mgn#ApplicationMigrationService
Issue-Key: mgn|InitializeService|4xx->2xx|$headers.content-length
Issue-Related: 20261022T132055Z-mgn-initializeservice-returns-204-success-instead-of-modeled.md
Issue-Commit: b35d6c95ba9daecf1910283ce30896df8ba3f27b

# mgn InitializeService always answers 204 success and never serializes the modeled ValidationException error response

> _Note: this issue was identified against the code base at commit `b35d6c95ba9daecf1910283ce30896df8ba3f27b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InitializeService

## Description
Single cause (Cause 1 of 1): the mgn InitializeService handler can only ever conclude in success, so the operation's modeled client-error path is never produced — neither its status, nor its error-type header, nor its error body, nor the Content-Length framing of that body.

AWS: Smithy (com.amazonaws.mgn, ApplicationMigrationService) models InitializeService as POST /InitializeService with success @http code 204 and an empty InitializeServiceResponse, and binds the errors AccessDeniedException (httpError 403) and ValidationException (httpError 400). ValidationException is a client-error structure whose members are message (LargeBoundedString, @length 0..65536), code (same), reason (ValidationExceptionReason, values unknownOperation|cannotParse|fieldValidationFailed|other) and fieldList (ValidationExceptionFieldList). When account initialization cannot complete, AWS answers the awsJson-framed error: HTTP 400, response header x-amzn-errortype: ValidationException, a body carrying __type com.amazon.mgn#ValidationException with reason populated and a message describing the failure, and a Content-Length header declaring that body's byte length. Error message rule: the fixed text of this error class is the sentence AWS words for a failed initialization — the recording shows "Failed to create SLR or instance profiles", whose fixed text is the whole sentence and whose only variable part is which resources failed (service-linked role and/or instance profiles) as taken from the emulator's initialization state; the error code (ValidationException), the reason value (recorded as OTHER, from ValidationExceptionReason), the 400 status and the presence of Content-Length are the requirements. The recorded Content-Length value (123) is just the byte length of that one recorded body and is NOT part of the requirement — the requirement is that the error response declares its own body length.

Emulator: the emulator answers the unconditional success response instead. Replay reports "status: expected 400, got 204"; "$: empty_body (expected {'message': 'Failed to create SLR or instance profiles', 'reason': 'OTHER', '__type': 'com.amazon.mgn#ValidationException'}, got {})"; "$headers.x-amzn-errortype: missing (expected 'ValidationException', got None)"; and "$headers.content-length: missing (expected '123', got None)". So the error is not raised, the error body is not serialized, the x-amzn-errortype header is not set, and the response carries no Content-Length at all.

Why: a client bootstrapping an account through InitializeService is told initialization succeeded when the service-linked-role / instance-profile setup did not complete. SDKs decide failure from the 4xx status and the x-amzn-errortype header, so no exception is raised, ValidationException.reason is never surfaced, no retry or remediation runs, and every later migration call proceeds against an account that was never initialized — a silent-success divergence, not a cosmetic one. The missing Content-Length additionally leaves the (would-be) error body undelimited for clients and proxies that frame on it.

Fix: belongs in the mgn InitializeService handler plus the mgn error serializer. The handler must be able to conclude in failure, driven by the initialization state it actually attempts to establish, rather than always returning the modeled 204 success code; the error serializer must emit HTTP 400, set x-amzn-errortype: ValidationException, write a body with __type com.amazon.mgn#ValidationException carrying reason (from ValidationExceptionReason) and message (plus code/fieldList when known), and set Content-Length to that body's byte length.

Related open issue, which does NOT cover this cluster: "mgn InitializeService returns 204 success instead of modeled ValidationException failure" (20261022T132055Z-mgn-initializeservice-returns-204-success-instead-of-modeled.md) describes the same 204-vs-400 handler defect but its recorded replay keys cover only $, $headers.x-amzn-errortype and status — the cluster now also diverges on $headers.content-length, so replay does not report this cluster as owned by it and its replay pointer does not exercise the header. The sibling issue "mgn empty-body 204 tag responses omit the Content-Length header (2 causes)" is about Unit-output 204 responses on TagResource/UntagResource, not about this error response.

Traces: fixture mgn-account-bootstrap, step task#1 (the only member of this cluster).

## Evidence
Replay of the simparity-wire-aws-v1 wire corpus against the emulator built from this checkout (emulator_up: true, bootstrap_signal: false, score_pct 21.4).

Fixture mgn-account-bootstrap, step task#1, operation InitializeService, service mgn, verdict mismatch.
divergence_key: service=mgn|InitializeService|status=400->204|keys=$,$headers.content-length,$headers.x-amzn-errortype,status
affected: ["mgn-account-bootstrap#task#1"] (affected_count 1)
stub_path: (empty) — not blocked by an unimplemented operation. owned_by: (empty) — no open issue carries this replay key.

Expected (recorded AWS response):
  HTTP 400
  header x-amzn-errortype: ValidationException
  header content-length: present, declaring the error body's byte length (recorded value 123 — an example, not a requirement)
  body: {"__type": "com.amazon.mgn#ValidationException", "reason": "OTHER", "message": "Failed to create SLR or instance profiles"}

Actual (emulator):
  HTTP 204
  no x-amzn-errortype header
  no content-length header
  empty body ({})

Replay detail verbatim:
  status: expected 400, got 204
  $: empty_body (expected {'message': 'Failed to create SLR or instance profiles', 'reason': 'OTHER', '__type': 'com.amazon.mgn#ValidationException'}, got {})
  $headers.content-length: missing (expected '123', got None)
  $headers.x-amzn-errortype: missing (expected 'ValidationException', got None)

Model support (services/service-com-amazonaws-mgn-applicationmigrationservice/smithy-model/com-amazonaws-mgn-applicationmigrationservice.json): InitializeService http {method POST, uri /InitializeService, code 204}; input InitializeServiceRequest (no members); output InitializeServiceResponse (no members); errors AccessDeniedException (403) and ValidationException (400). com.amazonaws.mgn#ValidationException: client error, httpError 400, members message/code (LargeBoundedString @length 0..65536), reason -> ValidationExceptionReason [unknownOperation, cannotParse, fieldValidationFailed, other], fieldList -> ValidationExceptionFieldList.

## Steps to Reproduce
1. git_clone the mockforge-aws-emulator repo for this run and boot the emulator built from that checkout.
2. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus), or replay fixture mgn-account-bootstrap alone.
3. Observe step task#1: POST /InitializeService against the mgn endpoint, with an empty JSON request body.
4. The emulator answers HTTP 204, empty body, with neither an x-amzn-errortype nor a content-length response header. The recorded AWS response for the same step is HTTP 400 with x-amzn-errortype: ValidationException, a Content-Length header framing the body, and a com.amazon.mgn#ValidationException body whose reason is OTHER.
5. Inspect the raw response with curl -i to confirm both headers are absent rather than present with other values.
6. After the fix, re-replay mgn-account-bootstrap#task#1 and confirm all four asserted keys ($, status, $headers.x-amzn-errortype, $headers.content-length) now match: HTTP 400, x-amzn-errortype: ValidationException, Content-Length equal to the serialized error body's byte length, and a ValidationException body with its reason populated.
