Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.create-resource-invalid-pathpart-rejected apigateway.create-rest-api-empty-name-rejected apigateway.custom-authorization-without-authorizer-id-rejected apigateway.op-contract-putMethod apigateway.put-method-custom-auth-without-authorizer apigateway.update-method-invalid-authorizer-rejected apigateway.put-method-invalid-request-parameter-location-rejected apigateway.put-method-invalid-request-parameter-name-rejected apigateway.put-method-malformed-request-parameter-name-rejected apigateway.put-method-response-invalid-model-reference-rejected apigateway.put-integration-aws-service-proxy-credential-validation-rejected apigateway.get-resource-unauthorized apigateway.resource-method-get-lacks-badrequest-error apigateway.create-rest-api-invalid-endpoint-configuration-type-rejected apigateway.create-rest-api-invalid-endpoint-type-rejected apigateway.deployment-description-empty-string-preserved apigateway.rest-api-empty-description-vs-absent-distinguished
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway.create-resource-invalid-pathpart-rejected|_error#6|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: apigateway.create-rest-api-empty-name-rejected|_error#2|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: apigateway.custom-authorization-without-authorizer-id-rejected|_error#5|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: apigateway.op-contract-putMethod|_error#13|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: apigateway.put-method-custom-auth-without-authorizer|_error#5|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: apigateway.update-method-invalid-authorizer-rejected|_client_error#7|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: apigateway.put-method-invalid-request-parameter-location-rejected|_error#9|AssertionError|raise AssertionError("real AWS refuses this call, but it SUCCEEDED")
Issue-Key: apigateway.put-method-invalid-request-parameter-name-rejected|_error#8|AssertionError|raise AssertionError("real AWS refuses this call, but it SUCCEEDED")
Issue-Key: apigateway.put-method-malformed-request-parameter-name-rejected|_error#6|AssertionError|raise AssertionError(f"expected a ClientError but the call succeeded: {result!r}")
Issue-Key: apigateway.put-method-response-invalid-model-reference-rejected|_error#7|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded and returned {result!r}")
Issue-Key: apigateway.put-integration-aws-service-proxy-credential-validation-rejected|_error#8|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: apigateway.get-resource-unauthorized|_error#11|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: apigateway.resource-method-get-lacks-badrequest-error|_error#24|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded with {result!r}")
Issue-Key: apigateway.create-rest-api-invalid-endpoint-configuration-type-rejected|run#6|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: apigateway.create-rest-api-invalid-endpoint-type-rejected|run#6|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: apigateway.deployment-description-empty-string-preserved|run#9|AssertionError|assert created.get("description") is None, created.get("description")
Issue-Key: apigateway.rest-api-empty-description-vs-absent-distinguished|run#13|AssertionError|assert "description" not in emptied, (
Issue-Commit: 001920e7906d433f041aca694d32f13b44f642fa

# API Gateway control plane accepts inputs real AWS refuses, enforces no caller authorization, and mis-renders two representations (10 causes)

