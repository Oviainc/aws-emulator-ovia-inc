Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__cloudwatch.http-api-count-and-4xx-metrics-carry-the-apiid-stage-dimensions apigatewayv2__cloudwatch.http-api-missing-invoke-permission-rejected-as-5xx-metric apigatewayv2__cloudwatch.http-api-route-throttle-zero-rejected-429-counted-in-4xx apigatewayv2__cloudwatch.obs-apigatewayv2-2-http-api-detailed-metrics-opt-in-adds-method-resource-series
Issue-Key: apigatewayv2__cloudwatch|CreateStage|NotImplemented
Issue-Commit: e2dfe022d58b39493003b3d90859f84840a12f00

# apigatewayv2__cloudwatch: every HTTP-API metric check aborts at setup because apigatewayv2 CreateStage answers NotImplemented (1 cause)

> _Note: this issue was identified against the code base at commit `e2dfe022d58b39493003b3d90859f84840a12f00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStage

## Description
Cause 1 of 1: apigatewayv2 CreateStage is an unimplemented scaffold stub, so no HTTP API can be given a stage and none of the apigatewayv2+cloudwatch metric checks ever reach the CloudWatch assertions they exist to grade.
AWS: Smithy model (services/service-com-amazonaws-apigatewayv2-apigatewayv2/smithy-model) CreateStage -> POST /v2/apis/{ApiId}/stages, success code 201, returning the stage document (StageName, AutoDeploy, DeploymentId, RouteSettings, DefaultRouteSettings, CreatedDate...); modelled errors are BadRequestException (400), ConflictException (409), NotFoundException (404) and TooManyRequestsException (429) — 'NotImplemented' is not among them. A valid CreateStage(ApiId=<api>, StageName='$default', AutoDeploy=True) must return 201 with the stage, after which the stage serves traffic and AWS/ApiGateway emits Count/4xx/5xx series dimensioned by ApiId and Stage.
Emulator: CreateStage answers error code 'NotImplemented' with message 'operation CreateStage is scaffolded but not implemented in this emulator', aborting each check at setup before any invoke or GetMetricStatistics call is made.
Why: a client can neither deploy an HTTP API nor observe any API Gateway metric: the ApiId/Stage dimensioned Count, 4xx and 5xx series, the 429 produced by a zero route throttle, the 5xx produced by a missing lambda invoke permission, and the extra Method/Resource series that DetailedMetricsEnabled adds are all unreachable. Every HTTP-API observability path is dark.
Fix: the CreateStage handler and stage state (services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/ops/create_stage.rs), then the stage's data-plane invoke path and the AWS/ApiGateway metric emission that feeds cloudwatch.
Note: the open issues 20261023T124140Z-apigatewayv2-createstage-stub.md and 20261023T113357Z-apigatewayv2-api-gateway-v2-createstage-and-createdeployment-are-unimplemented.md name the same handler but carry none of the replay keys below, so these four checks are not covered by them.
Traces: apigatewayv2__cloudwatch.http-api-count-and-4xx-metrics-carry-the-apiid-stage-dimensions, apigatewayv2__cloudwatch.http-api-missing-invoke-permission-rejected-as-5xx-metric, apigatewayv2__cloudwatch.http-api-route-throttle-zero-rejected-429-counted-in-4xx, apigatewayv2__cloudwatch.obs-apigatewayv2-2-http-api-detailed-metrics-opt-in-adds-method-resource-series.

## Evidence
Replay of suite simparity-behavioral-aws-v1: apigatewayv2__cloudwatch 0/4 (0%), 4 failed, all with the same error.

Expected: CreateStage returns HTTP 201 with the stage document (modelled errors only BadRequestException/ConflictException/NotFoundException/TooManyRequestsException).
Actual (all four checks): botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateStage operation: operation CreateStage is scaffolded but not implemented in this emulator.

Because setup aborts, no metric assertion in these checks was ever evaluated — the CloudWatch side is untested, not passing. None of the four checks carried an 'ALREADY FILED' or 'OWNED by' marker.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. CreateApi(Name=..., ProtocolType='HTTP'); CreateIntegration(IntegrationType='AWS_PROXY', IntegrationUri=<lambda arn>, PayloadFormatVersion='2.0'); CreateRoute(RouteKey='GET /x', Target='integrations/<id>').
3. Call CreateStage(ApiId=<api>, StageName='$default', AutoDeploy=True). Expect HTTP 201 and the stage document; observe ClientError NotImplemented.
4. Re-replay the four check ids in trace_ids; each should then proceed to its GetMetricStatistics assertions on AWS/ApiGateway Count/4xx/5xx with ApiId and Stage dimensions.
