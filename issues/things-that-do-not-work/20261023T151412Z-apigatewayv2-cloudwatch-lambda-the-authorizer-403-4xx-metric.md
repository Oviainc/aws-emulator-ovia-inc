Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__cloudwatch__lambda.metrics-elbv2-apigw-http-api-authorizer-forbidden-403-counts-lowercase-4xx-under-apiid
Issue-Key: apigatewayv2__cloudwatch__lambda|CreateAuthorizer|NotImplemented
Issue-Commit: e2dfe022d58b39493003b3d90859f84840a12f00

# apigatewayv2__cloudwatch__lambda: the authorizer-403 4xx-metric check aborts because apigatewayv2 CreateAuthorizer answers NotImplemented (1 cause)

> _Note: this issue was identified against the code base at commit `e2dfe022d58b39493003b3d90859f84840a12f00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAuthorizer

## Description
Cause 1 of 1: apigatewayv2 CreateAuthorizer is an unimplemented scaffold stub, so an HTTP API cannot be given a Lambda REQUEST authorizer and the check that grades a 403-from-authorizer being counted as a lowercase 4xx metric under the ApiId dimension never reaches any CloudWatch assertion.
AWS: Smithy model (services/service-com-amazonaws-apigatewayv2-apigatewayv2/smithy-model) CreateAuthorizer -> POST /v2/apis/{ApiId}/authorizers, success code 201, returning AuthorizerId plus the authorizer document (AuthorizerType, AuthorizerUri, IdentitySource, AuthorizerPayloadFormatVersion, EnableSimpleResponses, AuthorizerResultTtlInSeconds 0..3600); modelled errors are BadRequestException (400), ConflictException (409), NotFoundException (404) and TooManyRequestsException (429) — 'NotImplemented' is not among them. A valid CreateAuthorizer must return 201, after which a request the authorizer denies is answered 403 and counted in the AWS/ApiGateway 4xx series dimensioned by ApiId.
Emulator: CreateAuthorizer answers error code 'NotImplemented' with message 'operation CreateAuthorizer is scaffolded but not implemented in this emulator'.
Why: no authorizer can be attached to any API, so authorization outcomes and the metric series they drive are both unobservable; a client sees an unmodelled error code where AWS returns a created authorizer.
Fix: the CreateAuthorizer handler and authorizer state (services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/ops/create_authorizer.rs), then the data-plane authorizer invocation and its 4xx metric emission.
Note: the open issues 20261023T125319Z-apigatewayv2-createauthorizer-stub.md and 20261023T124140Z-apigatewayv2-lambda-websocket-request-authorizer-checks-abort-because.md name the same handler but do not carry this check's replay key.
Traces: apigatewayv2__cloudwatch__lambda.metrics-elbv2-apigw-http-api-authorizer-forbidden-403-counts-lowercase-4xx-under-apiid.

## Evidence
Replay of suite simparity-behavioral-aws-v1: apigatewayv2__cloudwatch__lambda 0/3 (0%), this check failing with:
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateAuthorizer operation: operation CreateAuthorizer is scaffolded but not implemented in this emulator

Expected: CreateAuthorizer returns HTTP 201 with an AuthorizerId (modelled errors only BadRequestException/ConflictException/NotFoundException/TooManyRequestsException).
Actual: error code NotImplemented, check aborts before any invoke or metric read. The check carried no 'ALREADY FILED' or 'OWNED by' marker.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Create a Lambda authorizer function and an HTTP API (CreateApi ProtocolType='HTTP').
3. Call CreateAuthorizer(ApiId=<api>, Name=..., AuthorizerType='REQUEST', AuthorizerUri=<lambda invoke uri>, IdentitySource=['$request.header.Authorization'], AuthorizerPayloadFormatVersion='2.0'). Expect HTTP 201 with AuthorizerId; observe ClientError NotImplemented.
4. Re-replay apigatewayv2__cloudwatch__lambda.metrics-elbv2-apigw-http-api-authorizer-forbidden-403-counts-lowercase-4xx-under-apiid.
