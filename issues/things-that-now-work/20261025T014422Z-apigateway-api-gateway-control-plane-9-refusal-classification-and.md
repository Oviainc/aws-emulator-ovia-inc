Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.create-rest-api-invalid-endpoint-configuration-type-rejected apigateway.put-gateway-response-invalid-status-code-fails apigateway.put-gateway-response-invalid-status-code-rejected apigateway.put-gateway-response-malformed-statuscode-rejected apigateway.put-integration-response-invalid-content-handling-rejected apigateway.update-vpc-link-invalid-patch-op-rejected apigateway.authorizer-deletion-blocked-while-referenced-rejected apigateway.create-deployment-without-methods-fails apigateway.create-vpc-link-invalid-target-arn-returns-badrequest apigateway.stage-repoint-unblocks-old-deployment-delete apigateway.delete-vpc-link-in-use-by-integration-conflict apigateway.integration-response-delete-removes-response apigateway.import-api-keys-duplicate-value-rejected apigateway.import-api-keys-rollback-on-failonwarnings-rejected apigateway.import-rest-api-resolves-aws-pseudo-parameters-and-reresolves-on-overwrite
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway.create-rest-api-invalid-endpoint-configuration-type-rejected|run#4|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}"
Issue-Key: apigateway.put-gateway-response-invalid-status-code-fails|run#10|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}"
Issue-Key: apigateway.put-gateway-response-invalid-status-code-rejected|run#7|AssertionError|assert err == "ValidationException", f"expected ValidationException, got {err!r}"
Issue-Key: apigateway.put-gateway-response-malformed-statuscode-rejected|run#5|AssertionError|assert code == "ValidationException", (
Issue-Key: apigateway.put-integration-response-invalid-content-handling-rejected|run#9|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}"
Issue-Key: apigateway.update-vpc-link-invalid-patch-op-rejected|run#8|AssertionError|assert (err.get("Code"), err.get("Message")) == ("ValidationException", expected), (
Issue-Key: apigateway.authorizer-deletion-blocked-while-referenced-rejected|_error#13|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: apigateway.create-deployment-without-methods-fails|_error#9|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: apigateway.create-vpc-link-invalid-target-arn-returns-badrequest|run#12|AssertionError|assert final["status"] == "FAILED", f"expected FAILED, got {final['status']}"
Issue-Key: apigateway.stage-repoint-unblocks-old-deployment-delete|run#19|AssertionError|assert revert_code == "BadRequestException", (
Issue-Key: apigateway.delete-vpc-link-in-use-by-integration-conflict|run#14|AssertionError|assert code == "BadRequestException", (
Issue-Key: apigateway.integration-response-delete-removes-response|run#16|AssertionError|assert update_code == "NotFoundException", f"expected NotFoundException on update, got {update_code}"
Issue-Key: api-gateway|ImportApiKeys|BadRequestException
Issue-Key: apigateway.import-api-keys-rollback-on-failonwarnings-rejected|run#9|AssertionError|assert message == (
Issue-Key: apigateway.import-rest-api-resolves-aws-pseudo-parameters-and-reresolves-on-overwrite|run#14|KeyError|assert gateway_response["responseTemplates"]["application/json"] == (
Issue-Commit: 2c888af5aa33aca2a1e4835e22187ff259bac9bc

# API Gateway control plane: 9 refusal-classification and missing-refusal causes — constraint violations answered as BadRequestException, four operations accept input real AWS refuses, two answer NotImplemented, and ImportApiKeys/PutRestApi semantics are wrong

