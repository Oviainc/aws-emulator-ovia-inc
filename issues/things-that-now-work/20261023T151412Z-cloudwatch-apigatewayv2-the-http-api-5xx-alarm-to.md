Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__cloudwatch.obs-apigatewayv2-3-http-api-5xx-alarm-notifies-sqs-via-sns
Issue-Key: apigatewayv2__cloudwatch|CreateStage|NotImplemented
Issue-Commit: e2dfe022d58b39493003b3d90859f84840a12f00

# cloudwatch__apigatewayv2: the HTTP-API 5xx alarm-to-SNS-to-SQS check aborts because apigatewayv2 CreateStage answers NotImplemented (1 cause)

> _Note: this issue was identified against the code base at commit `e2dfe022d58b39493003b3d90859f84840a12f00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStage

## Description
Cause 1 of 1: apigatewayv2 CreateStage is an unimplemented scaffold stub, so the HTTP API whose 5xx series is supposed to drive a CloudWatch alarm can never be deployed, and the alarm -> SNS -> SQS notification path is never exercised.
AWS: Smithy model (services/service-com-amazonaws-apigatewayv2-apigatewayv2/smithy-model) CreateStage -> POST /v2/apis/{ApiId}/stages, success code 201, returning the stage document (StageName, AutoDeploy, DeploymentId, CreatedDate...); modelled errors are BadRequestException (400), ConflictException (409), NotFoundException (404) and TooManyRequestsException (429) — 'NotImplemented' is not among them. A valid CreateStage must return 201, after which requests whose integration answers HTTP 500 raise the AWS/ApiGateway 5xx series for that ApiId/Stage, a CloudWatch alarm on it transitions to ALARM, and its SNS action delivers a notification to the subscribed SQS queue.
Emulator: CreateStage answers error code 'NotImplemented' with message 'operation CreateStage is scaffolded but not implemented in this emulator', aborting the check at setup before any metric, alarm transition or queue read.
Why: an observability pipeline built on API Gateway metrics cannot be emulated end to end — the client never gets a servable stage, so no 5xx datapoint, no alarm transition and no SNS/SQS notification occur, and the error code returned is not one the API models.
Fix: the CreateStage handler and stage state (services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/ops/create_stage.rs), then the stage's data-plane invoke path and AWS/ApiGateway 5xx metric emission that the alarm evaluates.
Note: the open issues 20261023T124140Z-apigatewayv2-createstage-stub.md and 20261023T113357Z-apigatewayv2-api-gateway-v2-createstage-and-createdeployment-are-unimplemented.md name the same handler but do not carry this check's replay key.
Traces: cloudwatch__apigatewayv2.obs-apigatewayv2-3-http-api-5xx-alarm-notifies-sqs-via-sns.

## Evidence
Replay of suite simparity-behavioral-aws-v1: cloudwatch__apigatewayv2 0/1 (0%), failing with:
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateStage operation: operation CreateStage is scaffolded but not implemented in this emulator

Expected: CreateStage returns HTTP 201 with the stage document (modelled errors only BadRequestException/ConflictException/NotFoundException/TooManyRequestsException).
Actual: error code NotImplemented; the alarm/SNS/SQS assertions were never evaluated. The check carried no 'ALREADY FILED' or 'OWNED by' marker.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Create a Lambda whose handler returns HTTP 500, CreateApi(ProtocolType='HTTP'), an AWS_PROXY integration and a route to it.
3. Call CreateStage(ApiId=<api>, StageName='$default', AutoDeploy=True). Expect HTTP 201 and the stage document; observe ClientError NotImplemented.
4. Re-replay cloudwatch__apigatewayv2.obs-apigatewayv2-3-http-api-5xx-alarm-notifies-sqs-via-sns; it should then invoke the stage, see the AWS/ApiGateway 5xx datapoints, watch the alarm go to ALARM and read the SNS notification off the subscribed SQS queue.
