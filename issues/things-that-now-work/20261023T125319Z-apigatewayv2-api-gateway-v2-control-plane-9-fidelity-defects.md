Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__events__sqs.xsvc-apigatewayv2-1-putevents-subtype-routes-to-custom-bus-rule-queue apigatewayv2__events__sqs.xsvc-apigatewayv2-1-putevents-without-events-permission-denied-delivers-nothing apigatewayv2__stepfunctions.xsvc-apigatewayv2-1-startexecution-subtype-execution-input-is-the-posted-body apigatewayv2__stepfunctions.xsvc-apigatewayv2-1-startexecution-without-states-permission-denied-starts-nothing apigatewayv2__stepfunctions.xsvc-apigatewayv2-1-startsyncexecution-subtype-returns-the-state-machine-output apigatewayv2__sqs.aws-proxy-subtype-sqs-sendmessage-delivers-to-the-queue apigatewayv2.cors-wildcard-origin-with-credentials-rejected apigatewayv2.http-api-create-get-delete apigatewayv2.bad-protocol-type-rejected apigatewayv2.http-api-integration-timeout-above-30000-rejected apigatewayv2.create-route-targeting-a-missing-integration-rejected apigatewayv2.websocket-connect-custom-authorizer-nonexistent-id-rejected apigatewayv2.http-api-route-key-precedence-and-path-parameters apigatewayv2.http-proxy-payload-format-version-2-rejected apigatewayv2__lambda.xsvc-apigatewayv2-3-websocket-aws-integration-request-template-reaches-lambda apigatewayv2.websocket-post-to-connection-delivers-then-gone-after-disconnect apigatewayv2.duplicate-route-key-conflict-rejected
Issue-Key: apigatewayv2__events__sqs|CreateIntegration|BadRequestException
Issue-Key: apigatewayv2__stepfunctions|CreateIntegration|BadRequestException
Issue-Key: apigatewayv2__sqs|CreateIntegration|BadRequestException
Issue-Key: apigatewayv2.cors-wildcard-origin-with-credentials-rejected|run#10|AssertionError|assert code == "BadRequestException", (
Issue-Key: apigatewayv2.http-api-create-get-delete|run#20|AssertionError|assert code == "NotFoundException", f"expected NotFoundException updating the deleted API, got {code}"
Issue-Key: apigatewayv2.bad-protocol-type-rejected|run#8|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got {code}"
Issue-Key: apigatewayv2.http-api-integration-timeout-above-30000-rejected|run#11|AssertionError|assert code == "BadRequestException", (
Issue-Key: apigatewayv2.create-route-targeting-a-missing-integration-rejected|_error#7|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: apigatewayv2.websocket-connect-custom-authorizer-nonexistent-id-rejected|_error#7|AssertionError|raise AssertionError(f"expected the call to fail, it succeeded: {result!r}")
Issue-Key: apigatewayv2.http-api-route-key-precedence-and-path-parameters|run#10|AssertionError|raise AssertionError(
Issue-Key: apigatewayv2.http-proxy-payload-format-version-2-rejected|_err#8|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded with {result!r}")
Issue-Key: apigatewayv2__lambda.xsvc-apigatewayv2-3-websocket-aws-integration-request-template-reaches-lambda|_rejection#9|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: apigatewayv2.websocket-post-to-connection-delivers-then-gone-after-disconnect|run#6|AssertionError|assert endpoint.startswith(("wss://", "ws://")), f"a WEBSOCKET ApiEndpoint must be ws(s)://, got {endpoint!r}"
Issue-Key: apigatewayv2.duplicate-route-key-conflict-rejected|run#12|AssertionError|assert message == expected_message, f"expected the message {expected_message!r}, got {message!r}"
Issue-Commit: 001920e7906d433f041aca694d32f13b44f642fa

# API Gateway v2 control plane: 9 fidelity defects — AWS_PROXY service-subtype integrations refused, UpdateApi is a 501 stub, off-model ValidationException, and missing CreateRoute/CreateIntegration validation

