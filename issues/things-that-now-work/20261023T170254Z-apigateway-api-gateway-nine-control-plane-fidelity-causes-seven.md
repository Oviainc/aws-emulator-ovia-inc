Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.create-model-invalid-content-type-rejected apigateway.create-model-invalid-json-schema-rejected apigateway.model-invalid-schema-json-rejected apigateway.model-schema-invalid-json-rejected apigateway.create-usage-plan-invalid-throttle-rate-rejected apigateway.create-usage-plan-negative-quota-limit-rejected apigateway.create-usage-plan-negative-throttle-rate-rejected apigateway.create-vpc-link-empty-target-arns-rejected apigateway.create-vpc-link-empty-target-list-rejected apigateway.create-vpc-link-invalid-target-badrequest apigateway.create-vpc-link-missing-target-arns-rejected apigateway.vpc-link-invalid-target-arn-rejected apigateway.authorizer-deletion-blocked-while-referenced-rejected apigateway.delete-authorizer-attached-to-method-conflict apigateway.delete-authorizer-in-use-rejected apigateway.op-contract-delete-authorizer apigateway.authorizer-ttl-out-of-range-rejected apigateway.update-authorizer-invalid-ttl-error-contract apigateway.update-stage-invalid-loglevel-rejected apigateway.concurrent-create-documentation-version-race apigateway.import-documentation-parts-bulk-atomic apigateway.put-gateway-response-invalid-response-parameters-rejected
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway.create-model-invalid-content-type-rejected|run#6|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got {code}"
Issue-Key: apigateway.create-model-invalid-json-schema-rejected|run#7|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got {code}"
Issue-Key: apigateway.model-invalid-schema-json-rejected|run#7|AssertionError|assert code == "BadRequestException", code
Issue-Key: apigateway.model-schema-invalid-json-rejected|run#6|AssertionError|assert err.get("Code") == "BadRequestException", f"expected BadRequestException, got {err.get('Code')!r}"
Issue-Key: apigateway.create-usage-plan-invalid-throttle-rate-rejected|run#4|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got {code}"
Issue-Key: apigateway.create-usage-plan-negative-quota-limit-rejected|run#4|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got {code!r}"
Issue-Key: apigateway.create-usage-plan-negative-throttle-rate-rejected|run#4|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got {code}"
Issue-Key: apigateway.create-vpc-link-empty-target-arns-rejected|run#4|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got {code}"
Issue-Key: apigateway.create-vpc-link-empty-target-list-rejected|run#6|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got {code}"
Issue-Key: apigateway.create-vpc-link-invalid-target-badrequest|run#5|AssertionError|assert code == "BadRequestException", f"expected BadRequestException for invalid target ARN, got {code!r}"
Issue-Key: apigateway.create-vpc-link-missing-target-arns-rejected|run#4|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got {code}"
Issue-Key: apigateway.vpc-link-invalid-target-arn-rejected|run#5|AssertionError|assert code == "BadRequestException", f"expected BadRequestException for malformed targetArns, got {code}"
Issue-Key: apigateway.authorizer-deletion-blocked-while-referenced-rejected|run#14|AssertionError|assert code == "ConflictException", f"expected ConflictException while referenced, got {code!r}"
Issue-Key: apigateway.delete-authorizer-attached-to-method-conflict|run#10|AssertionError|assert code == "ConflictException", (
Issue-Key: apigateway.delete-authorizer-in-use-rejected|run#10|AssertionError|assert code == "ConflictException", f"expected ConflictException while authorizer is referenced, got {code!r}"
Issue-Key: apigateway.op-contract-delete-authorizer|run#11|AssertionError|assert err.get("Code") == "ConflictException", f"expected ConflictException, got {err.get('Code')!r}"
Issue-Key: apigateway.authorizer-ttl-out-of-range-rejected|run#9|AssertionError|assert code == "BadRequestException", f"expected BadRequestException on update, got {code}"
Issue-Key: apigateway.update-authorizer-invalid-ttl-error-contract|run#11|AssertionError|assert code == "BadRequestException", f"expected BadRequestException for TTL over the max, got {code}"
Issue-Key: apigateway.update-stage-invalid-loglevel-rejected|run#4|AssertionError|assert err.get("Code") == "BadRequestException", f"expected BadRequestException, got {err.get('Code')!r}"
Issue-Key: apigateway.concurrent-create-documentation-version-race|run#11|AssertionError|assert len(successes) == 1, f"nok={len(successes)} c={[o[1] for o in failures]}"
Issue-Key: apigateway.import-documentation-parts-bulk-atomic|_error#19|AssertionError|raise AssertionError("expected a ClientError, but the call SUCCEEDED")
Issue-Key: apigateway.put-gateway-response-invalid-response-parameters-rejected|_error#5|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Commit: f9ff4020ff731d5f2129f1ceb9f16685ad9310db

