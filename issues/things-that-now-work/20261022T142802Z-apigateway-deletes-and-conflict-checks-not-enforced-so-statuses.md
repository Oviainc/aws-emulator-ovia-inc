Issue-Source: trace-sweep-divergence
Issue-Trace: apigateway-resource-method-crud-lifecycle
Issue-Step: task#11
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigateway-resource-method-crud-lifecycle apigateway-resource-method-integration-full-lifecycle apigateway-stage-api-key-toggle apigateway-stage-restricted-api-key-toggle apigateway-stage-method-settings-duplicate-spelling
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway|CreateResource|4xx->2xx|$.message
Issue-Key: apigateway|CreateResource|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: apigateway|CreateResource|4xx->2xx|status
Issue-Key: apigateway|GetMethod|4xx->2xx|$.message
Issue-Key: apigateway|GetMethod|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: apigateway|GetMethod|4xx->2xx|status
Issue-Key: apigateway|GetResource|4xx->2xx|$.message
Issue-Key: apigateway|GetResource|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: apigateway|GetResource|4xx->2xx|status
Issue-Key: apigateway|CreateApiKey|4xx->2xx|$.message
Issue-Key: apigateway|CreateApiKey|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: apigateway|CreateApiKey|4xx->2xx|status
Issue-Key: apigateway|DeleteRestApi|4xx->2xx|status
Issue-Commit: c73ebf67a23d923b166243bec836d38257c16c68

# Deletes and conflict checks not enforced so statuses diverge

> _Note: this issue was identified against the code base at commit `c73ebf67a23d923b166243bec836d38257c16c68`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateResource

## Description
Five causes, one shared fault: the emulator answers 2xx on requests AWS rejects, because existence and uniqueness are not checked (and because deletes do not remove the entity from state). Every cause here is a wrong HTTP status plus a missing modeled error envelope, which is the class of divergence that breaks client control flow outright.

Cause 1 of 5: duplicate CreateResource under the same parent is accepted.
AWS: CreateResource is POST /restapis/{restApiId}/resources/{parentId} with required pathPart and success 201; ConflictException (httpError 409) is a modeled error (Smithy: CreateResource errors BadRequestException 400, ConflictException 409, LimitExceededException 429, NotFoundException 404, TooManyRequestsException 429, UnauthorizedException 401). Creating a second child with a pathPart that already exists under that parent is rejected with HTTP 409 and header `x-amzn-errortype: ConflictException`. The recorded message is `Another resource with the same parent already has this name: hello` (example text).
Emulator: HTTP 201 with a freshly created resource, no error body and no `x-amzn-errortype` header (`status: expected 409, got 201`).
Why: the API silently grows two sibling resources with the same path segment, so routing becomes ambiguous and the client's idempotency/conflict handling never runs. A caller that relies on 409 to detect "already created" treats the duplicate as a new resource and leaks it.
Fix: the CreateResource handler's uniqueness check against existing children of parentId, raising the modeled ConflictException (409).
Traces: fixture `apigateway-resource-method-crud-lifecycle`, step `task#11`.

