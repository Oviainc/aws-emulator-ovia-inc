Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.concurrent-delete-and-update-stage-serializes apigateway.delete-nonexistent-gateway-response-returns-notfound apigateway.delete-nonexistent-integration-response-returns-notfound apigateway.get-documentation-version-nonexistent-returns-notfound apigateway.get-vpc-link-nonexistent-not-found apigateway.untag-resource-nonexistent-resource-arn-rejected apigateway.reject-domain-name-access-association-invalid-fails apigateway.create-authorizer-invalid-type-rejected
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway.concurrent-delete-and-update-stage-serializes|run#13|AssertionError|assert delete_outcome[0] == "ok", f"DeleteStage should unconditionally succeed, got {delete_outcome}"
Issue-Key: apigateway.delete-nonexistent-gateway-response-returns-notfound|run#6|AssertionError|assert code1 == "NotFoundException", f"expected NotFoundException, got {code1}"
Issue-Key: apigateway.delete-nonexistent-integration-response-returns-notfound|run#4|AssertionError|assert code1 == "NotFoundException", f"expected NotFoundException, got {code1}"
Issue-Key: apigateway.get-documentation-version-nonexistent-returns-notfound|run#6|AssertionError|assert code == "NotFoundException", f"expected NotFoundException, got {code}"
Issue-Key: apigateway.get-vpc-link-nonexistent-not-found|run#3|AssertionError|assert code == "NotFoundException", f"expected NotFoundException, got {code}"
Issue-Key: apigateway.untag-resource-nonexistent-resource-arn-rejected|run#7|AssertionError|assert err.get("Code") == "NotFoundException", f"expected NotFoundException, got {err.get('Code')!r}"
Issue-Key: apigateway.reject-domain-name-access-association-invalid-fails|run#5|AssertionError|assert code == "BadRequestException", f"expected BadRequestException for a malformed ARN pair, got {code!r}"
Issue-Key: apigateway.create-authorizer-invalid-type-rejected|run#7|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got {code}"
Issue-Commit: e2dfe022d58b39493003b3d90859f84840a12f00

# API Gateway: 8 control-plane causes — seven operations answer 501 NotImplemented instead of their modeled result/error, and an out-of-enum authorizer type is refused as ValidationException instead of BadRequestException

> _Note: this issue was identified against the code base at commit `e2dfe022d58b39493003b3d90859f84840a12f00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteStage

## Description
Eight independent root causes in the API Gateway (REST, v1) control plane, all reached by behavioral checks that never got past the operation under test. Causes 1-7 are distinct scaffolded operations, each with its own handler file; cause 8 lives in the request-validation layer, not in a handler. A fixer must close ALL EIGHT before submitting.

Cause 1 of 8: DeleteStage answers 501 NotImplemented.
AWS: Smithy (com.amazonaws.apigateway#DeleteStage): HTTP DELETE /restapis/{restApiId}/stages/{stageName}, success status 202 with an empty body; modeled errors BadRequestException(400), ConflictException(409), LimitExceededException(429), NotFoundException(404), TooManyRequestsException(429), UnauthorizedException(401). Deleting an existing stage succeeds unconditionally; a stage that does not exist is NotFoundException.
Emulator: every DeleteStage call answers HTTP 501 with error code NotImplemented ("operation DeleteStage is scaffolded but not implemented in this emulator"). The concurrency check observed ('err','NotImplemented') where an unconditional success was required.
Why: a client cannot tear down a stage at all, so any stage lifecycle (create -> deploy -> delete) and every cleanup path that deletes a stage leaks state and fails; a caller that treats 501 as retryable will spin.
Fix: the operation's handler, services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/delete_stage.rs (the file still carries the `//! SCAFFOLDED, NOT IMPLEMENTED` marker and returns Scaffolded::not_implemented).
Traces: apigateway.concurrent-delete-and-update-stage-serializes.

Cause 2 of 8: DeleteGatewayResponse answers 501 NotImplemented.
AWS: Smithy (DeleteGatewayResponse): DELETE /restapis/{restApiId}/gatewayresponses/{responseType}, success 202; modeled errors include NotFoundException(404) and BadRequestException(400). Deleting a gateway response that was never overridden, or deleting one twice, answers NotFoundException(404).
Emulator: 501 NotImplemented; the check expecting NotFoundException observed code NotImplemented.
Why: callers cannot reset a customized gateway response, and negative-path code that branches on NotFoundException takes the wrong branch.
Fix: the operation's handler, services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/delete_gateway_response.rs.
Traces: apigateway.delete-nonexistent-gateway-response-returns-notfound.

