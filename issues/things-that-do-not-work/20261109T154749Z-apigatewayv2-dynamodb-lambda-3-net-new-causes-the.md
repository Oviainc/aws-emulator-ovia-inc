Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-body-action-selects-custom-route-else-default apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-connect-request-authorizer-deny-refuses-before-integration apigatewayv2__dynamodb__lambda.websocket-api-connect-message-post-back-disconnect apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-selection-expression-update-redirects-dispatch apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-unmatched-action-without-default-route-not-delivered apigatewayv2__dynamodb__lambda.e2e-serverless-visitor-ip-capture-lifecycle
Issue-Key: apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-body-action-selects-custom-route-else-default|_poll_until#21|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Key: apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-connect-request-authorizer-deny-refuses-before-integration|_poll_until#28|AssertionError|raise AssertionError(
Issue-Key: apigatewayv2__dynamodb__lambda.websocket-api-connect-message-post-back-disconnect|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Key: apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-selection-expression-update-redirects-dispatch|run#24|AssertionError|assert reply.get("marker") == markers["alpha"] and reply.get("routeKey") == "alpha", (
Issue-Key: apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-unmatched-action-without-default-route-not-delivered|run#27|AssertionError|assert reply.get("marker") == marker and reply.get("routeKey") == "sendmessage", (
Issue-Key: apigatewayv2__dynamodb__lambda.e2e-serverless-visitor-ip-capture-lifecycle|phase#6|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Commit: 39b863d101414613449d16e44f0b23fd775a8d87

# apigatewayv2__dynamodb__lambda: 3 net-new causes — the WebSocket handshake never upgrades within the run budget, upgraded frames answer 'Internal server error' instead of the route integration's reply, and the published ApiEndpoint is an AWS execute-api host

> _Note: this issue was identified against the code base at commit `39b863d101414613449d16e44f0b23fd775a8d87`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApi

## Description
Three net-new root causes behind the surviving WebSocket/serverless checks of this service triple. All three must be closed.

Cause 1 of 3: a deployed WEBSOCKET stage never completes the RFC 6455 upgrade — the handshake produces no 101 within the check's budget (3 checks).
AWS: recorded behavior only. A correct handshake against a deployed WEBSOCKET stage is answered 'HTTP/1.1 101 Switching Protocols' with Sec-WebSocket-Accept; $connect decides 101 vs 403/401. The connection is then usable for route dispatch and @connections pushes.
Emulator: no upgrade is produced at all — "WebSocket handshake against stage dev of <api-id>: the run budget ended first; not graded", "an Allow-token WebSocket handshake against stage dev of <api-id> not met within 150.0s. Last result: None", and one check exhausts the whole 180 s budget. Nothing is served on the handshake path.
Why: no WebSocket client can connect, so an estate whose connect/disconnect handlers persist rows in DynamoDB and whose routes reply to the client cannot run end to end.
Fix: the WEBSOCKET execute-api front door (the upgrade seam that hands the connection to the api's socket after running $connect) in the apigatewayv2 data-plane handler.
Related but not covering: 20261107T173512Z-apigatewayv2-dynamodb-lambda-a-deployed-websocket-api-stage.md and 20261103T042714Z-apigatewayv2-dynamodb-lambda-a-deployed-api-gateway-v2-websocket-stage-never.md describe the same front door for this service triple but own other replay keys.
Traces: apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-body-action-selects-custom-route-else-default, apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-connect-request-authorizer-deny-refuses-before-integration, apigatewayv2__dynamodb__lambda.websocket-api-connect-message-post-back-disconnect.

Cause 2 of 3: where a connection does upgrade, every inbound frame is answered with the gateway's own error frame instead of the selected route integration's reply (2 checks).
AWS: recorded behavior only. Under RouteSelectionExpression '$request.body.action', a frame whose action matches a route is handed to that route's integration and the client receives the integration's own payload (the marker the backend echoes); "Internal server error" is reserved for a genuine integration failure.
Emulator: the client receives {"connectionId":"<id>","message":"Internal server error","requestId":"<id>"} for a healthy Lambda integration, so neither the selection-expression update nor the unmatched-action-without-$default contract can be graded. Fixed text "Internal server error"; connectionId/requestId are per-run values.
Why: WebSocket applications see a backend error for a request AWS answers successfully, and route selection cannot be relied on at all.
Fix: the WEBSOCKET route-dispatch path of the apigatewayv2 data-plane handler (route-selection-expression evaluation and integration invocation), which must deliver the integration's reply and only frame real failures.
Traces: apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-selection-expression-update-redirects-dispatch, apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-unmatched-action-without-default-route-not-delivered.

Cause 3 of 3: the HTTP API's published ApiEndpoint is an AWS public execute-api hostname the emulator does not serve, so the serverless visitor-capture lifecycle cannot be driven (1 check).
AWS: Smithy (service-com-amazonaws-apigatewayv2-apigatewayv2): CreateApi/GetApi output member ApiEndpoint, documented as "The URI of the API, of the form {api-id}.execute-api.{region}.amazonaws.com" — the address the API is served on.
Emulator: "https://<api-id>.execute-api.<region>.amazonaws.com" is published and refused by replay with PublishedUrlRejected ("its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>"); the phase aborts before any request.
Why: the documented discovery path sends the client off the emulator, so the Lambda+DynamoDB lifecycle behind the API is never exercised.
Fix: the ApiEndpoint construction in the apigatewayv2 crate's CreateApi/GetApi handlers plus the execute-api front-door listener that must serve it.
Traces: apigatewayv2__dynamodb__lambda.e2e-serverless-visitor-ip-capture-lifecycle.

## Evidence
Cause 1 — expected: 'HTTP/1.1 101 Switching Protocols' on wss://<own host>:<port>/<stage>. Actual: TimeoutError "WebSocket handshake against stage dev of <api-id>: the run budget ended first; not graded"; AssertionError "an Allow-token WebSocket handshake against stage dev of <api-id> not met within 150.0s. Last result: None"; "Check timed out after 180.0s".
Cause 2 — expected: the frame reaches the alpha/sendmessage integration and the client receives its marker (e.g. 'mk-alpha-<id>'). Actual: {'connectionId': '<id>', 'message': 'Internal server error', 'requestId': '<id>'}.
Cause 3 — expected: requests against the published ApiEndpoint reach the Lambda and the visitor row lands in DynamoDB. Actual: AssertionError "[phase 2/4 solve] endpoint_url was given https://<api-id>.execute-api.us-west-2.amazonaws.com (published by the emulator or built by the check): its host is not one of the emulator's own hosts".

## Steps to Reproduce
1. CreateApi(ProtocolType='WEBSOCKET', RouteSelectionExpression='$request.body.action') with $connect/$default/custom routes bound to Lambdas, AutoDeploy stage; open a handshake — no 101 is produced. Replay apigatewayv2__dynamodb__lambda.websocket-api-connect-message-post-back-disconnect.
2. On a connection that does upgrade, send {"action":"alpha",...} — the reply frame is the gateway's "Internal server error". Replay apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-selection-expression-update-redirects-dispatch.
3. CreateApi(ProtocolType='HTTP'), read ApiEndpoint and POST to it — the host is not served. Replay apigatewayv2__dynamodb__lambda.e2e-serverless-visitor-ip-capture-lifecycle.
