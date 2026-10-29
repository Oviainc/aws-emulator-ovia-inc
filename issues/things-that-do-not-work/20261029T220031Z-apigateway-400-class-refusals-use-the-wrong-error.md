Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewayv2-domain-name-create-server-defaults
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewayv2-domain-name-create-server-defaults apigatewayv2-import-api-basepath-split apigatewayv2-import-api-failonwarnings-rejected apigatewayv2-import-reimport-error-recovery apigatewayv2-integration-error-paths apigatewayv2-integration-response-errors apigatewayv2-model-error-paths apigatewayv2-route-response-errors apigatewayv2-routes-pagination-max-results-one
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigateway|CreateDomainName|4xx->4xx|$.message
Issue-Key: apigateway|CreateDomainName|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: apigateway|CreateDomainName|4xx->4xx|status
Issue-Key: apigateway|ImportApi|4xx->4xx|$.message
Issue-Key: apigateway|ReimportApi|4xx->4xx|$.message
Issue-Key: apigateway|ReimportApi|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: apigateway|ReimportApi|4xx->4xx|status
Issue-Key: apigateway|CreateIntegration|4xx->4xx|$.message
Issue-Key: apigateway|CreateIntegrationResponse|4xx->4xx|$.message
Issue-Key: apigateway|CreateIntegrationResponse|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: apigateway|CreateIntegrationResponse|4xx->4xx|status
Issue-Key: apigateway|CreateModel|4xx->4xx|$.message
Issue-Key: apigateway|CreateRouteResponse|4xx->4xx|$.message
Issue-Key: apigateway|CreateRouteResponse|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: apigateway|CreateRouteResponse|4xx->4xx|status
Issue-Key: apigateway|GetRoutes|4xx->4xx|$.message
Issue-Commit: 89c27cbcf326b5c1a1abaeca904759cc614d9072

# apigateway 400-class refusals use the wrong error type and message

> _Note: this issue was identified against the code base at commit `89c27cbcf326b5c1a1abaeca904759cc614d9072`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomainName

## Description
Eight refusal paths answer with an emulator-authored error: the wrong HTTP status, the wrong x-amzn-errortype, and/or a message that is not AWS's text. Each cause names the required status, error code and message template (fixed text vs values taken from the request or state).

Cause 1 of 8: CreateDomainName on an existing domain name.
AWS: 400 with x-amzn-errortype `BadRequestException` and the message `The domain name you provided already exists.` - the whole sentence is fixed text; AWS does NOT interpolate the domain name.
Emulator: 409 with x-amzn-errortype `ConflictException` and the message `The domain name <domainName> already exists.`
Why: a client that branches on 400/BadRequestException to treat the duplicate as a benign already-exists sees an unhandled 409, and a client matching AWS's message text never matches.
Fix: the CreateDomainName handler's duplicate branch (status, error shape and message).
Traces: apigatewayv2-domain-name-create-server-defaults#task#4.

Cause 2 of 8: ImportApi with an unrecognized basepath option.
AWS: 400 with the message `Unrecognized base path option: <basepath>` - fixed text `Unrecognized base path option: ` plus the rejected value from the `basepath` query parameter.
Emulator: `Invalid value for Basepath: '<basepath>'. Valid values are: ignore, prepend, split.`
Why: the client-visible reason text differs from AWS's, so message-based assertions and user-facing error surfacing diverge.
Fix: the ImportApi basepath validation message.
Traces: apigatewayv2-import-api-basepath-split#task#7.

Cause 3 of 8: ReimportApi against a non-existent api id.
AWS: 404 with x-amzn-errortype `NotFoundException` and the message `Invalid API identifier specified <accountId>:<apiId>` - fixed text `Invalid API identifier specified ` plus the caller's account id and the api id from the request path, joined by a colon. The api-not-found check runs BEFORE the body is validated.
Emulator: 400 with x-amzn-errortype `BadRequestException` and a body-validation message about the spec's integration type.
Why: a client cannot distinguish "this api does not exist" from "this spec is bad", and retry/create-if-missing logic keyed on 404 never fires.
Fix: the ReimportApi handler's validation order - resolve the api id first and refuse with NotFoundException.
Traces: apigatewayv2-import-reimport-error-recovery#task#4.

