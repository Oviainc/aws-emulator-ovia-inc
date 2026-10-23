Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.apigw-integration-response-parameters-evaluate-context-api-account-and-route-vars apigateway.apigw-response-header-mapping-evaluates-context-and-stage-variables apigateway.concurrent-put-integration-response-race apigateway.concurrent-put-integration-response-race-last-write-wins apigateway.untag-resource-unknown-key-fails apigateway.tags-unicode-value-round-trip apigateway.stage-variables-empty-map-vs-absent-distinguished apigateway.stage-variables-empty-map-vs-absent-preserved apigateway.stage-variables-empty-vs-absent-preserved apigateway.get-method-unauthorized-vs-not-found apigateway.get-documentation-version-unauthorized apigateway.create-stage-nonexistent-deployment-rejected apigateway.op-contract-getSdkTypes apigateway.put-method-response-invalid-model-reference-rejected apigateway.deep-apigateway-3-apigw-private-public-host-rejected
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: api-gateway|PutIntegrationResponse|BadRequestException
Issue-Key: api-gateway|PutIntegrationResponse|ConflictException
Issue-Key: apigateway.concurrent-put-integration-response-race-last-write-wins|_race#11|AssertionError|raise AssertionError(
Issue-Key: api-gateway|UntagResource|NotFoundException
Issue-Key: apigateway.tags-unicode-value-round-trip|_client_error#10|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: apigateway.stage-variables-empty-map-vs-absent-distinguished|run#19|AssertionError|assert got_empty.get("variables") == {}, (
Issue-Key: apigateway.stage-variables-empty-map-vs-absent-preserved|run#12|AssertionError|assert "variables" in resp_empty and resp_empty["variables"] == {}, (
Issue-Key: apigateway.stage-variables-empty-vs-absent-preserved|_error#5|AssertionError|raise AssertionError(f"expected this call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: apigateway.get-method-unauthorized-vs-not-found|_error#12|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: apigateway.get-documentation-version-unauthorized|_error#9|AssertionError|raise AssertionError(f"expected this call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: apigateway.create-stage-nonexistent-deployment-rejected|run#10|AssertionError|assert code == "BadRequestException", f"CODE={code}"
Issue-Key: api-gateway|GetSdkTypes|BadRequestException
Issue-Key: apigateway.deep-apigateway-3-apigw-private-public-host-rejected|_error#8|AssertionError|raise AssertionError("call succeeded where real AWS rejects it")
Issue-Related: 20261023T125454Z-apigateway-api-gateway-control-plane-accepts-inputs-real-aws.md
Issue-Commit: 8bb3e31d2bc3a99611966740f58be573e66087fa

# API Gateway control plane: 10 fidelity causes — context.* response-parameter mappings refused, PutIntegrationResponse not an upsert, UntagResource 404s on an unknown key, no caller authorization, and five over-permissive/mis-coded validations

