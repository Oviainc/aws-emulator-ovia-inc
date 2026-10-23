Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__dynamodb__lambda.websocket-api-connect-message-post-back-disconnect apigatewayv2__dynamodb__lambda.wsduplex-client-binary-frame-rejected-close-1003 apigatewayv2__dynamodb__lambda.wsduplex-connect-denial-rejects-handshake
Issue-Key: apigatewayv2__dynamodb__lambda.websocket-api-connect-message-post-back-disconnect|_poll_until#14|Timeout|time.sleep(interval_s)
Issue-Key: apigatewayv2__dynamodb__lambda.wsduplex-client-binary-frame-rejected-close-1003|_poll_until#2|Timeout|time.sleep(interval_s)
Issue-Key: apigatewayv2__dynamodb__lambda.wsduplex-connect-denial-rejects-handshake|_poll_until#4|Timeout|time.sleep(interval_s)
Issue-Commit: 836bc7a08cca6ccbbb64b51e180fe3d49cf03502

# apigatewayv2__dynamodb__lambda: all three WebSocket duplex checks hang to the 60 s cap — the deployed WEBSOCKET stage never completes the RFC 6455 upgrade (1 cause)

> _Note: this issue was identified against the code base at commit `836bc7a08cca6ccbbb64b51e180fe3d49cf03502`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
$connect (WebSocket data plane)

## Description
Cause 1 of 1: three cross-service WebSocket checks build their stack successfully (DynamoDB table, Lambda, apigatewayv2 WEBSOCKET API with $connect/$disconnect/$default routes, deployment, stage) and then never obtain a 101 upgrade on the deployed stage, so each hangs to the 60 s harness cap. Failing check ids: apigatewayv2__dynamodb__lambda.websocket-api-connect-message-post-back-disconnect, apigatewayv2__dynamodb__lambda.wsduplex-client-binary-frame-rejected-close-1003, apigatewayv2__dynamodb__lambda.wsduplex-connect-denial-rejects-handshake.

AWS: recorded/documented behavior only — the opening handshake is the execute-api data plane, not a modeled operation. Smithy (apigatewayv2 2018-11-29): ProtocolType enum [WEBSOCKET, HTTP]; Api.ApiEndpoint doc "The URI of the API, of the form {api-id}.execute-api.{region}.amazonaws.com. The stage name is typically appended to this URI to form a complete path to a deployed API stage"; Api.RouteSelectionExpression required, selecting the route from the frame. On real AWS the upgrade is answered HTTP/1.1 101 Switching Protocols with a Sec-WebSocket-Accept derived from the client key; $connect then runs with requestContext.connectionId/domainName/stage, a client text frame is routed by the route-selection expression to $default, the integration can push back via ApiGatewayManagementApi PostToConnection (POST /@connections/{ConnectionId}, Smithy restJson1), and a clean client close fires $disconnect.

Emulator: no 101 is ever returned on the deployed stage, so none of those legs is reachable. Each check blocks in a poll for the upgrade (`_poll_until(lambda: _try_ws_connect(ctx, api_id, stage), lambda s: s is not None, timeout_s=180.0, interval_s=5.0, description="WebSocket handshake against stage {stage}")`) and is cut by the 60 s harness cap, reporting `Check timed out after 60.0s`. The status actually returned for that upgrade is observed directly by the apigatewaymanagementapi checks in this same run: b'HTTP/1.1 404 Not Found'.

Why: a client relying on AWS behavior gets no WebSocket data plane at all. $connect/$default/$disconnect never fire, so nothing is written to or removed from DynamoDB, the integration's PostToConnection push-back never happens, and the negative paths these checks grade (a $connect integration denying the handshake, a binary frame being refused with close code 1003) can never be produced. Instead of a wrong answer the client gets a 404 on wss:// or an indefinite wait.

Fix: the execute-api data-plane front door / request router plus the apigatewayv2 WebSocket session layer — recognise the RFC 6455 upgrade on a deployed WEBSOCKET API's stage path and answer 101 + Sec-WebSocket-Accept instead of falling through to the generic 404; then mint a connection id, invoke $connect (honouring a non-2xx integration response by refusing the handshake), route frames via the route-selection expression, enforce the frame-opcode policy, and fire $disconnect on close. This is a capability gap, not an error-shape bug.

Related open issues, none of which covers this: "apigatewayv2__dynamodb__lambda: WebSocket duplex checks all abort at deploy because apigatewayv2 CreateDeployment answers NotImplemented (1 cause)" (20261023T124140Z-...) and "apigatewayv2__dynamodb__lambda: WebSocket REQUEST-authorizer and serverless-lifecycle checks abort on apigatewayv2 CreateAuthorizer and CreateStage NotImplemented" (20261023T151412Z-...) both describe an ABORT during control-plane setup; in this replay those calls succeed and the checks now reach the data plane and hang there, which is a different cause.

Traces: the three failing check ids listed above.

## Evidence
Suite: simparity-behavioral-ga-prod-v1, apigatewayv2__dynamodb__lambda 0/3 (0%).

Expected (real AWS): HTTP/1.1 101 Switching Protocols on the upgrade; $connect writes a conn# row to DynamoDB; a client text frame reaches $default and persists a msg# row; the integration's PostToConnection delivers {'echo': ...} back on the same socket; a clean close removes the conn# row. For the negative checks: a $connect integration returning a denial refuses the handshake (no upgrade), and a client binary frame on a text-only API is refused with a close frame carrying status code 1003.

Actual (emulator), all three: `Check timed out after 60.0s`.
  - websocket-api-connect-message-post-back-disconnect blocks on `_poll_until(lambda: _try_ws_connect(ctx, api_id, stage), lambda s: s is not None, timeout_s=180.0, interval_s=5.0, description=f"WebSocket handshake against stage {stage}")`, before its first DynamoDB poll (`exactly one conn# row written by the $connect route`). Control-plane setup preceding it (create_api ProtocolType=WEBSOCKET, create_integration AWS_PROXY, create_route for $connect/$disconnect/$default, lambda add_permission, create_deployment, create_stage) all succeeded.
  - wsduplex-client-binary-frame-rejected-close-1003 and wsduplex-connect-denial-rejects-handshake hang in the same handshake poll.
Corroborating status line for that upgrade on this emulator (same run, same stack shape, 10 checks): `ConnectionError: websocket handshake failed: b'HTTP/1.1 404 Not Found'`.

Model facts used: smithy_query shape ProtocolType -> [WEBSOCKET, HTTP]; shape Api -> ApiEndpoint/RouteSelectionExpression docs; smithy_query digest (ApiGatewayManagementApi) -> PostToConnection POST /@connections/{ConnectionId}, restJson1.

## Steps to Reproduce
1. Boot the emulator and replay the apigatewayv2__dynamodb__lambda WebSocket checks of simparity-behavioral-ga-prod-v1.
2. Create a DynamoDB table and a Lambda; create_api(ProtocolType=WEBSOCKET, RouteSelectionExpression='$request.body.action'); create an AWS_PROXY integration pointing at the Lambda; create routes $connect, $disconnect and $default; create_deployment and create_stage('dev').
3. Send an RFC 6455 opening handshake to the stage's WebSocket URL and poll for a 101.
4. Observe that no 101 ever arrives (HTTP/1.1 404 Not Found), so $connect never runs, no conn# row appears in DynamoDB, no frame can be sent, and the checks hang to the 60 s cap.