Cause 4 of 8: CreateIntegration with ConnectionType VPC_LINK and no ConnectionId.
AWS: 400 with the message `ConnectionId must be set to vpcLinkId for ConnectionType VPC_LINK` - the whole sentence is fixed text.
Emulator: `ConnectionId is required for a connection of type VPC_LINK.`
Why/Fix: the CreateIntegration validation message.
Traces: apigatewayv2-integration-error-paths#task#6.

Cause 5 of 8: CreateIntegrationResponse with a duplicate integration-response key.
AWS: 400 with x-amzn-errortype `BadRequestException` and the message `IntegrationResponse with key <integrationResponseKey> is already present in Integration with ID <integrationId>` - fixed text `IntegrationResponse with key ` / ` is already present in Integration with ID `, with the key from the request body and the integration id from the request path.
Emulator: 409 with x-amzn-errortype `ConflictException` and the message `Integration response with key <key> already exists for this integration.`
Why: duplicate handling keyed on 400/BadRequestException breaks on the 409.
Fix: the CreateIntegrationResponse duplicate branch (status, error shape and message).
Traces: apigatewayv2-integration-response-errors#task#4.

Cause 6 of 8: CreateModel with a duplicate model name.
AWS: 409 with the message `Model name already exists for this REST API` - the whole sentence is fixed text; AWS does NOT interpolate the model name.
Emulator: 409 with the message `Model with name <modelName> already exists for this API.`
Why/Fix: the CreateModel duplicate-name message.
Traces: apigatewayv2-model-error-paths#task#3.

Cause 7 of 8: CreateRouteResponse with a duplicate route-response key.
AWS: 400 with x-amzn-errortype `BadRequestException` and the message `RouteResponse with key <routeResponseKey> already exists on this Route. RouteResponse keys must be unique within a given Route.` - both sentences are fixed text apart from the key, which comes from the request body.
Emulator: 409 with x-amzn-errortype `ConflictException` and the message `Route response with key <key> already exists for this route.`
Why/Fix: the CreateRouteResponse duplicate branch (status, error shape and message).
Traces: apigatewayv2-route-response-errors#task#4.

Cause 8 of 8: GetRoutes with a non-integer maxResults.
AWS: 400 with the message `maxResults must be an integer.` - the whole sentence is fixed text; AWS does NOT echo the offending value.
Emulator: `Invalid value for MaxResults: '<value>'. MaxResults must be a positive integer.`
Why/Fix: the shared maxResults query-parameter validation message used by the collection handlers.
Traces: apigatewayv2-routes-pagination-max-results-one#task#12.

## Evidence
Replay verdict `mismatch`. Details include `status: expected 400, got 409` with `$headers.x-amzn-errortype: shape_mismatch (... got 'ConflictException')` (CreateDomainName, CreateIntegrationResponse, CreateRouteResponse); `status: expected 404, got 400` with `$headers.x-amzn-errortype: value_mismatch (expected 'NotFoundException', got 'BadRequestException')` (ReimportApi); and `$.message: value_mismatch` with both the AWS and emulator strings quoted for the remaining clusters.

## Steps to Reproduce
Replay the corpus. Cause 1: POST /v2/domainnames twice with the same domain name (fixture apigatewayv2-domain-name-create-server-defaults step task#4) - the emulator answers 409 ConflictException instead of 400 BadRequestException. Cause 3: PUT /v2/apis/{bogusApiId} with any body (apigatewayv2-import-reimport-error-recovery#task#4) - 400 BadRequestException instead of 404 NotFoundException.