# API Gateway: nine control-plane fidelity causes — seven operations answer 501 NotImplemented instead of their modeled 4xx, and ImportDocumentationParts/PutGatewayResponse accept input real AWS refuses

> _Note: this issue was identified against the code base at commit `f9ff4020ff731d5f2129f1ceb9f16685ad9310db`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateModel

## Description
Nine distinct root causes in the apigateway control plane, all grounded in failing behavioral checks that the replay report did NOT mark as already filed or owned. Causes 1-7 are operations whose modeled 4xx error contract is unreachable because the handler is a scaffold stub that answers 501 NotImplemented; causes 8-9 are operations that are implemented but skip validation and accept input AWS refuses. Related open issues exist for some of these operations as bare 'unimplemented scaffold stub' work items (20261023T125454Z-apigateway-createmodel-stub.md, -createusageplan-stub.md, -createvpclink-stub.md, -updatestage-stub.md, -importdocumentationparts-stub.md, and 20261023T151100Z-apigateway-api-gateway-8-control-plane-causes-seven-operations.md); none of them carries the replay keys below, which is why they are filed here with the specific error contract each check asserts.

Cause 1 of 9: CreateModel — invalid contentType and malformed JSON schema are not refused.
AWS: Smithy CreateModel (POST /restapis/{restApiId}/models, success 201) models errors BadRequestException (400), ConflictException (409), NotFoundException (404), LimitExceededException (429), TooManyRequestsException (429), UnauthorizedException (401); `contentType` and `name` are @required. Real AWS answers 400 BadRequestException for a contentType that is not a media type and for a `schema` that is not a parseable JSON Schema document.
Emulator: the call answers 501 with error code NotImplemented ('operation CreateModel is scaffolded but not implemented in this emulator').
Why: a client cannot create request/response models at all, and a client that relies on the 400 to surface a bad schema to its user instead sees an unmodeled NotImplemented that no SDK retry/handling path expects.
Fix: the CreateModel handler and its input validation (services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/create_model.rs).
Traces: apigateway.create-model-invalid-content-type-rejected, apigateway.create-model-invalid-json-schema-rejected, apigateway.model-invalid-schema-json-rejected, apigateway.model-schema-invalid-json-rejected.

Cause 2 of 9: CreateUsagePlan — out-of-range throttle and quota values are not refused.
AWS: Smithy CreateUsagePlan (POST /usageplans, success 201) models BadRequestException (400) among its errors. Real AWS answers 400 BadRequestException for a negative/invalid throttle rateLimit and for a negative quota limit.
Emulator: 501 NotImplemented.
Why: usage plans cannot be created, and the negative-input contract (400) is unreachable.
Fix: the CreateUsagePlan handler and its validation (src/ops/create_usage_plan.rs).
Traces: apigateway.create-usage-plan-invalid-throttle-rate-rejected, apigateway.create-usage-plan-negative-quota-limit-rejected, apigateway.create-usage-plan-negative-throttle-rate-rejected.

Cause 3 of 9: CreateVpcLink — empty, missing and malformed targetArns are not refused.
AWS: Smithy CreateVpcLink (POST /vpclinks, success 202) has `name` and `targetArns` @required and models BadRequestException (400). Real AWS answers 400 BadRequestException when targetArns is absent, an empty list, or holds a value that is not an NLB ARN.
Emulator: 501 NotImplemented.
Why: private-integration setup is impossible and the 400 contract for a malformed target is unreachable.
Fix: the CreateVpcLink handler and its validation (src/ops/create_vpc_link.rs).
Traces: apigateway.create-vpc-link-empty-target-arns-rejected, apigateway.create-vpc-link-empty-target-list-rejected, apigateway.create-vpc-link-invalid-target-badrequest, apigateway.create-vpc-link-missing-target-arns-rejected, apigateway.vpc-link-invalid-target-arn-rejected.