> _Note: this issue was identified against the code base at commit `2c888af5aa33aca2a1e4835e22187ff259bac9bc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutGatewayResponse

## Description
Nine distinct causes in the API Gateway v1 control plane. Each is numbered with its own failing check ids; all nine must be closed before this issue is submitted.

Cause 1 of 9: a violation of an input member's modeled constraint is reported as BadRequestException instead of ValidationException.
AWS: Smithy models these members with constraint traits — PutGatewayResponse.statusCode @pattern "^[1-5]\\d\\d$"; PutIntegrationResponse.contentHandling targets the enum ContentHandlingStrategy; UpdateVpcLink.patchOperations[].op targets the Op enum; CreateRestApi.endpointConfiguration.types targets the EndpointType enum. A value outside a constraint trait is refused by AWS's constraint layer as ValidationException with HTTP 400, which is distinct from the operation's modeled BadRequestException (also 400) that reports a semantic refusal. The recorded responses show code ValidationException for all of these.
Emulator: answers 400 with code BadRequestException. For UpdateVpcLink it even builds AWS's constraint wording ("1 validation error detected: Value 'not-a-real-op' at 'updateVpcLinkInput.patchOperations.1.member.op' failed to satisfy constraint: Member must satisfy enum value set: [add, remove, move, test, replace, copy]") but labels it BadRequestException.
Why: clients branch on the error code — a constraint violation is a caller bug that must never be retried and is not the same class as a semantic refusal. Error-code-driven retry/classification logic takes the wrong branch, and SDK error types do not match.
Fix: the shared input-constraint validation layer for this service (pattern/enum/range checks applied before the operation handler) must emit ValidationException, leaving BadRequestException for the handlers' semantic refusals.
Checks: apigateway.create-rest-api-invalid-endpoint-configuration-type-rejected, apigateway.put-gateway-response-invalid-status-code-fails, apigateway.put-gateway-response-invalid-status-code-rejected, apigateway.put-gateway-response-malformed-statuscode-rejected, apigateway.put-integration-response-invalid-content-handling-rejected, apigateway.update-vpc-link-invalid-patch-op-rejected

Cause 2 of 9: DeleteAuthorizer succeeds while a method still references the authorizer.
AWS: refuses the delete while any method's authorizerId still points at it. Smithy: DeleteAuthorizer models BadRequestException (400) and ConflictException (409) alongside its 202 success. Recorded behavior: the call is refused, the authorizer survives. Hypothesis: the code is the 400 BadRequestException (API Gateway reports referential refusals that way for authorizers); the requirement asserted here is only that the call is refused with one of the modeled 4xx and the authorizer remains.
Emulator: returns HTTP 202 and deletes the authorizer, leaving the referencing method pointing at a dangling id.
Why: a client can silently break every CUSTOM-authorized method on an API; later invokes fail with no diagnostic tying them to the deleted authorizer.
Fix: DeleteAuthorizer's handler — a referential-integrity check over the API's methods before removing the authorizer from state.
Checks: apigateway.authorizer-deletion-blocked-while-referenced-rejected

Cause 3 of 9: CreateDeployment succeeds for a REST API that contains no methods.
AWS: refuses to deploy an API with no method. Smithy: CreateDeployment models BadRequestException (400). Recorded behavior: the call is refused and no deployment is created.
Emulator: returns HTTP 201 with a fresh deployment id (e.g. {'id': '5mw7f4baaa', 'createdDate': ...}).
Why: a client gets a deployment id for an API that cannot serve anything, so a deploy pipeline reports success where AWS would have failed the step, and the stage it then creates is permanently empty.
Fix: CreateDeployment's handler — reject when the API's resource tree has no method before allocating a deployment.
Checks: apigateway.create-deployment-without-methods-fails

Cause 4 of 9: a VPC link created with an invalid target ARN settles AVAILABLE instead of FAILED.
AWS: CreateVpcLink is asynchronous; a target ARN that does not name a usable NLB drives the link to status FAILED (Smithy: VpcLink.status targets the VpcLinkStatus enum, with statusMessage carrying the reason).
Emulator: the link converges to AVAILABLE.
Why: a client that polls GetVpcLink until it leaves PENDING sees a healthy link that can never carry traffic, and never surfaces the provisioning failure.
Fix: CreateVpcLink's provisioning state machine — validate the target ARNs and transition to FAILED with statusMessage instead of unconditionally reaching AVAILABLE.
Checks: apigateway.create-vpc-link-invalid-target-arn-returns-badrequest

Cause 5 of 9: repointing a stage at a deleted deployment is refused with the wrong error class.
AWS: refuses with BadRequestException (400). Smithy: UpdateStage models both BadRequestException (400) and NotFoundException (404); the recorded response for a patch naming a deployment id that was deleted is BadRequestException.
Emulator: answers NotFoundException.
Why: the client cannot distinguish "the stage/API does not exist" (404, abandon) from "this patch value is not usable" (400, fix the request), so recovery logic mis-routes.
Fix: UpdateStage's /deploymentId patch validation.
Checks: apigateway.stage-repoint-unblocks-old-deployment-delete

Cause 6 of 9: two operations answer NotImplemented where the modeled 4xx is required, so the behaviour behind them is unobservable.
AWS: DeleteVpcLink refuses a delete while an Integration still references the link (Smithy: DeleteVpcLink models BadRequestException 400 and ConflictException 409); UpdateIntegrationResponse on a deleted integration response answers NotFoundException (404).
Emulator: both answer the scaffold error code NotImplemented.
Why: a client sees an unmodeled error code for a path AWS answers with a modeled refusal, so both the refusal and the surrounding state transition are untestable. The open issues 20261023T210711Z-apigateway-updateintegrationresponse-stub.md and 20261023T125454Z-apigateway-createvpclink-stub.md name adjacent stubs but neither carries these check ids.
Fix: the DeleteVpcLink and UpdateIntegrationResponse handlers.
Checks: apigateway.delete-vpc-link-in-use-by-integration-conflict, apigateway.integration-response-delete-removes-response

Cause 7 of 9: ImportApiKeys refuses a payload whose two rows share one key value instead of de-duplicating it.
AWS: ImportApiKeys enforces uniqueness only within the submitted payload, and does so by collapsing colliding rows to a single ApiKey in which the LAST colliding row's attributes win; the call returns 201 with exactly one entry in "ids" and an EMPTY "warnings" list (Smithy: ImportApiKeys output ApiKeyIds{ids, warnings}; success code 201). It performs no account-wide, cross-resource value-uniqueness check at all.
Emulator: rejects the whole call with HTTP 400 BadRequestException, message e.g. 'Errors found during import: row 2: an API Key already exists for the given value'.
Why: a client importing a CSV that legitimately repeats a value gets the entire batch refused instead of the documented last-row-wins dedup, so no keys are created.
Fix: ImportApiKeys' CSV import handler — in-payload dedup keyed on the key value with last-row-wins, no cross-resource uniqueness check.
Checks: apigateway.import-api-keys-duplicate-value-rejected

Cause 8 of 9: ImportApiKeys with failOnWarnings=true classifies a structurally-invalid record as an error rather than as the warning it refused to tolerate.
AWS: a record whose key is shorter than 20 characters is a WARNING; with failOnWarnings=true the call fails with BadRequestException whose message names the warning it refused to tolerate ('Warnings found during import:\n\tSkipped invalid record: 2. API Key value should be at least 20 characters'), the invalid record is skipped, and — the load-bearing part — the other, valid record in the same payload has already been committed (ImportApiKeys is not transactional).
Emulator: refuses with BadRequestException but reports it as an import ERROR — 'Errors found during import: row 2: key must be between 20 and 128 characters, got 12' — i.e. the warning channel and the failOnWarnings contract are not modelled.
Why: a client that imports with failOnWarnings=false expects the valid rows to land and the bad row to be reported in "warnings"; here the distinction between warning and error does not exist, so the outcome of the whole import differs.
Fix: ImportApiKeys' handler — a warnings channel separate from errors, per-record commit, and failOnWarnings gating on that channel.
Checks: apigateway.import-api-keys-rollback-on-failonwarnings-rejected

Cause 9 of 9: PutRestApi(mode=overwrite) of an OpenAPI document that declares gateway responses does not materialize them, so the imported gateway response has no responseTemplates.
AWS: an import carrying x-amazon-apigateway-gateway-responses creates the gateway responses with their response templates, resolving AWS pseudo-parameters in the template text, and re-resolves them on a subsequent overwrite import. Recorded behavior: GetGatewayResponse returns a populated responseTemplates map keyed by content type.
Emulator: the gateway response read back after the import carries no responseTemplates member at all (the check's lookup raises KeyError: 'responseTemplates').
Why: an API imported from a spec silently loses its customized gateway responses, so error bodies at the edge differ from the spec's and pseudo-parameter substitution never happens.
Fix: the PutRestApi/ImportRestApi OpenAPI importer — the gateway-responses extension branch and its pseudo-parameter resolution, plus the GetGatewayResponse serializer for the templates map. Note the open issue 20261023T151100Z-apigateway-api-gateway-putrestapi-overwrite-imports-paths-but-no.md covers integrations lost on overwrite import, not gateway responses; it does not carry this check id.
Checks: apigateway.import-rest-api-resolves-aws-pseudo-parameters-and-reresolves-on-overwrite

## Evidence
Cause 1 — constraint violations mis-coded:
  apigateway.create-rest-api-invalid-endpoint-configuration-type-rejected: expected ValidationException, got 'BadRequestException'
  apigateway.put-gateway-response-invalid-status-code-fails: expected ValidationException, got 'BadRequestException'
  apigateway.put-gateway-response-invalid-status-code-rejected: expected ValidationException, got 'BadRequestException'
  apigateway.put-gateway-response-malformed-statuscode-rejected: expected ValidationException for a malformed statusCode, got BadRequestException
  apigateway.put-integration-response-invalid-content-handling-rejected: expected ValidationException, got 'BadRequestException'
  apigateway.update-vpc-link-invalid-patch-op-rejected: expected ValidationException for an invalid JSON-Patch op, got {'Code': 'BadRequestException', 'Message': "1 validation error detected: Value 'not-a-real-op' at 'updateVpcLinkInput.patchOperations.1.member.op' failed to satisfy constraint: Member must satisfy enum value set: [add, remove, move, test, replace, copy]"}

Cause 2 — DeleteAuthorizer while referenced:
  expected: the call to be rejected
  actual:   SUCCEEDED, HTTPStatusCode 202, empty body

Cause 3 — CreateDeployment with no methods:
  expected: the call to be rejected
  actual:   SUCCEEDED, HTTPStatusCode 201, {'id': '5mw7f4baaa', 'createdDate': ...}

Cause 4 — VpcLink with an invalid target ARN:
  expected: status FAILED
  actual:   status AVAILABLE

Cause 5 — stage repointed at a deleted deployment:
  expected: BadRequestException
  actual:   NotFoundException

Cause 6 — NotImplemented instead of a modeled 4xx:
  apigateway.delete-vpc-link-in-use-by-integration-conflict: expected BadRequestException deleting a VpcLink with an active Integration reference, got 'NotImplemented'
  apigateway.integration-response-delete-removes-response: expected NotFoundException on update, got NotImplemented

Cause 7 — ImportApiKeys in-payload duplicate value:
  expected: 201, ids == 1 entry (last row's name/value wins), warnings == []
  actual:   BadRequestException 400 — 'Errors found during import: row 2: an API Key already exists for the given value'

Cause 8 — ImportApiKeys failOnWarnings:
  expected message: 'Warnings found during import:\n\tSkipped invalid record: 2. API Key value should be at least 20 characters'
  actual message:   'Errors found during import: row 2: key must be between 20 and 128 characters, got 12'

Cause 9 — PutRestApi overwrite loses gateway-response templates:
  expected: GetGatewayResponse(...)['responseTemplates']['application/json'] populated with the pseudo-parameters resolved
  actual:   KeyError: 'responseTemplates' — the member is absent from the read-back

## Steps to Reproduce
Cause 1: PutGatewayResponse with statusCode="6xx" (outside @pattern ^[1-5]\d\d$); observe code BadRequestException where ValidationException is required. Same with PutIntegrationResponse contentHandling="BOGUS", UpdateVpcLink patchOperations[].op="not-a-real-op", CreateRestApi endpointConfiguration.types=["BOGUS"].
Cause 2: CreateRestApi, CreateAuthorizer, PutMethod with authorizationType=CUSTOM and that authorizerId, then DeleteAuthorizer; observe 202 instead of a refusal.
Cause 3: CreateRestApi, then CreateDeployment with no method defined; observe 201.
Cause 4: CreateVpcLink with a targetArn that is not a usable NLB, poll GetVpcLink; observe AVAILABLE instead of FAILED.
Cause 5: create two deployments, point a stage at the second, delete the first, then UpdateStage replace /deploymentId with the deleted id; observe NotFoundException instead of BadRequestException.
Cause 6: DeleteVpcLink while an Integration references it; UpdateIntegrationResponse after DeleteIntegrationResponse. Both answer NotImplemented.
Cause 7: ImportApiKeys format=csv with body "name,key\nA,<40-char value>\nB,<same value>\n"; observe BadRequestException instead of 201 with one id and empty warnings.
Cause 8: ImportApiKeys format=csv failOnWarnings=true with one valid row and one row whose key is 12 characters; inspect the message and then GetApiKeys for the valid row.
Cause 9: PutRestApi mode=overwrite with a spec carrying x-amazon-apigateway-gateway-responses with a responseTemplates entry, then GetGatewayResponse; observe responseTemplates absent.