> _Note: this issue was identified against the code base at commit `001920e7906d433f041aca694d32f13b44f642fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateIntegration

## Description
Nine distinct root causes in the apigatewayv2 control plane. Each must be closed before submitting.

Cause 1 of 9: CreateIntegration refuses AWS_PROXY integrations that use IntegrationSubtype.
AWS: Smithy CreateIntegration input has `IntegrationSubtype` (StringWithLengthBetween1And128, jsonName integrationSubtype) documented as "Supported only for HTTP API AWS_PROXY integrations. Specifies the AWS service action to invoke."; `IntegrationUri` is NOT a required member (only ApiId and IntegrationType are). A first-class service integration (IntegrationType=AWS_PROXY + IntegrationSubtype=SQS-SendMessage / EventBridge-PutEvents / StepFunctions-StartExecution|StartSyncExecution + CredentialsArn + RequestParameters) is created with NO IntegrationUri and returns 201.
Emulator: the create is rejected with HTTP 400 `BadRequestException`, message example `IntegrationUri is required for an integration of type AWS_PROXY.`
Why: the whole AWS-service-proxy integration family (SQS, EventBridge, Step Functions, …) is unusable; a client that follows the documented subtype form gets a 400 for a valid request.
Fix: CreateIntegration's input validation in the apigatewayv2 handler (services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/ops/create_integration.rs) — require IntegrationUri only when IntegrationSubtype is absent, and accept/persist IntegrationSubtype + CredentialsArn.
Traces: apigatewayv2__events__sqs.xsvc-apigatewayv2-1-putevents-subtype-routes-to-custom-bus-rule-queue, apigatewayv2__events__sqs.xsvc-apigatewayv2-1-putevents-without-events-permission-denied-delivers-nothing, apigatewayv2__sfn.xsvc-apigatewayv2-1-startexecution-subtype-execution-input-is-the-posted-body, apigatewayv2__sfn.xsvc-apigatewayv2-1-startexecution-without-states-permission-denied-starts-nothing, apigatewayv2__sfn.xsvc-apigatewayv2-1-startsyncexecution-subtype-returns-the-state-machine-output, apigatewayv2__sqs.aws-proxy-subtype-sqs-sendmessage-delivers-to-the-queue

Cause 2 of 9: UpdateApi is an unimplemented scaffold stub.
AWS: Smithy UpdateApi (PATCH /v2/apis/{ApiId}, errors BadRequestException 400 / ConflictException 409 / NotFoundException 404 / TooManyRequestsException 429) mutates an API in place; on a deleted ApiId it answers NotFoundException 404, and an illegal CORS document (AllowOrigins=["*"] with AllowCredentials=true) is refused with BadRequestException 400.
Emulator: every UpdateApi call returns `NotImplemented`, so neither the 404-after-delete leg nor the CORS rejection can happen.
Why: no client can modify an existing API (description, name, CORS, DisableExecuteApiEndpoint) and the emulator silently stores CORS documents AWS refuses.
Fix: implement the UpdateApi handler and its validation (ops/update_api.rs), including the wildcard-origin-with-credentials rule and NotFoundException for unknown/deleted ApiIds.
Traces: apigatewayv2.cors-wildcard-origin-with-credentials-rejected, apigatewayv2.http-api-create-get-delete

Cause 3 of 9: input-validation rejections carry an off-model `ValidationException` instead of `BadRequestException`.
AWS: no apigatewayv2 operation models a ValidationException. CreateApi and CreateIntegration both declare exactly BadRequestException(400)/ConflictException(409)/NotFoundException(404)/TooManyRequestsException(429). An invalid ProtocolType ("FTP", outside the ProtocolType enum) is BadRequestException 400, message example `Invalid protocol specified. Must be one of [HTTP, WEBSOCKET]`; TimeoutInMillis above the modeled range (IntegerWithLengthBetween50And30000, range 50–30000) is likewise BadRequestException 400.
Emulator: both are rejected with code `ValidationException` (generic constraint text, e.g. `1 validation error detected: Value '30001' at 'timeoutInMillis' failed to satisfy constraint: Member must have value less than or equal to 30000`).
Why: botocore maps modeled error shapes to typed exceptions — `client.exceptions.BadRequestException` never matches, so retry/except logic written against AWS breaks even though the request was correctly refused.
Fix: the service's error mapping / constraint-validation layer for apigatewayv2 must surface BadRequestException (the rule: member name + violated constraint drive the message) rather than the generic ValidationException.
Traces: apigatewayv2.bad-protocol-type-rejected, apigatewayv2.http-api-integration-timeout-above-30000-rejected

Cause 4 of 9: CreateRoute accepts dangling references instead of rejecting them.
AWS: Smithy CreateRoute declares NotFoundException(404) and BadRequestException(400). A Target of `integrations/<id>` for an integration that does not exist on the API is refused, and AuthorizationType=CUSTOM with an AuthorizerId that does not exist on the API is refused; nothing is created.
Emulator: both calls return HTTP 201 and mint a route — observed `Target: 'integrations/abc1234'` and `AuthorizerId: 'nonexistent123'` persisted.
Why: over-permissiveness — a template or SDK caller can build an API whose routes point at nothing, which real AWS rejects at create time; the failure then only appears at invoke time (or never, in the emulator).
Fix: referential validation in the CreateRoute handler (ops/create_route.rs): resolve Target's integration id and AuthorizerId against the API's own state before persisting.
Traces: apigatewayv2.create-route-targeting-a-missing-integration-rejected, apigatewayv2.websocket-connect-custom-authorizer-nonexistent-id-rejected

Cause 5 of 9: CreateRoute accepts conflicting path variables at the same path segment.
AWS: recorded behavior only (the model types RouteKey as SelectionKey with no pattern). AWS refuses a route key that introduces a SECOND distinct variable at a level that already has one — with `GET /items/{id}` present, `GET /items/{proxy+}` is rejected with BadRequestException 400.
Emulator: the second route key is accepted, so an API ends up with two conflicting variable routes at /items/ and route-key precedence becomes undefined.
Why: clients get an API whose request routing cannot be reproduced on AWS, and the AWS-side create would have failed.
Fix: route-key conflict validation in the CreateRoute handler (compare the new key's segments against existing route keys on the API).
Traces: apigatewayv2.http-api-route-key-precedence-and-path-parameters

Cause 6 of 9: CreateIntegration accepts PayloadFormatVersion '2.0' on an HTTP_PROXY integration.
AWS: recorded behavior only — PayloadFormatVersion 2.0 is a Lambda (AWS_PROXY) payload shape; for IntegrationType=HTTP_PROXY AWS refuses it with BadRequestException 400 and creates nothing.
Emulator: returns 201 with `IntegrationType: 'HTTP_PROXY', PayloadFormatVersion: '2.0'`.
Why: an invalid integration is stored and reported back as valid; the same template fails on AWS.
Fix: CreateIntegration validation (ops/create_integration.rs) — constrain PayloadFormatVersion by IntegrationType.
Traces: apigatewayv2.http-proxy-payload-format-version-2-rejected

Cause 7 of 9: CreateIntegration accepts an unescaped `TemplateSelectionExpression` of `$default`.
AWS: recorded behavior only (the model types TemplateSelectionExpression as a plain SelectionExpression string). AWS refuses the unescaped variable with BadRequestException 400, message example `Unexpected variable in selection expression: $default`, and creates nothing.
Emulator: returns 201 with `TemplateSelectionExpression: '$default'`.
Why: a WebSocket AWS integration is created in a state AWS never allows, hiding a configuration error until the same stack is deployed for real.
Fix: selection-expression validation in the CreateIntegration handler.
Traces: apigatewayv2__lambda.xsvc-apigatewayv2-3-websocket-aws-integration-request-template-reaches-lambda

Cause 8 of 9: a WEBSOCKET API's ApiEndpoint is returned with an https:// scheme.
AWS: recorded behavior only (Smithy types CreateApi/GetApi `ApiEndpoint` as __string). For ProtocolType=WEBSOCKET the endpoint AWS returns is a WebSocket URL (`wss://<api-id>.execute-api.<region>.amazonaws.com`); HTTP APIs get `https://`.
Emulator: a WEBSOCKET API reports `https://<api-id>.execute-api.us-west-2.amazonaws.com` (host/region/id vary per environment; the scheme is the defect).
Why: clients derive the @connections callback URL and the client socket URL from ApiEndpoint; an https:// scheme makes every WebSocket client fail to connect.
Fix: ApiEndpoint construction in the CreateApi/GetApi serializer or API state (ops/create_api.rs) — choose the scheme from ProtocolType.
Traces: apigatewayv2.websocket-post-to-connection-delivers-then-gone-after-disconnect

