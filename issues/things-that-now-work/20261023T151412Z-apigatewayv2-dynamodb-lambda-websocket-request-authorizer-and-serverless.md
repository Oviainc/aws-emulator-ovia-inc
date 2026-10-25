Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-connect-request-authorizer-deny-refuses-before-integration apigatewayv2__dynamodb__lambda.e2e-serverless-visitor-ip-capture-lifecycle
Issue-Key: apigatewayv2__dynamodb__lambda|CreateAuthorizer|NotImplemented
Issue-Key: apigatewayv2__dynamodb__lambda|CreateStage|NotImplemented
Issue-Commit: e2dfe022d58b39493003b3d90859f84840a12f00

# apigatewayv2__dynamodb__lambda: WebSocket REQUEST-authorizer and serverless-lifecycle checks abort on apigatewayv2 CreateAuthorizer and CreateStage NotImplemented stubs (2 causes)

> _Note: this issue was identified against the code base at commit `e2dfe022d58b39493003b3d90859f84840a12f00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAuthorizer

## Description
Two distinct apigatewayv2 scaffold stubs abort two cross-service checks in this service group. Neither check carried an 'ALREADY FILED' or 'OWNED by' marker in the replay report. Two causes, numbered.

Cause 1 of 2: CreateAuthorizer answers NotImplemented, so a WebSocket $connect route cannot be fronted by a REQUEST authorizer.
AWS: Smithy model (services/service-com-amazonaws-apigatewayv2-apigatewayv2/smithy-model) CreateAuthorizer -> POST /v2/apis/{ApiId}/authorizers, success code 201, returning AuthorizerId and the authorizer document (AuthorizerType, AuthorizerUri, IdentitySource, AuthorizerPayloadFormatVersion, EnableSimpleResponses); modelled errors BadRequestException (400), ConflictException (409), NotFoundException (404), TooManyRequestsException (429) — 'NotImplemented' is not among them. A valid REQUEST authorizer must be created with 201, and a Deny policy from it must refuse the WebSocket handshake before the $connect integration is invoked.
Emulator: CreateAuthorizer answers error code 'NotImplemented' ('operation CreateAuthorizer is scaffolded but not implemented in this emulator'), aborting the check at setup.
Why: a client cannot protect a WebSocket API at all; the 'deny refuses before the integration runs' guarantee — the whole point of a $connect authorizer — is unobservable, and the error code returned is not one the API models.
Fix: the CreateAuthorizer handler and authorizer state (services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/ops/create_authorizer.rs), then the $connect-time authorizer invocation and handshake refusal path.
Traces: apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-connect-request-authorizer-deny-refuses-before-integration.

Cause 2 of 2: CreateStage answers NotImplemented, so the end-to-end serverless visitor-capture lifecycle cannot be deployed.
AWS: Smithy CreateStage -> POST /v2/apis/{ApiId}/stages, success code 201, returning the stage document (StageName, AutoDeploy, DeploymentId, RouteSettings, CreatedDate...); modelled errors BadRequestException (400), ConflictException (409), NotFoundException (404), TooManyRequestsException (429). A valid CreateStage must return 201 and the stage must then serve the API's routes.
Emulator: CreateStage answers error code 'NotImplemented' ('operation CreateStage is scaffolded but not implemented in this emulator'), and the check's phase runner aborts phase 2/4 ('solve').
Why: no HTTP API can be made invocable, so the whole API Gateway -> Lambda -> DynamoDB lifecycle (write on invoke, read back, teardown) is unreachable for any client.
Fix: the CreateStage handler and stage state (services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/ops/create_stage.rs), then the stage's data-plane invoke path.
Traces: apigatewayv2__dynamodb__lambda.e2e-serverless-visitor-ip-capture-lifecycle.

Note: the open issues 20261023T125319Z-apigatewayv2-createauthorizer-stub.md, 20261023T124140Z-apigatewayv2-createstage-stub.md and 20261023T124140Z-apigatewayv2-dynamodb-lambda-websocket-duplex-checks-all-abort.md (which is about CreateDeployment) name the same handlers but carry neither of the replay keys above.

## Evidence
Replay of suite simparity-behavioral-aws-v1: apigatewayv2__dynamodb__lambda 0/9 (0%).

Cause 1 — apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-connect-request-authorizer-deny-refuses-before-integration [error]:
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateAuthorizer operation: operation CreateAuthorizer is scaffolded but not implemented in this emulator
Expected: HTTP 201 with an AuthorizerId.

Cause 2 — apigatewayv2__dynamodb__lambda.e2e-serverless-visitor-ip-capture-lifecycle [error]:
_PhaseError: [phase 2/4 solve] ClientError: An error occurred (NotImplemented) when calling the CreateStage operation: operation CreateStage is scaffolded but not implemented in this emulator
Expected: HTTP 201 with the stage document.

Both checks abort before any DynamoDB or Lambda assertion runs. Neither carried an 'ALREADY FILED' or 'OWNED by' marker.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Cause 1: CreateApi(ProtocolType='WEBSOCKET', RouteSelectionExpression='$request.body.action'), then CreateAuthorizer(ApiId=<api>, AuthorizerType='REQUEST', AuthorizerUri=<lambda invoke uri>, IdentitySource=['route.request.querystring.token'], AuthorizerPayloadFormatVersion='2.0'). Expect HTTP 201 with AuthorizerId; observe ClientError NotImplemented.
3. Cause 2: CreateApi(ProtocolType='HTTP') with an AWS_PROXY integration and route, then CreateStage(ApiId=<api>, StageName='$default', AutoDeploy=True). Expect HTTP 201 with the stage document; observe ClientError NotImplemented.
4. Re-replay the two check ids in trace_ids.
