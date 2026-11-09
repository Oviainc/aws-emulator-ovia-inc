Issue-Source: trace-sweep-divergence
Issue-Trace: wickr-network-create-and-validation
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: wickr-network-create-and-validation wickr-network-lifecycle-and-not-found-errors
Issue-Shape-Id: com.amazonaws.wickr#WickrAdminApi
Issue-Key: wickr|GetNetwork|4xx->4xx|$.message
Issue-Key: wickr|GetNetwork|4xx->4xx|$headers.content-type
Issue-Key: wickr|GetNetwork|4xx->4xx|$headers.etag
Issue-Key: wickr|GetNetwork|4xx->4xx|status
Issue-Key: wickr|GetNetwork|4xx->4xx|$
Issue-Key: wickr|GetNetwork|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: 22941ee6d43df66d29bcadd7a62b7906e5868e84

# wickr: GetNetwork error responses use the wrong status and message for invalid, unknown and deleted networkIds

> _Note: this issue was identified against the code base at commit `22941ee6d43df66d29bcadd7a62b7906e5868e84`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetNetwork

## Description
Three net-new causes: GetNetwork's refusal path answers a different modeled error — and a different HTTP status and message — than AWS for all three unhappy networkId cases. The Wickr admin API answers these with authorization-class errors rather than a validation or not-found error; the emulator answers each from its own input-validation/state lookup. A fixer must close all THREE before submitting.

Cause 1 of 3: a networkId path label the emulator judges malformed is refused as a bad request instead of being denied.
  AWS: recorded behavior — the response is status 403 with body {"message": "Access Denied"}, i.e. the modeled ForbiddenError (Smithy: GetNetwork errors include ForbiddenError, @error client, @httpError 403, single `message` member). The message is entirely fixed text: "Access Denied", with no variable part — it names neither the member nor the value. Hypothesis: AWS resolves ownership of the networkId before it validates the label's shape, so a label it cannot own is denied rather than reported as a validation failure.
  Emulator: status 400 (BadRequestError) with $.message = "1 validation error detected: networkId" — a framework constraint message naming the NetworkId @length 8 / @pattern ^[0-9]{8}$ check.
  Why: a client that branches on 403/AccessDenied (to re-auth, to assume-role, or to report a permissions problem) instead sees a 400 it classifies as its own malformed request, and error-code-driven retry/alarming logic takes the wrong branch.
  Fix: the GetNetwork handler's refusal path — specifically the ordering of path-label validation against the ownership/authorization check, and the error shape it selects.
  Traces: wickr-network-create-and-validation#task#3.

Cause 2 of 3: a well-formed networkId that names no network is reported as not found instead of unauthenticated.
  AWS: recorded behavior — status 401 with body {"message": "Access Denied"}, i.e. the modeled UnauthorizedError (Smithy: @error client, @httpError 401, single `message` member). Fixed text "Access Denied" with no variable part; in particular AWS does not disclose the networkId or its existence.
  Emulator: status 404 ResourceNotFoundError with $.message = "network <id> does not exist" — its own wording, interpolating the requested networkId.
  Why: AWS deliberately does not confirm whether an unowned network exists; the emulator both uses the wrong status/class and leaks existence, so a client's 401 re-authentication path is never exercised and negative tests that assert non-disclosure pass against AWS and fail here.
  Fix: the GetNetwork handler's unknown-network branch (error shape, status and message), in the same refusal path.
  Traces: wickr-network-lifecycle-and-not-found-errors#task#5.

Cause 3 of 3: GetNetwork on a network that has just been deleted answers 404 ResourceNotFoundError instead of 403 "Network not found".
  AWS: recorded behavior — status 403 with body {"message": "Network not found"}, i.e. the modeled ForbiddenError (403) carrying that fixed text; the message names no id.
  Emulator: status 404 with x-amzn-errortype: ResourceNotFoundError and body {"__type": "ResourceNotFoundError", "message": "network <id> does not exist"}. Because DeleteNetwork is asynchronous on AWS, the fixture POLLS GetNetwork for the 403/"Network not found" terminal response; the emulator's 404 never converges, so the poll exhausts all 30 attempts and the post-deletion state is unobservable.
  Why: a client that polls for deletion completion by waiting for the documented terminal refusal never terminates against the emulator, and any client switching on 403 vs 404 classifies a deleted network differently than AWS does.
  Fix: the GetNetwork handler's deleted/absent-network branch (error shape, status and message) and the deletion state it reads.
  Traces: wickr-network-lifecycle-and-not-found-errors#task#7.

All three responses additionally omit the content-type charset and the ETag header; that service-wide header defect is filed as its own cause in the wickr write-path envelope issue and is not restated here. No open wickr issue covers any of these three causes.

## Evidence
Cause 1 — wickr-network-create-and-validation#task#3: expected status 403, got 400; $.message value_mismatch (expected 'Access Denied', got '1 validation error detected: networkId').
Cause 2 — wickr-network-lifecycle-and-not-found-errors#task#5: expected status 401, got 404; $.message value_mismatch (expected 'Access Denied', got 'network 00000001 does not exist').
Cause 3 — wickr-network-lifecycle-and-not-found-errors#task#7: expected status 403, got 404; $.message value_mismatch (expected 'Network not found', got 'network 10000000 does not exist'); $headers.x-amzn-errortype shape_mismatch (got 'ResourceNotFoundError'); poll: status 404 not in [403] after 30 attempt(s) — expected body {'message': 'Network not found'}, got {'__type': 'ResourceNotFoundError', 'message': 'network 10000000 does not exist'}.

## Steps to Reproduce
1. GET /networks/{networkId} with a networkId that does not satisfy NetworkId (@length 8, @pattern ^[0-9]{8}$): AWS answers 403 {"message":"Access Denied"}; the emulator answers 400 '1 validation error detected: networkId'.
2. GET /networks/{networkId} with a well-formed 8-digit networkId that names no network: AWS answers 401 {"message":"Access Denied"}; the emulator answers 404 'network <id> does not exist'.
3. Create a network, DELETE /networks/{networkId}, then poll GET /networks/{networkId}: AWS converges on 403 {"message":"Network not found"}; the emulator returns 404 ResourceNotFoundError indefinitely.
4. Replay fixtures wickr-network-create-and-validation (task#3) and wickr-network-lifecycle-and-not-found-errors (task#5, task#7).