Cause 4 of 9: DeleteAuthorizer — deleting an authorizer still referenced by a method is not refused with ConflictException.
AWS: Smithy DeleteAuthorizer (DELETE /restapis/{restApiId}/authorizers/{authorizerId}, success 202) models ConflictException (409), BadRequestException (400), NotFoundException (404), TooManyRequestsException (429), UnauthorizedException (401). Real AWS answers 409 ConflictException while a method's authorizerId still points at it.
Emulator: 501 NotImplemented.
Why: a client that deletes an authorizer and expects a 409 to tell it the authorizer is still wired to a method gets an unmodeled 501 instead; referential integrity of the API is unverifiable.
Fix: the DeleteAuthorizer handler plus the reference check against method state (src/ops/delete_authorizer.rs).
Traces: apigateway.authorizer-deletion-blocked-while-referenced-rejected, apigateway.delete-authorizer-attached-to-method-conflict, apigateway.delete-authorizer-in-use-rejected, apigateway.op-contract-delete-authorizer.

Cause 5 of 9: UpdateAuthorizer — an out-of-range authorizerResultTtlInSeconds patch is not refused.
AWS: Smithy UpdateAuthorizer (PATCH /restapis/{restApiId}/authorizers/{authorizerId}, success 200) models BadRequestException (400). Real AWS answers 400 BadRequestException when a patchOperation sets authorizerResultTtlInSeconds outside the accepted range.
Emulator: 501 NotImplemented.
Why: authorizers cannot be reconfigured, and the 400 that bounds the TTL is unreachable.
Fix: the UpdateAuthorizer handler and its patch validation (src/ops/update_authorizer.rs).
Traces: apigateway.authorizer-ttl-out-of-range-rejected, apigateway.update-authorizer-invalid-ttl-error-contract.

Cause 6 of 9: UpdateStage — an invalid loggingLevel patch is not refused.
AWS: Smithy UpdateStage (PATCH /restapis/{restApiId}/stages/{stageName}, success 200) models BadRequestException (400). Real AWS answers 400 BadRequestException for a /*/*/logging/loglevel value outside the accepted set.
Emulator: 501 NotImplemented.
Why: stage settings cannot be patched and the 400 that guards the log-level enum is unreachable.
Fix: the UpdateStage handler and its patch validation (src/ops/update_stage.rs).
Traces: apigateway.update-stage-invalid-loglevel-rejected.

Cause 7 of 9: CreateDocumentationVersion — concurrent duplicate creates produce no ConflictException.
AWS: Smithy CreateDocumentationVersion (POST /restapis/{restApiId}/documentation/versions, success 201) models ConflictException (409). Real AWS serializes two concurrent identical creates into exactly one 201 and one 409 ConflictException.
Emulator: both concurrent calls answer 501 NotImplemented (the check reports nok=0 with both outcomes 'NotImplemented').
Why: documentation versioning is unusable and the uniqueness contract is unobservable.
Fix: the CreateDocumentationVersion handler and the uniqueness check in documentation state (src/ops/create_documentation_version.rs).
Traces: apigateway.concurrent-create-documentation-version-race.

Cause 8 of 9: ImportDocumentationParts accepts a bulk payload real AWS refuses, instead of failing the whole import.
AWS: Smithy ImportDocumentationParts (PUT /restapis/{restApiId}/documentation/parts, success 200) takes a @required `body` blob and models BadRequestException (400). Real AWS refuses an import whose parts are not all valid and applies nothing — the import is atomic.
Emulator: the call SUCCEEDED where a ClientError was expected (the check reports 'expected a ClientError, but the call SUCCEEDED').
Why: a client gets a 200 and believes the whole documentation set was imported, while AWS would have rejected the request; partially-applied or invalid parts then diverge silently from the real service.
Fix: the ImportDocumentationParts handler — validate every part before applying any, and reject with BadRequestException otherwise (src/ops/import_documentation_parts.rs).
Traces: apigateway.import-documentation-parts-bulk-atomic.

