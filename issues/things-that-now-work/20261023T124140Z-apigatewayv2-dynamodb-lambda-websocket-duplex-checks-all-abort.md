Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__dynamodb__lambda.websocket-api-connect-message-post-back-disconnect apigatewayv2__dynamodb__lambda.wsduplex-client-binary-frame-rejected-close-1003 apigatewayv2__dynamodb__lambda.wsduplex-connect-denial-rejects-handshake apigatewayv2__dynamodb__lambda.wsduplex-oversize-client-message-rejected-close-1009
Issue-Key: apigatewayv2__dynamodb__lambda|CreateDeployment|NotImplemented
Issue-Commit: 001920e7906d433f041aca694d32f13b44f642fa

# apigatewayv2__dynamodb__lambda: WebSocket duplex checks all abort at deploy because apigatewayv2 CreateDeployment answers NotImplemented (1 cause)

> _Note: this issue was identified against the code base at commit `001920e7906d433f041aca694d32f13b44f642fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeployment

## Description
One net-new root cause, reached through the apigatewayv2__dynamodb__lambda cross-service token. All 4 checks of this service fail with the identical head, before any WebSocket handshake is attempted.

Cause 1 of 1: apigatewayv2 CreateDeployment is an unimplemented scaffold stub, so the WebSocket API that backs the $connect/$default/$disconnect Lambda integrations can never be promoted to a stage.
AWS: POST /v2/apis/{ApiId}/deployments returns 201 with DeploymentId and DeploymentStatus, which the stage then references (Smithy, apigatewayv2: CreateDeployment -> CreateDeploymentResponse {DeploymentId, DeploymentStatus, AutoDeployed, CreatedDate}; http {POST, /v2/apis/{ApiId}/deployments, 201}; modeled errors BadRequestException 400, ConflictException 409, NotFoundException 404, TooManyRequestsException 429 — NotImplemented is not modeled). Once deployed, wss://{api-id}.execute-api.{region}.amazonaws.com/{stage} accepts the RFC 6455 upgrade, fires $connect (a Deny/non-2xx authorizer or handler response rejecting the handshake), routes client frames, and closes with the protocol-specific codes these checks grade (1003 for an unsupported binary frame, 1009 for a message over the frame limit).
Emulator: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateDeployment operation: operation CreateDeployment is scaffolded but not implemented in this emulator.
Why: the entire WebSocket duplex data plane is unreachable. A client that creates routes and integrations successfully then fails at deploy with an error code that is not in the model, and none of the handshake-rejection or close-code behavior can be relied on.
Fix: implement the CreateDeployment handler in the apigatewayv2 service crate's ops module (services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/ops/create_deployment.rs) and link deployments to stages in that crate's state, so the WebSocket stage endpoint exists and the duplex path can be graded. The open issue 20261023T113357Z-apigatewayv2-api-gateway-v2-createstage-and-createdeployment-are-unimplemented.md records the same stub under the apigatewayv2__lambda__logs / apigatewayv2__dynamodb__lambda__logs__s3 tokens, but carries none of the replay keys below.
Traces: apigatewayv2__dynamodb__lambda.websocket-api-connect-message-post-back-disconnect, .wsduplex-client-binary-frame-rejected-close-1003, .wsduplex-connect-denial-rejects-handshake, .wsduplex-oversize-client-message-rejected-close-1009.

## Evidence
Suite report: apigatewayv2__dynamodb__lambda 0/4 (0%) 4 failed; all four blocks end with

  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateDeployment operation: operation CreateDeployment is scaffolded but not implemented in this emulator

Expected: HTTP 201 + CreateDeploymentResponse{DeploymentId, DeploymentStatus}. Actual: unmodeled NotImplemented, so the stage is never created and the WebSocket upgrade is never attempted.

## Steps to Reproduce
1. Create a WEBSOCKET api, a Lambda AWS_PROXY integration and the $connect/$default/$disconnect routes (all succeed).
2. aws apigatewayv2 create-deployment --api-id <id> --stage-name dev -> ClientError NotImplemented.
3. With no deployment, wss://<api-id>.execute-api.<region>.amazonaws.com/dev cannot be opened, so the binary-frame (1003), oversize-message (1009) and $connect-denial handshake behaviors cannot be observed.
