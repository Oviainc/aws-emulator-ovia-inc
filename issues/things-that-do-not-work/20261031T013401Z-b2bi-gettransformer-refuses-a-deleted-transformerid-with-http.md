Issue-Source: trace-sweep-divergence
Issue-Trace: b2bi-transformer-lifecycle
Issue-Step: teardown#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: b2bi-transformer-lifecycle
Issue-Shape-Id: com.amazonaws.b2bi#B2BI
Issue-Key: b2bi|GetTransformer|4xx->4xx|status
Issue-Commit: 63c612bdac710916216a1ca8dd5dbf0b76505045

# b2bi GetTransformer refuses a deleted transformerId with HTTP 404 instead of 400

> _Note: this issue was identified against the code base at commit `63c612bdac710916216a1ca8dd5dbf0b76505045`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTransformer

## Description
One cause. Cause 1 of 1: GetTransformer called with a transformerId that has just been deleted (the lifecycle fixture's teardown read-back after DeleteTransformer).

AWS: GetTransformer is bound to the awsJson1_0 b2bi service (Smithy: services/service-com-amazonaws-b2bi-b2bi/smithy-model/com-amazonaws-b2bi-b2bi.json; GetTransformer input transformerId -> TransformerId @required @length 1..64 @pattern ^[a-zA-Z0-9_-]+$; modeled errors AccessDeniedException(403), InternalServerException(500), ResourceNotFoundException(httpError 404), ThrottlingException(429), ValidationException(400)). The model annotates ResourceNotFoundException with @httpError 404, but the recorded AWS wire response for this refusal is HTTP **400** — the same status the recording shows for every other b2bi client refusal in this corpus — so 400 is the requirement for this service. The error body itself matched on replay, so the requirement being missed here is only the HTTP status line: status 400 carrying ResourceNotFoundException. Per the recorded sibling refusals on this service, the body's `__type` is the fully-qualified shape id in the service's internal namespace (com.amazonaws.visby#ResourceNotFoundException) and the message member is spelled `message`; the message for a missing transformer is the service's fixed not-found text with the requested transformerId from the request substituted in — the id itself is a per-run value, not part of the requirement.

Emulator: answers HTTP 404 for the same request. Replay reports `status: expected 400, got 404` for b2bi-transformer-lifecycle#teardown#7, with no body key mismatches.

Why: an SDK, CLI or generic HTTP client keys its handling off the status line before it ever parses the error shape. A 404 where AWS sends 400 makes a modeled, non-retryable resource-not-found refusal look like a missing endpoint or a transport-level 404, so client code that distinguishes "the API rejected my request" from "the API is not there" takes the wrong branch, and polling/wait loops that treat 400 as the terminal deleted signal never settle.

Fix: the b2bi error-status mapping for ResourceNotFoundException as surfaced by GetTransformer's handler — the same status-mapping component named by the already-open issue 20261029T171003Z-b2bi-client-errors-answer-404-with-an-unqualified.md, which covers this fault for CreateCapability/GetCapability/DeleteCapability/Create-Get-DeletePartnership/GetProfile/ListTagsForResource/TestParsing/GetTransformerJob but does NOT cover GetTransformer; and 20261030T140435Z-b2bi-updatepartnership-refuses-an-unknown-partnershipid-with-http.md, which covers UpdatePartnership only. Neither carries the replay key for this step, so GetTransformer's refusal is filed here.

Traces: b2bi-transformer-lifecycle#teardown#7.

## Evidence
Replay of simparity-wire-aws-v1 (10 fixtures, 56 steps, 23 passed, score 41.1%). Cluster service=b2bi|GetTransformer|status=400->404|keys=status at fixture b2bi-transformer-lifecycle step teardown#7: verdict mismatch, expected_status 400, actual_status 404, detail "status: expected 400, got 404". No response-body keys diverged, so the divergence is exactly the HTTP status: AWS 400, emulator 404. The cluster's owned_by list is empty and its stub_path is empty, so no filed issue carries this replay key.

## Steps to Reproduce
1. Boot the emulator from this checkout. 2. Replay the wire corpus simparity-wire-aws-v1. 3. Replay fixture b2bi-transformer-lifecycle and let it reach teardown#7, i.e. CreateTransformer, then DeleteTransformer, then GetTransformer on the deleted transformerId. 4. Observe the emulator answers HTTP 404 where the recording has HTTP 400 for the ResourceNotFoundException refusal.