> _Note: this issue was identified against the code base at commit `001920e7906d433f041aca694d32f13b44f642fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMethod

## Description
Ten independent root causes in the API Gateway (REST, v1) control plane. Every one is grounded in a failing behavioral check from suite simparity-behavioral-aws-v1; none is marked ALREADY FILED or OWNED, and no open issue names any of them (the open apigateway issues are about the execute-api invoke path, GetAccount/apigatewayv2 stubs and CreateApi 500).

Cause 1 of 10 — CreateResource accepts a pathPart containing '/'.
AWS: Smithy CreateResource (POST /restapis/{restApiId}/resources/{parentId}, success 201) models BadRequestException (HTTP 400); a path part may not contain a path separator, so real AWS refuses pathPart='illegal/slash' with BadRequestException 400.
Emulator: answers 201 and persists the resource, returning id/pathPart='illegal/slash', path='/illegal/slash'.
Why: a client that relies on AWS refusing a malformed path segment silently gets an API whose resource tree contains an unroutable path; the mistake surfaces only at invoke time.
Fix: CreateResource's input validation in the operation handler.
Traces: apigateway.create-resource-invalid-pathpart-rejected.

Cause 2 of 10 — CreateRestApi accepts an empty name.
AWS: Smithy CreateRestApi.name is @required and models BadRequestException (HTTP 400); real AWS refuses name=''.
Emulator: answers 201 and creates a REST API with name='' (apiStatus AVAILABLE, rootResourceId allocated).
Why: an empty-named API is created where AWS would have failed the call, so error-handling paths in deploy tooling never run and the account accumulates unnamed APIs.
Fix: CreateRestApi's input validation in the operation handler.
Traces: apigateway.create-rest-api-empty-name-rejected.

Cause 3 of 10 — PutMethod accepts authorizationType=CUSTOM with no authorizerId.
AWS: Smithy PutMethod models BadRequestException (HTTP 400); authorizationType CUSTOM (and COGNITO_USER_POOLS) requires authorizerId, so real AWS refuses the call.
Emulator: answers 201 returning {httpMethod, authorizationType: 'CUSTOM', apiKeyRequired: false} with no authorizer bound.
Why: a method is created that claims custom authorization but has no authorizer, so requests are either unauthenticated or fail at invoke time instead of at configuration time.
Fix: PutMethod's input validation (cross-member check authorizationType -> authorizerId) in the operation handler.
Traces: apigateway.custom-authorization-without-authorizer-id-rejected, apigateway.op-contract-putMethod, apigateway.put-method-custom-auth-without-authorizer.

Cause 4 of 10 — UpdateMethod accepts an authorizerId that names no authorizer.
AWS: Smithy UpdateMethod (PATCH, success 200) models BadRequestException (HTTP 400) and NotFoundException (HTTP 404); real AWS refuses a patch that points /authorizerId at a nonexistent authorizer.
Emulator: answers 200 with authorizationType 'CUSTOM' and the dangling authorizerId stored.
Why: the method's authorizer reference dangles; authorization silently does not happen.
Fix: UpdateMethod's patch validation (resolve /authorizerId against stored authorizers) in the operation handler.
Traces: apigateway.update-method-invalid-authorizer-rejected.

Cause 5 of 10 — PutMethod accepts requestParameters keys with an invalid location or name.
AWS: Smithy PutMethod.requestParameters is MapOfStringToBoolean and PutMethod models BadRequestException (HTTP 400); request-parameter keys are of the form method.request.{path|querystring|header}.{name}, so real AWS refuses an unknown location token or a malformed name.
Emulator: answers 201 and stores the key verbatim, e.g. requestParameters {'method.request.invalidloc.foo': False}.
Why: a mapping that can never match is accepted, so required-parameter enforcement and parameter mapping silently do nothing.
Fix: PutMethod's requestParameters key validation in the operation handler.
Traces: apigateway.put-method-invalid-request-parameter-location-rejected, apigateway.put-method-invalid-request-parameter-name-rejected, apigateway.put-method-malformed-request-parameter-name-rejected.

Cause 6 of 10 — PutMethodResponse accepts responseModels naming a model that does not exist.
AWS: Smithy PutMethodResponse models BadRequestException (HTTP 400) and NotFoundException (HTTP 404); a responseModels value must resolve to a model of that REST API.
Emulator: answers 201 with responseModels {'application/json': 'NoSuchModelXYZ123'}.
Why: the method response references a nonexistent model, so response validation/SDK generation break later instead of the call failing now.
Fix: PutMethodResponse's responseModels resolution in the operation handler.
Traces: apigateway.put-method-response-invalid-model-reference-rejected.

Cause 7 of 10 — PutIntegration accepts credentials that are not an IAM role ARN.
AWS: Smithy PutIntegration.credentials is a String and PutIntegration models BadRequestException (HTTP 400); credentials must be an IAM role ARN (or the literal arn:aws:iam::*:user/*), so real AWS refuses credentials='not-an-arn-at-all'.
Emulator: answers 201 persisting credentials='not-an-arn-at-all' on an AWS service-proxy integration.
Why: an integration that can never assume a role is accepted; the failure moves from the control plane to every invocation.
Fix: PutIntegration's credentials validation in the operation handler.
Traces: apigateway.put-integration-aws-service-proxy-credential-validation-rejected.

Cause 8 of 10 — control-plane reads apply no caller authorization.
AWS: Smithy GetResource and GetMethodResponse both model UnauthorizedException (HTTP 401) in their error sets; a caller whose credentials do not permit apigateway:GET on the resource is refused.
Emulator: both calls succeed — GetResource answers 200 with the resource body and GetMethodResponse answers 200 {'statusCode': '200'} — under credentials the check established must be refused.
Why: every permission-boundary test passes vacuously, and an emulated deployment cannot reproduce an authorization failure a real account would produce.
Fix: the service's request authorization layer, applied to the read operations' handlers.
Traces: apigateway.get-resource-unauthorized, apigateway.resource-method-get-lacks-badrequest-error.

Cause 9 of 10 — an enum violation inside endpointConfiguration.types is reported at the element path with no list-level constraint wrapper.
AWS: ListOfEndpointType is a list of EndpointType; AWS's ValidationException for a bad member reports the rendered LIST value at the LIST's path and nests the member constraint — recorded form: "1 validation error detected: Value '[NOT_A_REAL_ENDPOINT_TYPE]' at 'createRestApiInput.endpointConfiguration.types' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy enum value set: [...]]".
Emulator: reports the single element at an indexed member path with no nesting: "Value 'NOT_A_REAL_ENDPOINT_TYPE' at 'createRestApiInput.endpointConfiguration.types.1.member' failed to satisfy constraint: Member must satisfy enum value set: [...]". Status and error code match; the member path and constraint nesting do not. (The ORDER of the enum values inside the brackets also differs — the emulator uses Smithy declaration order — but that ordering follows no rule the model or the recording establishes, so it is not part of this requirement.)
Why: clients and test suites that parse the member path to attribute a validation failure attribute it to a list element that AWS never names.
Fix: the constraint-violation renderer for list members in the shared request-validation layer.
Traces: apigateway.create-rest-api-invalid-endpoint-configuration-type-rejected, apigateway.create-rest-api-invalid-endpoint-type-rejected.

Cause 10 of 10 — an explicitly empty description is preserved as '' instead of normalized to absent.
AWS: recorded behavior only — API Gateway normalizes an explicitly empty description to an absent field, both on the create response and on the subsequent read (CreateDeployment.description and the RestApi description patched to '').
Emulator: returns the literal empty string, so description is present-and-empty where AWS omits the member.
Why: a client that distinguishes absent from empty (or round-trips the value back into a patch) sees a field AWS never returns.
Fix: description normalization in CreateDeployment/GetDeployment and UpdateRestApi/GetRestApi state writes.
Traces: apigateway.deployment-description-empty-string-preserved, apigateway.rest-api-empty-description-vs-absent-distinguished.

## Evidence
Cause 1: expected BadRequestException; got HTTP 201 {'id': '5mw7f4baaa', 'pathPart': 'illegal/slash', 'path': '/illegal/slash'}.
Cause 2: expected BadRequestException; got HTTP 201 {'name': '', 'apiStatus': 'AVAILABLE', ...}.
Cause 3: expected BadRequestException; got HTTP 201 {'httpMethod': 'POST', 'authorizationType': 'CUSTOM', 'apiKeyRequired': False}.
Cause 4: expected a rejection; got HTTP 200 {'httpMethod': 'GET', 'authorizationType': 'CUSTOM', 'apiKeyRequired': False}.
Cause 5: expected BadRequestException; got HTTP 201 {'requestParameters': {'method.request.invalidloc.foo': False}}.
Cause 6: expected BadRequestException; got HTTP 201 {'statusCode': '200', 'responseModels': {'application/json': 'NoSuchModelXYZ123'}}.
Cause 7: expected BadRequestException; got HTTP 201 {'type': 'AWS', 'uri': 'arn:aws:apigateway:us-west-2:sns:action/ListTopics', 'credentials': 'not-an-arn-at-all', ...}.
Cause 8: expected UnauthorizedException (401); GetResource got 200 {'id': '6ofuwnbaaa', 'pathPart': 'widgets'} and GetMethodResponse got 200 {'statusCode': '200'}.
Cause 9: expected message at 'createRestApiInput.endpointConfiguration.types' with nested 'Member must satisfy constraint: [...]'; got 'createRestApiInput.endpointConfiguration.types.1.member' with a flat constraint.
Cause 10: expected description absent (None); got ''.

## Steps to Reproduce
Replay the behavioral suite and re-run the listed checks, e.g.:
1. apigateway.create-resource-invalid-pathpart-rejected — CreateRestApi, then CreateResource(parentId=root, pathPart='illegal/slash'); expect BadRequestException 400.
2. apigateway.put-method-custom-auth-without-authorizer — PutMethod(authorizationType='CUSTOM') with no authorizerId; expect BadRequestException 400.
3. apigateway.get-resource-unauthorized — GetResource with credentials that do not allow apigateway:GET; expect UnauthorizedException 401.
4. apigateway.create-rest-api-invalid-endpoint-type-rejected — CreateRestApi(endpointConfiguration={'types': ['NOT_A_REAL_ENDPOINT_TYPE']}); compare the ValidationException member path.
5. apigateway.deployment-description-empty-string-preserved — CreateDeployment(description=''); expect description absent on both the create response and GetDeployment.