> _Note: this issue was identified against the code base at commit `8bb3e31d2bc3a99611966740f58be573e66087fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutIntegrationResponse

## Description
Ten independent, net-new control-plane fidelity defects in the apigateway emulator, all reached with ordinary SDK calls against the control plane (the service answers; it answers wrongly). Each cause below is a separate root cause and must be closed separately. Open issues 20261023T125454Z-apigateway-api-gateway-control-plane-accepts-inputs-real-aws.md, 20261023T151100Z-apigateway-api-gateway-8-control-plane-causes-seven-operations.md and 20261023T170254Z-apigateway-api-gateway-nine-control-plane-fidelity-causes-seven.md cover other apigateway control-plane causes; none of them carries the replay keys listed here.

Cause 1 of 10: response-parameter mapping expressions rooted at `context.` are refused.
AWS: PutIntegrationResponse.responseParameters is MapOfStringToString (Smithy: PutIntegrationResponse, http PUT /restapis/{restApiId}/resources/{resourceId}/methods/{httpMethod}/integration/responses/{statusCode}, success 201; errors BadRequestException 400 / ConflictException 409 / NotFoundException 404 / UnauthorizedException 401). Recorded behavior only: real AWS accepts a value of the form `context.<variable>` (e.g. `context.httpMethod`, `context.requestId`) for a method-response header mapping, alongside static `'literal'`, `integration.response.header.{name}` and `integration.response.body.{JSONPath}` sources, and evaluates it at invoke time.
Emulator: the call is refused with HTTP 400 `BadRequestException`, example message "Invalid mapping expression specified: context.httpMethod; an integration response parameter value must be a static value enclosed in single quotes, integration.response.header.{name} or integration.response.body.{JSON-expression}" — i.e. `context.*` is simply absent from the accepted grammar.
Why: a client cannot configure any response header fed from request context (request id, API id, account id, stage/route variables). The API is unconfigurable, not merely different: the configuration call itself fails, so nothing downstream can be built.
Fix: the responseParameters mapping-expression validator used by the PutIntegrationResponse handler (and the same validator shared with method/integration response-parameter writes) must accept the `context.<variable>` source.
Traces: apigateway.apigw-integration-response-parameters-evaluate-context-api-account-and-route-vars, apigateway.apigw-response-header-mapping-evaluates-context-and-stage-variables.

Cause 2 of 10: PutIntegrationResponse is not an upsert — a second write for the same statusCode is refused with 409.
AWS: Smithy: PutIntegrationResponse is an idempotent PUT keyed by {restApiId, resourceId, httpMethod, statusCode} returning 201 with the IntegrationResponse. Recorded behavior only: repeating the call for the same statusCode replaces the stored response (last write wins); two concurrent writers both complete and the stored response ends up wholly one writer's payload.
Emulator: the second write answers HTTP 409 `ConflictException`, example message "Integration response already exists for this method". Under two concurrent writers one of them never completes even after 6 retries.
Why: a client that re-applies its configuration (every IaC/CLI re-run, every retry after a timeout) is refused, and there is no supported way to change an integration response's templates in place.
Fix: the PutIntegrationResponse handler and its state write — replace the create-only insert with a keyed replace.
Traces: apigateway.concurrent-put-integration-response-race, apigateway.concurrent-put-integration-response-race-last-write-wins.

Cause 3 of 10: UntagResource for a tag key that was never set is refused instead of being a no-op.
AWS: Smithy: UntagResource, http DELETE /tags/{resourceArn}, success 204 with an empty body; NotFoundException 404 is modeled but, per the recording, is not used for an unset tag key — removing a key that is not present succeeds and leaves existing tags untouched.
Emulator: HTTP 404 `NotFoundException`, example message "Invalid tag key(s) specified: sim2real-never-set-key".
Why: idempotent cleanup/teardown paths break — every repeated untag, and every untag issued without first reading the tag set, fails with an error AWS never returns.
Fix: the UntagResource handler — removing an absent key must succeed (204) rather than raise.
Traces: apigateway.untag-resource-unknown-key-fails.

Cause 4 of 10: TagResource accepts a tag value AWS rejects.
AWS: Smithy: TagResource, http PUT /tags/{resourceArn}, success 204; error BadRequestException 400. Recorded behavior only: AWS's tag character class admits unicode letters/digits/whitespace and a small punctuation set but refuses symbol-category characters (emoji), answering 400 BadRequestException.
Emulator: the call succeeds with HTTP 204 and the emoji value is stored.
Why: over-permissive — a value that would be refused in production is accepted here, so a client's input validation is never exercised and the emulator's state diverges from what AWS would hold.
Fix: TagResource input validation (tag key/value character class).
Traces: apigateway.tags-unicode-value-round-trip.

Cause 5 of 10: stage `variables` loses the empty-map-vs-absent distinction, and an empty variable value is accepted.
AWS: Smithy: CreateStage.variables is MapOfStringToString, Stage.variables likewise. Recorded behavior only: a stage created with `variables={}` reads back from GetStage with `variables` present and equal to `{}`, distinct from a stage created with variables omitted; and a variable whose value is the empty string is refused.
Emulator: GetStage returns no `variables` member at all for the explicitly-empty stage (botocore surfaces it as absent/None), so the two stages are indistinguishable; and CreateStage with `variables={"emptyVar": ""}` succeeds (HTTP 201) instead of being refused.
Why: a client that reads a stage to decide whether variables were ever configured gets the wrong answer, and round-tripping GetStage into CreateStage/UpdateStage silently drops the distinction.
Fix: Stage serialization (do not elide an empty map) plus CreateStage/UpdateStage variable-value validation.
Traces: apigateway.stage-variables-empty-map-vs-absent-distinguished, apigateway.stage-variables-empty-map-vs-absent-preserved, apigateway.stage-variables-empty-vs-absent-preserved.

Cause 6 of 10: read operations enforce no caller authorization.
AWS: Smithy: GetMethod and GetDocumentationVersion both model UnauthorizedException (HTTP 401). Recorded behavior only: a caller whose credentials carry no apigateway permission is refused rather than served.
Emulator: both calls succeed with HTTP 200 and return the full representation (GetMethod returns the method with its integration; GetDocumentationVersion returns the version).
Why: an authorization-negative test passes against the emulator and fails in production; nothing that depends on refusal (deny-by-default checks, least-privilege verification) can be exercised.
Fix: the authorization check in the GetMethod and GetDocumentationVersion handlers (shared request-authorization layer).
Traces: apigateway.get-method-unauthorized-vs-not-found, apigateway.get-documentation-version-unauthorized.

Cause 7 of 10: CreateStage with an unknown deploymentId returns the wrong error code.
AWS: Smithy: CreateStage models both BadRequestException (400) and NotFoundException (404); deploymentId is a required input member. Recorded behavior only: a deploymentId that does not exist is refused as `BadRequestException` (400), not as a missing resource.
Emulator: HTTP 404 `NotFoundException`.
Why: a client branching on the error code (retryable-vs-fix-your-input) takes the wrong branch, and "stage not created because the deployment id is bad" becomes indistinguishable from "the REST API is gone".
Fix: the CreateStage handler's deploymentId resolution — classify an unresolvable deploymentId as BadRequestException.
Traces: apigateway.create-stage-nonexistent-deployment-rejected.

Cause 8 of 10: GetSdkTypes rejects an unrecognized `position` token.
AWS: Smithy: GetSdkTypes, http GET /sdktypes, success 200, input `position` is an optional httpQuery parameter. Recorded behavior only: the SDK-type catalog is a small fixed list that is not truly paginated, so an unrecognized `position` is ignored and the full catalog is still returned with 200.
Emulator: HTTP 400 `BadRequestException`, example message "Invalid position: not-a-valid-pagination-token".
Why: a client that replays or persists a stale pagination token gets a hard failure on a listing AWS always serves.
Fix: the GetSdkTypes handler's `position` handling.
Traces: apigateway.op-contract-getSdkTypes.

Cause 9 of 10: PutMethodResponse accepts responseModels naming a model that does not exist.
AWS: Smithy: PutMethodResponse.responseModels is MapOfStringToString; errors include BadRequestException 400 and NotFoundException 404. Recorded behavior only: a responseModels entry whose value is not an existing model name on that REST API is refused.
Emulator: the call succeeds with HTTP 201 and echoes `{'statusCode': '200', 'responseModels': {'application/json': 'NoSuchModelXYZ123'}}`.
Why: over-permissive — a broken method response is accepted and only surfaces later (or never), so a client cannot rely on the emulator to catch a mistyped model reference.
Fix: PutMethodResponse input validation — resolve each responseModels value against the API's models.
Traces: apigateway.put-method-response-invalid-model-reference-rejected.

Cause 10 of 10: a PRIVATE REST API can be deployed with no resource policy attached.
AWS: Smithy: CreateDeployment, http POST /restapis/{restApiId}/deployments, success 201; error BadRequestException 400. Recorded behavior only: for a REST API whose endpointConfiguration.types is [PRIVATE] and which has no resource policy, CreateDeployment is refused with 400 BadRequestException and nothing is deployed; it succeeds only once a policy is attached.
Emulator: CreateDeployment succeeds, so the refusal that gates private-endpoint deployment never happens.
Why: over-permissive — the private-API deployment precondition is not enforced, so a configuration that cannot be deployed in production deploys here and the whole private-endpoint access path is unverifiable.
Fix: the CreateDeployment handler — gate deployment of a PRIVATE-endpoint REST API on the presence of a resource policy.
Traces: apigateway.deep-apigateway-3-apigw-private-public-host-rejected.

## Evidence
Cause 1: expected PutIntegrationResponse to accept responseParameters value `context.httpMethod` / `context.requestId`; got HTTP 400 BadRequestException "Invalid mapping expression specified: context.httpMethod; an integration response parameter value must be a static value enclosed in single quotes, integration.response.header.{name} or integration.response.body.{JSON-expression}".
Cause 2: expected the 2nd/concurrent PutIntegrationResponse for statusCode 200 to replace the stored response (201); got HTTP 409 ConflictException "Integration response already exists for this method"; concurrent run reported "racer 1 never completed PutIntegrationResponse after 6 attempts".
Cause 3: expected UntagResource(tagKeys=['<never-set-key>']) to succeed 204; got HTTP 404 NotFoundException "Invalid tag key(s) specified: sim2real-never-set-key".
Cause 4: expected TagResource(tags={'emoji': '\ud83c\udf89\ud83d\udd25'}) to be refused 400 BadRequestException; got HTTP 204 success.
Cause 5: expected GetStage of a stage created with variables={} to return variables == {}; got variables absent (None). Expected CreateStage(variables={'emptyVar': ''}) to be refused; got HTTP 201 with variables {'emptyVar': ''}.
Cause 6: expected GetMethod / GetDocumentationVersion to be refused for an unauthorized caller; both succeeded HTTP 200 and returned the full representation.
Cause 7: expected CreateStage(deploymentId=<nonexistent>) -> BadRequestException; got NotFoundException.
Cause 8: expected GetSdkTypes(position='not-a-valid-pagination-token') -> HTTP 200 with the full catalog; got HTTP 400 BadRequestException "Invalid position: not-a-valid-pagination-token".
Cause 9: expected PutMethodResponse(responseModels={'application/json': 'NoSuchModelXYZ123'}) to be refused; got HTTP 201 success.
Cause 10: expected CreateDeployment on a PRIVATE REST API with no resource policy -> BadRequestException; the call succeeded.

## Steps to Reproduce
Boot the emulator and replay the behavioral checks listed in trace_ids, e.g.:
1. Cause 1: create_rest_api -> put_method -> put_integration(type=MOCK) -> put_method_response(statusCode='200', responseParameters={'method.response.header.X-Ctx': False}) -> put_integration_response(statusCode='200', responseParameters={'method.response.header.X-Ctx': 'context.httpMethod'}) and observe the 400.
2. Cause 2: after a successful put_integration_response(statusCode='200'), call put_integration_response again for statusCode='200' with different responseTemplates and observe the 409.
3. Cause 3: tag_resource on a REST API ARN, then untag_resource(tagKeys=['never-set-key']) and observe the 404.
4. Cause 4: tag_resource(tags={'emoji': '\ud83c\udf89'}) and observe the 204.
5. Cause 5: create_deployment, then create_stage(variables={}) and get_stage; then create_stage(variables={'emptyVar': ''}).
6. Cause 6: call get_method / get_documentation_version with credentials carrying no apigateway permission.
7. Cause 7: create_stage(restApiId=<real>, stageName='s', deploymentId='doesnotexist').
8. Cause 8: get_sdk_types(position='not-a-valid-pagination-token').
9. Cause 9: put_method_response(statusCode='200', responseModels={'application/json': 'NoSuchModelXYZ123'}).
10. Cause 10: create_rest_api(endpointConfiguration={'types': ['PRIVATE']}), add a MOCK method, then create_deployment with no resource policy attached.