Cause 9 of 9: the duplicate-route-key ConflictException message carries a trailing period.
AWS: the message is built from the request, so its rule is known: `Route with key <RouteKey> already exists for this API` (no trailing period), with code ConflictException 409.
Emulator: same code, message `Route with key GET /dup already exists for this API.` — one extra `.`.
Why: callers that match the documented, request-derived message text (the only way to distinguish a route-key conflict from other 409s) mis-classify the error.
Fix: the conflict message formatting in the CreateRoute handler.
Traces: apigatewayv2.duplicate-route-key-conflict-rejected

Note: open issues 20261023T113357Z-apigatewayv2-…-createstage-and-createdeployment-are-unimplemented.md, 20261023T114649Z-apigatewayv2-createintegrationresponse-stub.md and 20261023T114649Z-apigatewayv2-createrouteresponse-stub.md cover different operations and none of the nine causes above.

## Evidence
Cause 1 — expected 201 for CreateIntegration(IntegrationType=AWS_PROXY, IntegrationSubtype='SQS-SendMessage', CredentialsArn=…, RequestParameters=…); actual `botocore.errorfactory.BadRequestException: An error occurred (BadRequestException) when calling the CreateIntegration operation: IntegrationUri is required for an integration of type AWS_PROXY.` (6 checks).
Cause 2 — expected BadRequestException for UpdateApi with AllowOrigins=['*'] + AllowCredentials=true, and NotFoundException for UpdateApi on a deleted ApiId; actual `NotImplemented` for both.
Cause 3 — expected BadRequestException, got ValidationException: `AssertionError: expected BadRequestException, got ValidationException` (CreateApi ProtocolType='FTP') and `CreateIntegration TimeoutInMillis=30001: expected BadRequestException, got ValidationException`.
Cause 4 — expected rejection, got HTTP 201: route created with `'Target': 'integrations/abc1234'`; and route created with `'AuthorizationType': 'CUSTOM', 'AuthorizerId': 'nonexistent123'`.
Cause 5 — `AssertionError: route key 'GET /items/{proxy+}' must be refused: it introduces a second variable at the /items/ level, which already has the variable of 'GET /items/{id}'`.
Cause 6 — expected rejection, got HTTP 201 with `'IntegrationType': 'HTTP_PROXY', 'PayloadFormatVersion': '2.0'`.
Cause 7 — expected BadRequestException `Unexpected variable in selection expression: $default`; got HTTP 201 with `'TemplateSelectionExpression': '$default'`.
Cause 8 — `AssertionError: a WEBSOCKET ApiEndpoint must be ws(s)://, got 'https://<api-id>.execute-api.us-west-2.amazonaws.com'`.
Cause 9 — `AssertionError: expected the message 'Route with key GET /dup already exists for this API', got 'Route with key GET /dup already exists for this API.'`

