Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda__logs.apigw-http-api-lambda-integration-positive apigateway__dynamodb__lambda__logs.apigw-http-api-lambda-missing-permission-negative apigateway__dynamodb__lambda__logs.apigw-websocket-api-lambda-integration-positive apigateway__dynamodb__lambda__logs.apigw-websocket-connect-disconnect-logged apigatewayv2__lambda__logs.access-log-record-joins-the-gateway-request-id apigatewayv2__lambda__logs.lambda-proxy-integration-round-trips-uri-and-resolves-the-backend-function apigatewayv2__lambda__logs.prod-stage-round-trips-its-deployment-and-default-route-settings apigatewayv2__lambda__logs.registration-reply-is-delivered-via-the-https-callback-url apigatewayv2__dynamodb__lambda__logs__s3.e2e-debug-websocket-api-backend-response-lifecycle
Issue-Key: apigateway__dynamodb__lambda__logs|CreateStage|NotImplemented
Issue-Key: apigatewayv2__lambda__logs|CreateStage|NotImplemented
Issue-Key: apigatewayv2__lambda__logs|CreateDeployment|NotImplemented
Issue-Key: apigatewayv2__dynamodb__lambda__logs__s3|CreateDeployment|NotImplemented
Issue-Commit: 048778663a26e363156c6db724999c4fb229af36

# API Gateway v2: CreateStage and CreateDeployment are unimplemented scaffold stubs, so no HTTP or WebSocket API can be deployed (2 causes)

> _Note: this issue was identified against the code base at commit `048778663a26e363156c6db724999c4fb229af36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStage

## Description
Two net-new root causes in the apigatewayv2 emulator. BOTH must be closed before submitting.

Cause 1 of 2: CreateStage answers NotImplemented.
AWS: POST /v2/apis/{apiId}/stages creates the stage and returns 201 with the Stage representation (StageName, AutoDeploy, AccessLogSettings, DefaultRouteSettings, DeploymentId). Smithy (apigatewayv2): CreateStage -> CreateStageResponse; modeled errors NotFoundException/TooManyRequestsException/BadRequestException/ConflictException - NotImplemented is not modeled.
Emulator: ClientError code 'NotImplemented', message 'operation CreateStage is scaffolded but not implemented in this emulator'.
Why: an HTTP or WebSocket API cannot be given a stage, so nothing downstream (access logging, route settings, invoke URL) can exist. Clients that create APIs declaratively fail at the first deploy step.
Fix: implement the CreateStage handler in the apigatewayv2 service crate's ops module, persisting stage state (plus AccessLogSettings/DefaultRouteSettings) so GetStage/UpdateStage read it back.
Traces: apigateway__dynamodb__lambda__logs.apigw-http-api-lambda-integration-positive, .apigw-http-api-lambda-missing-permission-negative, .apigw-websocket-api-lambda-integration-positive, .apigw-websocket-connect-disconnect-logged, apigatewayv2__lambda__logs.access-log-record-joins-the-gateway-request-id.

Cause 2 of 2: CreateDeployment answers NotImplemented.
AWS: POST /v2/apis/{apiId}/deployments creates a deployment of the current API configuration and returns 201 with DeploymentId/DeploymentStatus; a stage references it. Smithy (apigatewayv2): CreateDeployment -> CreateDeploymentResponse with NotFoundException/TooManyRequestsException/BadRequestException/ConflictException.
Emulator: ClientError code 'NotImplemented', message 'operation CreateDeployment is scaffolded but not implemented in this emulator'.
Why: the API's routes/integrations are never promoted, so a client sees an API that exists but can never serve traffic, and a stage cannot round-trip its DeploymentId.
Fix: implement the CreateDeployment handler in the apigatewayv2 service crate's ops module and link deployments to stages in that crate's state.
Traces: apigatewayv2__lambda__logs.lambda-proxy-integration-round-trips-uri-and-resolves-the-backend-function, .prod-stage-round-trips-its-deployment-and-default-route-settings, .registration-reply-is-delivered-via-the-https-callback-url, apigatewayv2__dynamodb__lambda__logs__s3.e2e-debug-websocket-api-backend-response-lifecycle.

## Evidence
Cause 1: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateStage operation: operation CreateStage is scaffolded but not implemented in this emulator. Expected: 201 + Stage.
Cause 2: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateDeployment operation: operation CreateDeployment is scaffolded but not implemented in this emulator; in the websocket e2e it surfaces as _PhaseError: [phase 1/6 deploy] ClientError ... CreateDeployment. Expected: 201 + DeploymentId/DeploymentStatus.

## Steps to Reproduce
aws apigatewayv2 create-api --name t --protocol-type HTTP --target <lambda-arn>; then aws apigatewayv2 create-stage --api-id <id> --stage-name prod  -> NotImplemented. Then aws apigatewayv2 create-deployment --api-id <id> -> NotImplemented.