Cause 3 of 8: DeleteIntegrationResponse answers 501 NotImplemented.
AWS: Smithy (DeleteIntegrationResponse): DELETE /restapis/{restApiId}/resources/{resourceId}/methods/{httpMethod}/integration/responses/{statusCode}, success 204; modeled errors include NotFoundException(404). A statusCode with no integration response answers NotFoundException.
Emulator: 501 NotImplemented; the check expecting NotFoundException observed NotImplemented.
Why: an integration response can be created but never removed, so method reconfiguration is impossible and the 404 contract for a missing response is unobservable.
Fix: the operation's handler, services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/delete_integration_response.rs.
Traces: apigateway.delete-nonexistent-integration-response-returns-notfound.

Cause 4 of 8: GetDocumentationVersion answers 501 NotImplemented.
AWS: Smithy (GetDocumentationVersion): GET /restapis/{restApiId}/documentation/versions/{documentationVersion}, success 200 returning DocumentationVersion{version, createdDate, description}; modeled errors NotFoundException(404), TooManyRequestsException(429), UnauthorizedException(401). An unknown version answers NotFoundException(404).
Emulator: 501 NotImplemented on both the unknown-version read and the positive read of a version that was just created.
Why: documentation-version state is write-only; a client can never read back or verify a published version.
Fix: the operation's handler, services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/get_documentation_version.rs.
Traces: apigateway.get-documentation-version-nonexistent-returns-notfound.

Cause 5 of 8: GetVpcLink answers 501 NotImplemented.
AWS: Smithy (GetVpcLink): GET /vpclinks/{vpcLinkId}, success 200 returning VpcLink{id, name, targetArns, status, statusMessage, tags}; modeled errors BadRequestException(400), NotFoundException(404). An unknown vpcLinkId answers NotFoundException(404).
Emulator: 501 NotImplemented, both for the unknown id and for polling a real link's `status`.
Why: a VPC link's provisioning status (PENDING -> AVAILABLE) can never be observed, so no client can wait for a link before using it in an integration, and the 404 contract is unobservable.
Fix: the operation's handler, services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/get_vpc_link.rs.
Traces: apigateway.get-vpc-link-nonexistent-not-found.

Cause 6 of 8: UntagResource answers 501 NotImplemented.
AWS: Smithy (UntagResource): DELETE /tags/{resourceArn} with tagKeys in the query, success 204; modeled errors BadRequestException(400), NotFoundException(404), ConflictException(409). An ARN naming a resource that does not exist answers NotFoundException(404); removing existing keys succeeds and the keys disappear from GetTags.
Emulator: 501 NotImplemented; the check expecting NotFoundException observed NotImplemented.
Why: tags can be added but never removed, so tag-driven cost/ownership workflows diverge permanently, and the not-found contract for a bogus ARN is unobservable.
Fix: the operation's handler, services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/untag_resource.rs.
Traces: apigateway.untag-resource-nonexistent-resource-arn-rejected.

Cause 7 of 8: RejectDomainNameAccessAssociation answers 501 NotImplemented.
AWS: Smithy (RejectDomainNameAccessAssociation): POST /rejectdomainnameaccessassociations with domainNameAccessAssociationArn and domainNameArn as required query parameters, success 202; modeled errors BadRequestException(400), ConflictException(409), NotFoundException(404). A malformed ARN pair is refused with BadRequestException(400).
Emulator: 501 NotImplemented; the check expecting BadRequestException for a malformed ARN pair observed 'NotImplemented'.
Why: the private-custom-domain access-association approval workflow has no reject path, so a pending association can never be declined and a malformed request is not classified as a client error.
Fix: the operation's handler, services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/reject_domain_name_access_association.rs.
Traces: apigateway.reject-domain-name-access-association-invalid-fails.