## Steps to Reproduce
Re-replay the check ids in trace_ids. Per cause:
1. CreateIntegration(ApiId, IntegrationType='AWS_PROXY', IntegrationSubtype='SQS-SendMessage', CredentialsArn=<role>, RequestParameters={'QueueUrl':…,'MessageBody':…}) with no IntegrationUri -> expect 201.
2. CreateApi(HTTP, CorsConfiguration AllowOrigins=['https://ok.example.com'], AllowCredentials=true) then UpdateApi widening AllowOrigins to ['*'] -> expect BadRequestException; DeleteApi then UpdateApi -> expect NotFoundException.
3. CreateApi(ProtocolType='FTP') and CreateIntegration(TimeoutInMillis=30001) -> expect BadRequestException in both.
4. CreateRoute(Target='integrations/abc1234') and CreateRoute(AuthorizationType='CUSTOM', AuthorizerId='nonexistent123') -> expect rejection.
5. CreateRoute('GET /items/{id}') then CreateRoute('GET /items/{proxy+}') -> expect BadRequestException.
6. CreateIntegration(IntegrationType='HTTP_PROXY', PayloadFormatVersion='2.0') -> expect BadRequestException.
7. CreateIntegration(IntegrationType='AWS', TemplateSelectionExpression='$default') -> expect BadRequestException.
8. CreateApi(ProtocolType='WEBSOCKET') -> ApiEndpoint must start with wss://.
9. CreateRoute('GET /dup') twice -> ConflictException message without a trailing period.