Cause 2 of 5: GetMethod succeeds after the method was deleted.
AWS: after DeleteMethod the method is gone, so GetMethod answers HTTP 404 with `x-amzn-errortype: NotFoundException` (NotFoundException is modeled at httpError 404 on the method operations). Recorded message `Invalid Method identifier specified` (example text).
Emulator: HTTP 200 with a Method body (`status: expected 404, got 200`; `$.message` and `$headers.x-amzn-errortype` missing).
Why: DeleteMethod is a no-op on state, so a client that deletes and re-creates a method, or that probes existence via GetMethod, sees a method AWS no longer has — and never gets the 404 its error handling is written against.
Fix: the DeleteMethod handler's removal from state and the GetMethod lookup's not-found path.
Traces: fixture `apigateway-resource-method-crud-lifecycle`, step `task#13` (also apigateway-resource-method-integration-full-lifecycle#task#33).

Cause 3 of 5: GetResource succeeds after the resource was deleted.
AWS: after DeleteResource, GetResource answers HTTP 404 with `x-amzn-errortype: NotFoundException`; recorded message `Invalid Resource identifier specified` (example text).
Emulator: HTTP 200 with a Resource body (`status: expected 404, got 200`, no error type header).
Why: same as Cause 2 one level up the tree — deleted resources stay visible, so teardown/rebuild flows and existence probes diverge.
Fix: the DeleteResource handler's removal from state (including its subtree) and the GetResource not-found path.
Traces: fixture `apigateway-resource-method-crud-lifecycle`, step `task#15` (also apigateway-resource-method-integration-full-lifecycle#task#35).

Cause 4 of 5: CreateApiKey accepts stageKeys that reference a stage which does not exist.
AWS: CreateApiKey input stageKeys is ListOfStageKeys and NotFoundException (404) is a modeled error on CreateApiKey; associating a key with an unknown restApiId/stageName pair is rejected with HTTP 404 and `x-amzn-errortype: NotFoundException`, recorded message `Invalid stage identifier specified` (example text).
Emulator: HTTP 201, the key is created with the dangling association (`status: expected 404, got 201`).
Why: the referential check is skipped, so clients can create keys bound to stages that do not exist and never learn the association is invalid; later reads report a stage association AWS would never have stored.
Fix: the CreateApiKey handler's validation of each requested stage key against stage state, raising the modeled NotFoundException (404).
Traces: fixture `apigateway-stage-api-key-toggle`, step `task#8` (also apigateway-stage-restricted-api-key-toggle#task#8).

Cause 5 of 5: DeleteRestApi on an API that no longer exists returns success.
AWS: DeleteRestApi is DELETE /restapis/{restApiId} with success code 202 and NotFoundException modeled at 404; deleting an unknown or already-deleted id answers HTTP 404, not 202.
Emulator: HTTP 202 (`status: expected 404, got 202`).
Why: delete is unconditionally successful, so a client cannot distinguish "I deleted it" from "it was never there" and any cleanup logic that keys off 404 to stop retrying misbehaves. Combined with Causes 2/3 it indicates the delete paths do not consult state at all.
Fix: the DeleteRestApi handler's existence lookup before deletion, raising the modeled NotFoundException (404).
Traces: fixture `apigateway-stage-method-settings-duplicate-spelling`, step `teardown#12`.

## Evidence
Representative (Cause 1): fixture apigateway-resource-method-crud-lifecycle step task#11, operation CreateResource, expected HTTP 409 / actual HTTP 201, divergence_key `service=apigateway|CreateResource|status=409->201|keys=$.message,$headers.x-amzn-errortype,status`, detail `status: expected 409, got 201` / `$.message: missing (expected 'Another resource with the same parent already has this name: hello', got None)` / `$headers.x-amzn-errortype: missing (expected 'ConflictException', got None)`.
Cause 2: apigateway-resource-method-crud-lifecycle#task#13 — GetMethod expected 404, got 200; missing `$headers.x-amzn-errortype: NotFoundException`.
Cause 3: apigateway-resource-method-crud-lifecycle#task#15 — GetResource expected 404, got 200; missing `$headers.x-amzn-errortype: NotFoundException`.
Cause 4: apigateway-stage-api-key-toggle#task#8 — CreateApiKey expected 404, got 201; missing `$headers.x-amzn-errortype: NotFoundException`.
Cause 5: apigateway-stage-method-settings-duplicate-spelling#teardown#12 — DeleteRestApi expected 404, got 202.
Smithy evidence: CreateResource POST /restapis/{restApiId}/resources/{parentId} code 201 with modeled ConflictException (409) and NotFoundException (404); CreateApiKey POST /apikeys code 201 with modeled NotFoundException (404) and input member stageKeys (ListOfStageKeys); DeleteRestApi DELETE /restapis/{restApiId} code 202 with modeled NotFoundException (404).
Recorded error messages are quoted as examples; the requirements asserted here are the HTTP statuses and the `x-amzn-errortype` error codes.
Run summary: trace_set_id=simparity-wire-aws-v1, emulator_up=true, bootstrap_signal=false, state_reset_failures=0.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay wire fixture `apigateway-resource-method-crud-lifecycle`: step `task#11` creates a second child resource with an already-used pathPart and the emulator answers 201 where the recording has 409/ConflictException; step `task#13` GETs a deleted method and gets 200 instead of 404/NotFoundException; step `task#15` GETs a deleted resource and gets 200 instead of 404/NotFoundException.
3. Replay `apigateway-stage-api-key-toggle` step `task#8`: CreateApiKey with a stageKey for a nonexistent stage answers 201 instead of 404/NotFoundException.
4. Replay `apigateway-stage-method-settings-duplicate-spelling` step `teardown#12`: DeleteRestApi for a missing API answers 202 instead of 404.
5. All five units must return the recorded status and error code before this issue is closed.
