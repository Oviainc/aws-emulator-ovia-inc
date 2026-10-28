Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.op-contract-delete-model apigateway.model-schema-unicode-round-trip
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway.op-contract-delete-model|_client_error#12|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: apigateway.model-schema-unicode-round-trip|run#16|AssertionError|assert err2.get("Code") == "NotFoundException", f"expected NotFoundException patching a deleted model, got {err2.get('Code')!r}"
Issue-Commit: 4f27818214051f2e31facb59d76008dc453b73ad

# apigateway Model lifecycle: 2 net-new root causes — DeleteModel never enforces the in-use ConflictException gate, and UpdateModel answers NotImplemented instead of the modeled NotFoundException

> _Note: this issue was identified against the code base at commit `4f27818214051f2e31facb59d76008dc453b73ad`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteModel

## Description
Two net-new root causes in the API Gateway `Model` resource lifecycle. Both are in the apigateway service; both must be closed before this issue is submitted.

Cause 1 of 2: DeleteModel accepts the deletion of a model that is still referenced by a Method's requestModels, where AWS refuses with ConflictException.
Failing check: apigateway.op-contract-delete-model
  AWS:      Smithy: `DeleteModel` (DELETE /restapis/{restApiId}/models/{modelName}, success 202) declares `ConflictException` (HTTP 409) — "The request configuration has conflicts. For details, see the accompanying error message." A model that is still named by a Method's `requestModels` entry is in use, so AWS refuses the delete with 409 `ConflictException` and leaves the model in place; the delete succeeds only once the referencing Method is removed. Recorded behavior: the refusal message's fixed text is `Cannot delete model '<modelName>', is referenced in method request: <resourcePath>/<httpMethod>` — the variable parts are the model name from the request, the path of the referencing resource, and that Method's HTTP verb, all taken from emulator state (for the API's ROOT resource the path segment is empty before the separator, which is what produces the doubled separator seen on a root-resource method, e.g. `//POST`). Those values are never fixed text; only the sentence around them is.
  Emulator: the DELETE is accepted. The emulator answers HTTP 202 with an empty body (`{'ResponseMetadata': {'HTTPStatusCode': 202, ...}}`) and removes the model, even though a Method on the API carries that model name in `/requestModels/application~1json`. No `ConflictException` is ever produced for this operation. The handler states this deliberately: `services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/delete_model.rs` reasons in comments that "no stored record of this service DEPENDS on this one and the modelled `ConflictException` has no documented rule to answer for here", and then removes the record unconditionally.
  Why:      a client that relies on AWS's referential-integrity gate silently loses it. Deleting a model that a Method still validates against is accepted, so the API is left pointing at a model name that no longer resolves; the request-validation behavior of that Method changes with no error surfaced. Code that treats the 409 as the signal to first detach the Method (or that asserts the model survived a rejected delete) takes the success path against the emulator and diverges from AWS, and any test of the in-use guard passes vacuously.
  Fix:      the `DeleteModel` handler's pre-delete validation — it must resolve, before removing the record, whether any Method (requestModels) or Method response (responseModels) on the same RestApi names this model, and return the modeled ConflictException with the message rule above instead of removing it. File citation under Traces.
  Traces:   apigateway.op-contract-delete-model (asserted `ConflictException` + the exact in-use message, then that a rejected delete leaves the model readable). Handler: services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/delete_model.rs lines 185-197 (the comment that waives ConflictException, and the unconditional `api.models.remove(...)`).

Cause 2 of 2: UpdateModel on a deleted/nonexistent model answers the unmodeled code NotImplemented instead of the modeled NotFoundException.
Failing check: apigateway.model-schema-unicode-round-trip
  AWS:      Smithy: `UpdateModel` (PATCH /restapis/{restApiId}/models/{modelName}, success 200) declares `NotFoundException` (HTTP 404) — "The requested resource is not found. Make sure that the request URI is correct." Patching a model name that does not exist on the API is a 404 `NotFoundException`. Recorded behavior: the message's fixed text is `Invalid model name specified: <modelName>`, with the model name taken from the request — the same message the emulator already returns correctly from `GetModel` for a deleted model in this very check.
  Emulator: the PATCH is refused with the unmodeled error code `NotImplemented` (HTTP 501, "operation UpdateModel is scaffolded but not implemented in this emulator"), so the not-found path is unreachable. Replay: `AssertionError: expected NotFoundException patching a deleted model, got 'NotImplemented'`.
  Why:      `NotImplemented` is not in this operation's modeled error set, so a client cannot classify it: SDK error handling keyed on `NotFoundException` (or on the 404 status) falls through to a generic/retryable branch, and a caller cannot distinguish "this model is gone" from a server fault. Every mutation of an existing model is equally unavailable.
  Fix:      implement the `UpdateModel` handler (resolve restApiId, then resolve modelName within that API, refusing with the modeled NotFoundException and the message rule above before applying patchOperations). NOTE: the open issue `20261027T101902Z-apigateway-updatemodel-stub.md` ("Implement apigateway UpdateModel: unimplemented scaffold stub") covers the same stub, but its Issue-Trace-Ids list only apigateway.update-method-response-model-and-validator, apigateway.update-model-and-validator-in-place and apigateway.update-model-invalid-schema-bad-request — it does NOT carry apigateway.model-schema-unicode-round-trip, which is why that check is recorded here. Closing that issue should also re-replay this check.
  Traces:   apigateway.model-schema-unicode-round-trip (the unicode schema round-trip, DeleteModel, and the GetModel 404 with its exact message all passed; the check fails only on the UpdateModel refusal). Handler: services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/update_model.rs.

