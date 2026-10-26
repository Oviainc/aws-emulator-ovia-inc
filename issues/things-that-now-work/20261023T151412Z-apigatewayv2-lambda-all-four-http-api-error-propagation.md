Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__lambda.xsvc-apigatewayv2-3-http-api-integration-timeout-50ms-rejected-503 apigatewayv2__lambda.xsvc-apigatewayv2-3-http-api-lambda-handler-raises-rejected-500 apigatewayv2__lambda.xsvc-apigatewayv2-3-http-api-lambda-without-invoke-permission-rejected-500 apigatewayv2__lambda.xsvc-apigatewayv2-3-http-api-malformed-proxy-response-rejected-500-not-502
Issue-Key: apigatewayv2__lambda|CreateStage|NotImplemented
Issue-Commit: e2dfe022d58b39493003b3d90859f84840a12f00

# apigatewayv2__lambda: all four HTTP-API error-propagation checks abort because apigatewayv2 CreateStage answers NotImplemented (1 cause)

> _Note: this issue was identified against the code base at commit `e2dfe022d58b39493003b3d90859f84840a12f00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStage

## Description
Cause 1 of 1: apigatewayv2 CreateStage is an unimplemented scaffold stub, so an HTTP API backed by a Lambda AWS_PROXY integration can never be given a stage, and the four checks that grade how the gateway maps backend faults to client status codes never reach a single invoke.
AWS: Smithy model (services/service-com-amazonaws-apigatewayv2-apigatewayv2/smithy-model) CreateStage -> POST /v2/apis/{ApiId}/stages, success code 201, returning the stage document (StageName, AutoDeploy, DeploymentId, DefaultRouteSettings, RouteSettings, CreatedDate...); modelled errors are BadRequestException (400), ConflictException (409), NotFoundException (404) and TooManyRequestsException (429) — 'NotImplemented' is not among them. A valid CreateStage(ApiId=<api>, StageName='$default', AutoDeploy=True) must return 201, after which (recorded behavior) the gateway answers 503 when the integration exceeds a 50 ms TimeoutInMillis, 500 when the Lambda handler raises, 500 when the gateway lacks lambda:InvokeFunction on the target, and 500 (not 502) when the handler returns a malformed proxy response.
Emulator: CreateStage answers error code 'NotImplemented' with message 'operation CreateStage is scaffolded but not implemented in this emulator', aborting all four checks at setup.
Why: no HTTP API is invocable, so every backend-fault-to-status-code mapping a client depends on (503 on integration timeout, 500 on handler exception, 500 on missing invoke permission, 500 rather than 502 on a malformed proxy response) is unemulated, and callers see an unmodelled error code where AWS returns a created stage.
Fix: the CreateStage handler and stage state (services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/ops/create_stage.rs), then the stage's data-plane invoke path and its integration error mapping.
Note: the open issues 20261023T124140Z-apigatewayv2-createstage-stub.md and 20261023T113357Z-apigatewayv2-api-gateway-v2-createstage-and-createdeployment-are-unimplemented.md name the same handler but carry none of the replay keys below.
Traces: apigatewayv2__lambda.xsvc-apigatewayv2-3-http-api-integration-timeout-50ms-rejected-503, apigatewayv2__lambda.xsvc-apigatewayv2-3-http-api-lambda-handler-raises-rejected-500, apigatewayv2__lambda.xsvc-apigatewayv2-3-http-api-lambda-without-invoke-permission-rejected-500, apigatewayv2__lambda.xsvc-apigatewayv2-3-http-api-malformed-proxy-response-rejected-500-not-502.

## Evidence
Replay of suite simparity-behavioral-aws-v1: apigatewayv2__lambda 0/13 (0%); these four checks each fail with the identical error:
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateStage operation: operation CreateStage is scaffolded but not implemented in this emulator

Expected: CreateStage returns HTTP 201 with the stage document; modelled errors are only BadRequestException/ConflictException/NotFoundException/TooManyRequestsException.
Actual: error code NotImplemented at setup, so the 503/500/500/500-not-502 assertions were never evaluated.

None of the four checks carried an 'ALREADY FILED' or 'OWNED by' marker.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Create a Lambda function, CreateApi(ProtocolType='HTTP'), CreateIntegration(IntegrationType='AWS_PROXY', IntegrationUri=<arn>, PayloadFormatVersion='2.0', TimeoutInMillis=50), CreateRoute(RouteKey='GET /x', Target='integrations/<id>').
3. Call CreateStage(ApiId=<api>, StageName='$default', AutoDeploy=True). Expect HTTP 201 and the stage document; observe ClientError NotImplemented.
4. Re-replay the four check ids in trace_ids; each should then invoke the stage URL and assert 503 (integration timeout), 500 (handler raises), 500 (no lambda:InvokeFunction) and 500-not-502 (malformed proxy response).