Cause 9 of 9: PutGatewayResponse accepts responseParameters with a key that is not a valid parameter mapping.
AWS: Smithy PutGatewayResponse (PUT /restapis/{restApiId}/gatewayresponses/{responseType}, success 201) models BadRequestException (400); `responseParameters` is a MapOfStringToString whose keys must be gateway-response parameter mappings. Real AWS answers 400 BadRequestException for a key that is not one.
Emulator: the call returns HTTP 201 and echoes the bad mapping back — recorded response body carries responseParameters {'invalid.param.name': "'foo'"} with responseType DEFAULT_4XX, defaultResponse false.
Why: a client's malformed gateway-response mapping is accepted and stored, so a configuration error that real AWS rejects at write time is only discovered (or silently ignored) at request time.
Fix: PutGatewayResponse input validation for responseParameters keys (src/ops/put_gateway_response.rs).
Traces: apigateway.put-gateway-response-invalid-response-parameters-rejected.

## Evidence
Cause 1: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateModel operation: operation CreateModel is scaffolded but not implemented in this emulator. Checks assert: 'expected BadRequestException, got NotImplemented'.
Cause 2: 'expected BadRequestException, got NotImplemented' / 'expected BadRequestException, got 'NotImplemented''.
Cause 3: 'expected BadRequestException, got NotImplemented', 'expected BadRequestException for invalid target ARN, got 'NotImplemented'', 'expected BadRequestException for malformed targetArns, got NotImplemented'.
Cause 4: 'expected ConflictException while referenced, got 'NotImplemented''; 'expected ConflictException deleting an authorizer attached to a method, got NotImplemented'; 'expected ConflictException while authorizer is referenced, got 'NotImplemented''; 'expected ConflictException, got 'NotImplemented''.
Cause 5: 'expected BadRequestException on update, got NotImplemented'; 'expected BadRequestException for TTL over the max, got NotImplemented'.
Cause 6: 'expected BadRequestException, got 'NotImplemented''.
Cause 7: AssertionError: nok=0 c=['NotImplemented', 'NotImplemented'] — expected exactly one 201 and one ConflictException.
Cause 8: AssertionError: expected a ClientError, but the call SUCCEEDED (check_apigateway_import_documentation_parts_bulk_atomic.py line 124).
Cause 9: AssertionError: expected a ClientError, but the call SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 201, ...}, 'responseType': 'DEFAULT_4XX', 'responseParameters': {'invalid.param.name': "'foo'"}, 'defaultResponse': False} (check_apigateway_put_gateway_response_invalid_response_parameters_rejected.py line 21). Expected HTTP 400 / BadRequestException.

## Steps to Reproduce
Replay the behavioral suite and re-run exactly the checks in trace_ids. Minimal manual repro per cause:
1. apigateway create-model --rest-api-id <id> --name M --content-type 'not/a/media/type' --schema '{' -> expect 400 BadRequestException, observe 501 NotImplemented.
2. apigateway create-usage-plan --name p --throttle 'rateLimit=-1,burstLimit=1' -> expect 400 BadRequestException, observe 501 NotImplemented.
3. apigateway create-vpc-link --name v --target-arns -> expect 400 BadRequestException, observe 501 NotImplemented.
4. Create an authorizer, point a method's authorizerId at it, then apigateway delete-authorizer -> expect 409 ConflictException, observe 501 NotImplemented.
5. apigateway update-authorizer --patch-operations op=replace,path=/authorizerResultTtlInSeconds,value=3601 -> expect 400 BadRequestException, observe 501 NotImplemented.
6. apigateway update-stage --patch-operations op=replace,path=/*/*/logging/loglevel,value=NOT_A_LEVEL -> expect 400 BadRequestException, observe 501 NotImplemented.
7. Issue two concurrent identical create-documentation-version calls -> expect one 201 and one 409, observe two 501s.
8. apigateway import-documentation-parts with a body containing an invalid part -> expect 400 BadRequestException and nothing applied, observe 200 success.
9. apigateway put-gateway-response --response-type DEFAULT_4XX --response-parameters 'invalid.param.name=\'foo\'' -> expect 400 BadRequestException, observe 201 with the mapping echoed back.
