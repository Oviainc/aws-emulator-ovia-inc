Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-body-action-selects-custom-route-else-default apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-selection-expression-update-redirects-dispatch apigatewayv2__dynamodb__lambda.e2e-serverless-visitor-ip-capture-lifecycle
Issue-Key: apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-body-action-selects-custom-route-else-default|_poll_until#20|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Key: apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-selection-expression-update-redirects-dispatch|_poll_until#18|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Key: apigatewayv2__dynamodb__lambda.e2e-serverless-visitor-ip-capture-lifecycle|phase#6|_PhaseError|raise _PhaseError(
Issue-Commit: c17c6e54c038586d9d6328bb68b9d1b3d6bcf360

# apigatewayv2__dynamodb__lambda: the deployed execute-api data plane serves nothing — the WebSocket RFC 6455 upgrade never completes and an HTTP $default route never answers 200 (2 causes)

> _Note: this issue was identified against the code base at commit `c17c6e54c038586d9d6328bb68b9d1b3d6bcf360`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InvokeApi (execute-api data plane)

## Description
Two distinct data-plane causes behind one apigatewayv2 surface. Related open issues describe the same family but carry none of these checks' replay keys: 20261024T120951Z-apigatewayv2-dynamodb-lambda-websocket-route-selection-and-serverless.md, 20261025T023548Z-apigatewayv2-websocket-stage-never-completes-the-rfc-6455.md, 20261027T053233Z-apigatewayv2-an-http-api-aws-proxy-route-still.md.

Cause 1 of 2 — a deployed WEBSOCKET stage never completes the RFC 6455 upgrade, so route selection is unobservable.
AWS: for a WEBSOCKET ApiGatewayV2 API with a deployed stage, a client opening wss://<api-id>.execute-api.<region>.amazonaws.com/<stage> with the RFC 6455 handshake headers (Upgrade: websocket, Connection: Upgrade, Sec-WebSocket-Key, Sec-WebSocket-Version: 13) receives 101 Switching Protocols with a correct Sec-WebSocket-Accept; thereafter frames are dispatched per the API's RouteSelectionExpression (Smithy: CreateApiRequest.RouteSelectionExpression, required for ProtocolType WEBSOCKET, typically $request.body.action), falling back to the $default route when no custom route key matches, and UpdateApi on the selection expression re-targets subsequent dispatch. Recorded behavior: the handshake is expected to succeed within the wait budget.
Emulator: the handshake never completes for the whole budget. Harness output: TimeoutError: "WebSocket handshake against stage dev of <api-id>: the run budget ended first; not graded" for both checks. No 101 is ever produced, so neither $request.body.action selection, the $default fallback, nor an UpdateApi-driven re-selection can be exercised.
Why: every WebSocket client (a connect, a frame, a route handler, an @connections push) is dead on arrival; a developer gets a control plane that reports a deployed stage with a working route table and a data plane that refuses every connection.
Fix: the execute-api data-plane listener/router for ProtocolType WEBSOCKET in the apigatewayv2 crate — the handshake path that must answer 101 and the frame-dispatch path that must evaluate RouteSelectionExpression against the frame body.
Traces: apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-body-action-selects-custom-route-else-default, apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-selection-expression-update-redirects-dispatch

Cause 2 of 2 — an HTTP API $default stage route backed by an AWS_PROXY Lambda integration never answers 200.
AWS: a GET to a deployed HTTP API's invoke URL path that matches a route with an AWS_PROXY integration invokes the function and returns the function's statusCode — 200 for the handler under test — and the handler's DynamoDB write is then readable. Recorded behavior: the GET is expected to return 200 within the wait budget.
Emulator: the GET never returns 200 for the whole budget. Harness output: _PhaseError: "[phase 2/4 solve] TimeoutError: GET https://<host>/_aws/execute-api/<api-id>/$default/capture returns 200: the run budget ended first; not graded". The route and stage exist in the control plane; the data plane never produces the 200.
Why: the whole serverless request path (route -> AWS_PROXY integration -> function -> DynamoDB) is unusable; an end-to-end lifecycle check can reach no verdict and a real client gets no response from an API the control plane reports as deployed.
Fix: the execute-api HTTP-API routing/integration component in the apigatewayv2 crate — route matching for the $default stage and invocation of the AWS_PROXY integration target.
Traces: apigatewayv2__dynamodb__lambda.e2e-serverless-visitor-ip-capture-lifecycle

## Evidence
Cause 1 — expected: HTTP 101 Switching Protocols with Sec-WebSocket-Accept derived from the client's Sec-WebSocket-Key, then frames routed by RouteSelectionExpression ($request.body.action) with $default as fallback.
Actual: no handshake ever completes.
  TimeoutError: WebSocket handshake against stage dev of tyjpwmhaaa: the run budget ended first; not graded
(reported identically by both wsroute checks).

Cause 2 — expected: GET <invoke-url>/capture -> 200 from the AWS_PROXY Lambda integration, with the captured visitor record readable from DynamoDB.
Actual:
  _PhaseError: [phase 2/4 solve] TimeoutError: GET https://127.0.0.1:38919/_aws/execute-api/tyjpwmhaaa/$default/capture returns 200: the run budget ended first; not graded
The host/port are per-run values and are not part of the requirement; the requirement is that the matched route on the deployed stage answers 200.

## Steps to Reproduce
Cause 1:
1. CreateApi(ProtocolType=WEBSOCKET, RouteSelectionExpression="$request.body.action").
2. CreateIntegration(AWS_PROXY -> a Lambda function), CreateRoute for $connect, $default and a custom route key, CreateDeployment, CreateStage(StageName=dev).
3. Open a WebSocket to wss://<api-id>.execute-api.<region>/dev with the RFC 6455 handshake headers.
Observed: the handshake never completes. Expected: 101 Switching Protocols.

Cause 2:
1. CreateApi(ProtocolType=HTTP) with an auto-deployed $default stage.
2. CreateIntegration(AWS_PROXY -> a Lambda function that writes the caller IP to DynamoDB), CreateRoute(RouteKey="GET /capture").
3. GET <invoke-url>/capture.
Observed: never 200. Expected: 200 with the handler's body, and the record present in the table.
