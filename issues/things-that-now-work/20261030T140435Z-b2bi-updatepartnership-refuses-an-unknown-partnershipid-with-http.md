Issue-Source: trace-sweep-divergence
Issue-Trace: b2bi-partnership-error-paths
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: b2bi-partnership-error-paths
Issue-Shape-Id: com.amazonaws.b2bi#B2BI
Issue-Key: b2bi|UpdatePartnership|4xx->4xx|status
Issue-Commit: 65165f71b3b3c14984c314d4becd7f5d11cc460b

# b2bi UpdatePartnership refuses an unknown partnershipId with HTTP 404 instead of 400

> _Note: this issue was identified against the code base at commit `65165f71b3b3c14984c314d4becd7f5d11cc460b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdatePartnership

## Description
One cause, net-new. It belongs to the same shared fault as the already-open issue 20261029T171003Z-b2bi-client-errors-answer-404-with-an-unqualified.md (b2bi client errors answer 404 with an unqualified __type instead of 400 and the modeled exception), but that issue's cause list stops at ten causes and does NOT name UpdatePartnership or any b2bi-partnership-error-paths#task#4 binding, so this refusal is covered by no open issue and no fixer replay exercises it.

Cause 1 of 1: UpdatePartnership against a partnershipId that does not exist.

AWS: Smithy model services/service-com-amazonaws-b2bi-b2bi (protocol awsJson1_0). UpdatePartnership takes partnershipId (target PartnershipId, @required, @length 1..64, @pattern ^[a-zA-Z0-9_-]+$) and models the client errors ResourceNotFoundException (errorKind client, @httpError 404) and ValidationException (errorKind client, @httpError 400). The recorded AWS wire response for this operation on an unknown partnershipId is HTTP **400** — on this service the recordings answer 400 for the modeled client-error refusals rather than the @httpError the shape declares, so 400 is the requirement here. The response body (error code and message) is what the recording expects and the emulator already produces: only the status line diverges in this step, so the message text is not in question for this cause.

Emulator: answers HTTP 404 for the same refusal. Replay reports "status: expected 400, got 404" with no body keys diverging.

Why: an SDK, CLI or any generic HTTP client keys its handling off the status line before it ever parses the body. A 404 where AWS sends 400 makes a modeled, non-retryable client refusal look like a missing endpoint or a transport-level not-found, so retry/error-classification middleware takes the wrong branch and callers that distinguish 4xx client-validation failures from a bad endpoint misreport the cause. Because the other b2bi partnership refusals (CreatePartnership, GetPartnership, DeletePartnership) are already filed with this same status fault, leaving UpdatePartnership out means a fixer can correct the mapping for the filed operations, replay only their steps, and close them while UpdatePartnership still answers 404.

Fix: the b2bi error-status mapping in the service's error serializer for the awsJson1_0 protocol — the same component the sibling issue names — applied to the not-found path of UpdatePartnership's handler. No change to the error code or message is needed for this cause.

Traces: b2bi-partnership-error-paths#task#4 (UpdatePartnership, expected 400, got 404).

## Evidence
Replay of simparity-wire-aws-v1 (10 fixtures, 56 steps, 21 passed, score 37.5%), divergence cluster service=b2bi|UpdatePartnership|status=400->404|keys=status, verdict mismatch, affected_count 1 (b2bi-partnership-error-paths#task#4), stub_path empty, owned_by empty.

Expected: HTTP 400 (recorded AWS response for UpdatePartnership on a partnershipId that does not exist); body as recorded, which the emulator already matches.
Actual: HTTP 404. detail: "status: expected 400, got 404" — no body key is reported as missing or mismatched for this step.

Model facts used: smithy_query query=operation, model_path=services/service-com-amazonaws-b2bi-b2bi/smithy-model/com-amazonaws-b2bi-b2bi.json, operation=UpdatePartnership — errors ResourceNotFoundException (client, httpError 404) and ValidationException (client, httpError 400); input member partnershipId @required @length 1..64 @pattern ^[a-zA-Z0-9_-]+$; httpBindingsInert (RPC, dispatch by X-Amz-Target).

## Steps to Reproduce
1. Boot the emulator from this checkout. 2. Replay the wire corpus simparity-wire-aws-v1. 3. Replay fixture b2bi-partnership-error-paths and stop at step task#4, which calls UpdatePartnership with a partnershipId that was never created (the fixture's preceding steps exercise the other partnership refusals). 4. Observe the response status is HTTP 404 where the recording has HTTP 400, while the error body itself matches the recording.