## Evidence
Cause 1 — apigateway.op-contract-delete-model (verdict: mismatch):
  Expected (AWS): ClientError with Error.Code == "ConflictException" (HTTP 409) and Error.Message == "Cannot delete model '<modelName>', is referenced in method request: <resourcePath>/<httpMethod>"; the model remains readable via GetModel afterwards.
  Actual (emulator): the call SUCCEEDED —
    AssertionError: expected a ClientError, but the call SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 202, 'HTTPHeaders': {'content-type': 'application/json', 'content-length': '0', ...}, 'RetryAttempts': 0}}
  Setup that makes the model in use: CreateModel(name=<model>, contentType=application/json, schema=<draft-04 object>); PutMethod(POST on the root resource, authorizationType=NONE); PutIntegration(type=MOCK); UpdateMethod with patchOperations=[{op: add, path: /requestModels/application~1json, value: <model>}]; GetMethod confirms requestModels["application/json"] == <model>. DeleteModel is then accepted with 202.
  Model: DeleteModel errors = BadRequestException(400), ConflictException(409), NotFoundException(404), TooManyRequestsException(429), UnauthorizedException(401). ConflictException is modeled and never produced by the handler.

Cause 2 — apigateway.model-schema-unicode-round-trip (verdict: mismatch):
  Expected (AWS): UpdateModel(restApiId=<api>, modelName=<deleted model>, patchOperations=[{op: replace, path: /description, value: x}]) -> ClientError Error.Code == "NotFoundException" (HTTP 404), Error.Message == "Invalid model name specified: <modelName>".
  Actual (emulator): AssertionError: expected NotFoundException patching a deleted model, got 'NotImplemented' (HTTP 501, "operation UpdateModel is scaffolded but not implemented in this emulator").
  Earlier assertions in the same check PASSED: the unicode schema (title/description/property names with CJK, Cyrillic, accents and emoji) round-tripped through CreateModel/GetModel unmangled, and GetModel after DeleteModel returned NotFoundException with the exact message "Invalid model name specified: <modelName>" — so only the UpdateModel error path diverges.
  Model: UpdateModel errors = BadRequestException(400), ConflictException(409), LimitExceededException(429), NotFoundException(404), TooManyRequestsException(429), UnauthorizedException(401). NotImplemented is not among them.

## Steps to Reproduce
Cause 1 (DeleteModel in-use gate):
1. Boot the emulator and create a REST API: CreateRestApi(name=<unique>), note its id; GetResources -> root resource id.
2. CreateModel(restApiId=<api>, name="SimParityWidget", contentType="application/json", schema='{"$schema":"http://json-schema.org/draft-04/schema#","title":"Widget","type":"object"}').
3. PutMethod(restApiId=<api>, resourceId=<root>, httpMethod="POST", authorizationType="NONE") and PutIntegration(..., type="MOCK").
4. UpdateMethod(..., patchOperations=[{"op":"add","path":"/requestModels/application~1json","value":"SimParityWidget"}]) and confirm with GetMethod.
5. Call DeleteModel(restApiId=<api>, modelName="SimParityWidget").
   Observed: HTTP 202, model removed. Expected: HTTP 409 ConflictException "Cannot delete model 'SimParityWidget', is referenced in method request: //POST" (the path/verb taken from the referencing method), and GetModel still returning the model.
6. Re-replay: apigateway.op-contract-delete-model.

Cause 2 (UpdateModel not-found):
1. CreateRestApi, then CreateModel with any valid name/schema.
2. DeleteModel for that name (succeeds), then GetModel for it -> 404 NotFoundException "Invalid model name specified: <name>" (already correct).
3. Call UpdateModel(restApiId=<api>, modelName=<the deleted name>, patchOperations=[{"op":"replace","path":"/description","value":"x"}]).
   Observed: HTTP 501, Error.Code "NotImplemented". Expected: HTTP 404, Error.Code "NotFoundException", Message "Invalid model name specified: <name>".
4. Re-replay: apigateway.model-schema-unicode-round-trip.
