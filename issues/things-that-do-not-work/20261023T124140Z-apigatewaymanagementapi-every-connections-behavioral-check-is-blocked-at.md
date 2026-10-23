Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewaymanagementapi.agwmgmt-cross-api-connection-id-rejected apigatewaymanagementapi.agwmgmt-live-connection-get-post-delete-lifecycle apigatewaymanagementapi.agwmgmt-malformed-connection-id-rejected apigatewaymanagementapi.agwmgmt-post-oversize-payload-rejected-before-lookup apigatewaymanagementapi.agwmgmt-undeployed-stage-endpoint-forbidden apigatewaymanagementapi.client-initiated-close-converges-to-gone-with-live-twin apigatewaymanagementapi.concurrent-post-to-connection-all-delivered apigatewaymanagementapi.connection-operations-after-delete-return-gone-rejected apigatewaymanagementapi.cross-api-connection-id-returns-gone apigatewaymanagementapi.delete-connection-forces-client-disconnect apigatewaymanagementapi.delete-connection-triggers-disconnect-route apigatewaymanagementapi.forbidden-without-manage-connections-permission-rejected apigatewaymanagementapi.get-connection-last-active-at-advances-on-traffic-not-on-reads apigatewaymanagementapi.get-connection-last-active-at-not-before-connected-at apigatewaymanagementapi.get-connection-returns-identity-metadata apigatewaymanagementapi.get-connection-user-agent-matches-handshake-header apigatewaymanagementapi.multiple-connections-message-isolation apigatewaymanagementapi.payload-too-large-rejected-at-boundary apigatewaymanagementapi.post-to-connection-binary-payload-roundtrip apigatewaymanagementapi.post-to-connection-delivers-text-payload apigatewaymanagementapi.post-to-connection-empty-payload-delivers apigatewaymanagementapi.post-to-connection-fragments-large-payload-across-frames apigatewaymanagementapi.reconnect-produces-distinct-connection-and-later-connected-at apigatewaymanagementapi.sequential-posts-preserve-message-order
Issue-Shape-Id: com.amazonaws.apigatewaymanagementapi#ApiGatewayManagementApi
Issue-Key: apigatewaymanagementapi|CreateDeployment|NotImplemented
Issue-Commit: 001920e7906d433f041aca694d32f13b44f642fa

# apigatewaymanagementapi: every @connections behavioral check is blocked at setup because apigatewayv2 CreateDeployment answers NotImplemented (1 cause)

> _Note: this issue was identified against the code base at commit `001920e7906d433f041aca694d32f13b44f642fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeployment

## Description
One net-new root cause, reached through the apigatewaymanagementapi service token. All 24 apigatewaymanagementapi behavioral checks in this suite fail at the same setup step and none of the @connections data-plane behavior is exercised at all.

Cause 1 of 1: the WebSocket API a @connections endpoint is addressed through can never be deployed, because apigatewayv2 CreateDeployment is an unimplemented scaffold stub.
AWS: POST /v2/apis/{ApiId}/deployments returns 201 with DeploymentId/DeploymentStatus (Smithy, apigatewayv2: CreateDeployment -> CreateDeploymentResponse {DeploymentId, DeploymentStatus, AutoDeployed, CreatedDate}, http {method: POST, uri: /v2/apis/{ApiId}/deployments, code: 201}; modeled errors BadRequestException 400, ConflictException 409, NotFoundException 404, TooManyRequestsException 429 — NotImplemented is not modeled). Only once a stage is deployed does https://{api-id}.execute-api.{region}.amazonaws.com/{stage} serve the @connections operations (GetConnection, PostToConnection, DeleteConnection) whose behavior these checks grade: a live connection answering 200/204, an unknown-but-well-formed id answering GoneException/410, a malformed id answering BadRequestException/400, an undeployed stage answering ForbiddenException/403, and an oversize Data payload answering HTTP 413.
Emulator: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateDeployment operation: operation CreateDeployment is scaffolded but not implemented in this emulator. The call raises before any connection is ever opened, so every check aborts in its fixture setup.
Why: a client cannot use the API Gateway Management API at all against this emulator. There is no deployable WebSocket stage, therefore no connection id can ever be minted and no @connections call can be made; the whole service's error taxonomy (Gone vs BadRequest vs Forbidden vs 413) is unverifiable. A deploy-then-push client fails on its first deploy call with an unmodeled error code that its SDK error handling has no branch for.
Fix: implement the CreateDeployment handler in the apigatewayv2 service crate's ops module (services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/ops/create_deployment.rs), persisting the deployment in that crate's state and linking it to a stage, so that the execute-api/@connections endpoint for a WEBSOCKET API resolves and the apigatewaymanagementapi handlers become reachable. The open issue 20261023T113357Z-apigatewayv2-api-gateway-v2-createstage-and-createdeployment-are-unimplemented.md describes the same stub for the apigatewayv2__lambda__logs / apigateway__dynamodb__lambda__logs tokens but does not carry any of the replay keys below, so these 24 checks are not covered by it.
Traces: all 24 failing apigatewaymanagementapi checks listed in trace_ids, e.g. apigatewaymanagementapi.agwmgmt-live-connection-get-post-delete-lifecycle, apigatewaymanagementapi.agwmgmt-malformed-connection-id-rejected, apigatewaymanagementapi.post-to-connection-delivers-text-payload.

## Evidence
Suite report: apigatewaymanagementapi 0/24 (0%) 24 failed. Every one of the 24 blocks carries the identical head:

  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateDeployment operation: operation CreateDeployment is scaffolded but not implemented in this emulator

Expected (AWS / Smithy): HTTP 201 with a CreateDeploymentResponse carrying DeploymentId and DeploymentStatus, after which the @connections endpoint for that API/stage serves GetConnection/PostToConnection/DeleteConnection. Actual: an unmodeled NotImplemented error at the deploy step, so no connection id is ever created and no @connections operation is reached.

## Steps to Reproduce
1. aws apigatewayv2 create-api --name t --protocol-type WEBSOCKET --route-selection-expression '$request.body.action'  (succeeds)
2. aws apigatewayv2 create-integration / create-route for $connect/$default (succeeds)
3. aws apigatewayv2 create-deployment --api-id <id>  -> ClientError NotImplemented ('operation CreateDeployment is scaffolded but not implemented in this emulator'); AWS returns 201 + DeploymentId.
4. Because step 3 fails, no WebSocket client can connect and no apigatewaymanagementapi call (get-connection / post-to-connection / delete-connection against https://<api-id>.execute-api.<region>.amazonaws.com/<stage>) can be issued.