Cause 8 of 8: an out-of-enum CreateAuthorizer `type` is refused with ValidationException instead of BadRequestException.
AWS: recorded behavior — CreateAuthorizer with type='NOT_A_REAL_TYPE' is refused by API Gateway with error code BadRequestException (HTTP 400); BadRequestException(400) is the modeled client-error for this operation. The service classifies an unknown authorizer type as a bad request, not as a framework-level constraint violation.
Emulator: the request is rejected before the handler by the modeled-enum validator and answers error code ValidationException (HTTP 400).
Why: the HTTP status agrees but the error CODE does not, so a client that branches on BadRequestException (botocore's exceptions.BadRequestException, or an `except ClientError` with a code test) does not recognize the refusal and treats it as an unknown/unexpected failure. This is independent of the CreateAuthorizer stub: the request never reaches the handler, so implementing the handler does not change it.
Fix: the operation's validation layer — the enum/constraint refusal path that maps modeled-constraint violations to an error code for this service must answer BadRequestException for API Gateway control-plane input violations rather than the generic ValidationException.
Traces: apigateway.create-authorizer-invalid-type-rejected.

Not filed here, and deliberately: checks whose only divergence is refusal TEXT (apigateway.delete-root-resource-rejected, apigateway.delete-method-error-contract-no-bad-request, apigateway.put-integration-invalid-type-fails and the other '1 validation error detected' enum-order differences, apigateway.op-contract-createRestApi, apigateway.vpc-link-integration-missing-connection-id-rejected). Their status and error code match AWS and no rule fixes the recorded wording, so they are examples, not requirements.

## Evidence
From the behavioral replay (suite simparity-behavioral-aws-v1):
- apigateway.concurrent-delete-and-update-stage-serializes -> AssertionError: DeleteStage should unconditionally succeed, got ('err', 'NotImplemented')
- apigateway.delete-nonexistent-gateway-response-returns-notfound -> AssertionError: expected NotFoundException, got NotImplemented
- apigateway.delete-nonexistent-integration-response-returns-notfound -> AssertionError: expected NotFoundException, got NotImplemented
- apigateway.get-documentation-version-nonexistent-returns-notfound -> AssertionError: expected NotFoundException, got NotImplemented
- apigateway.get-vpc-link-nonexistent-not-found -> AssertionError: expected NotFoundException, got NotImplemented
- apigateway.untag-resource-nonexistent-resource-arn-rejected -> AssertionError: expected NotFoundException, got 'NotImplemented'
- apigateway.reject-domain-name-access-association-invalid-fails -> AssertionError: expected BadRequestException for a malformed ARN pair, got 'NotImplemented'
- apigateway.create-authorizer-invalid-type-rejected -> AssertionError: expected BadRequestException, got ValidationException

Expected (AWS): 202/204/200 success bodies per the Smithy http bindings above, NotFoundException(404) for unknown ids, BadRequestException(400) for malformed input.
Actual (emulator): HTTP 501, Error.Code=NotImplemented, message 'operation <Op> is scaffolded but not implemented in this emulator' for causes 1-7; HTTP 400 Error.Code=ValidationException for cause 8.

## Steps to Reproduce
1. Boot the emulator and point botocore at it (region us-west-2).
2. Cause 1: create a REST API, a deployment and a stage, then call DeleteStage -> observe 501 NotImplemented instead of 202.
3. Cause 2: call DeleteGatewayResponse(restApiId, responseType='DEFAULT_4XX') on an API with no override -> observe NotImplemented instead of NotFoundException.
4. Cause 3: call DeleteIntegrationResponse(restApiId, resourceId, httpMethod='GET', statusCode='200') with no integration response present -> observe NotImplemented instead of NotFoundException.
5. Cause 4: call GetDocumentationVersion(restApiId, documentationVersion='v-does-not-exist') -> observe NotImplemented instead of NotFoundException; then create version 'v1' and read it back -> still NotImplemented.
6. Cause 5: call GetVpcLink(vpcLinkId='bogusvpclink123') -> observe NotImplemented instead of NotFoundException.
7. Cause 6: call UntagResource(resourceArn=<bogus but well-formed apigateway ARN>, tagKeys=['Foo']) -> observe NotImplemented instead of NotFoundException.
8. Cause 7: call RejectDomainNameAccessAssociation(domainNameAccessAssociationArn='not-a-valid-arn', domainNameArn=<any>) -> observe NotImplemented instead of BadRequestException.
9. Cause 8: call CreateAuthorizer(restApiId, name=..., type='NOT_A_REAL_TYPE', identitySource=...) -> observe Error.Code=ValidationException instead of BadRequestException.
Re-replay the eight checks listed in trace_ids to confirm.
