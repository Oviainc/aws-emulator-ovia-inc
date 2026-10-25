Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2.websocket-connect-mock-integration-rejected apigatewayv2.mock-integration-response-selected-by-status-code-pattern apigatewayv2.websocket-post-to-connection-delivers-then-gone-after-disconnect
Issue-Key: apigatewayv2.websocket-connect-mock-integration-rejected|run#16|AssertionError|assert result["status"] == 500, (
Issue-Key: apigatewayv2.mock-integration-response-selected-by-status-code-pattern|_poll_until#14|Timeout|time.sleep(interval_s)
Issue-Key: apigatewayv2.websocket-post-to-connection-delivers-then-gone-after-disconnect|_poll_until#18|Timeout|time.sleep(interval_s)
Issue-Commit: 5d61882ef55489b75c810fe19d4c281a0249999c

# apigatewayv2: the WEBSOCKET execute-api data plane still answers 404 UnknownService, so three more WebSocket checks cannot run (1 cause)

> _Note: this issue was identified against the code base at commit `5d61882ef55489b75c810fe19d4c281a0249999c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
WebSocket upgrade (execute-api data plane)

## Description
REOPENS 20261023T223918Z-apigatewayv2-a-deployed-websocket-stage-never-completes-the.md: its fix left the call refused. That issue is marked fixed, but a deployed WEBSOCKET stage still refuses the RFC 6455 upgrade with HTTP 404, and this replay grounds that in three checks the report marked neither ALREADY FILED nor OWNED. One cause.

=== Cause 1 of 1: the front door does not recognise a WEBSOCKET stage's announced wss host/path, so no handshake, route dispatch, MOCK reply or @connections delivery ever happens ===
AWS: recorded behavior only (execute-api data plane; not in the Smithy model). A WEBSOCKET API with routes, an integration, a deployment and a stage completes the upgrade on the stage's own wss:// URL with HTTP 101 Switching Protocols, then dispatches each frame by RouteSelectionExpression. The three checks here pin, on that live socket: (a) a $connect route bound to a MOCK integration FAILS the handshake with HTTP 500 — i.e. a refusal from the integration, not from the router; (b) a MOCK integration's reply is selected among three IntegrationResponses by matching the request-template-synthesized statusCode against '/2\\d\\d/', '/4\\d{2}/' and '$default', and the selected response's own ResponseTemplates renders the bytes the client reads back; (c) a connected client's connectionId is usable through the @connections management API (PostToConnection delivered to the live socket) and, once the client closes, PostToConnection / GetConnection / DeleteConnection each answer GoneException (HTTP 410).
Emulator: the upgrade request is answered `HTTP/1.1 404 Not Found` with `content-type: application/x-amz-json-1.1` and `x-amzn-errortype: UnknownService` — the router maps the announced execute-api WebSocket host/path to no service at all. The check that asserts the FIRST answer sees status 404 where AWS gives 500; the two checks that poll for a successful handshake never converge and are killed at the suite's 60 s cap.
Why: every WebSocket behaviour a client depends on is unobservable — connect-time authorization and rejection, content-based route and integration-response selection, MOCK response templating, server push through @connections, and the GoneException lifecycle after a close. An emulator that bookkeeps CreateApi/CreateRoute/CreateStage looks correct to control-plane assertions while serving nothing, and a 404/UnknownService is indistinguishable to a client from a wrong endpoint.
Fix: the execute-api WebSocket listener and the router entry that binds a WEBSOCKET stage's announced host/path to it; then $connect integration-type handling (a MOCK integration on $connect must fail the handshake with 500), frame dispatch by RouteSelectionExpression, IntegrationResponse selection by the synthesized statusCode, and the @connections connection registry including the 410 GoneException state after close. Related open issues 20261024T120951Z-apigatewayv2-the-execute-api-data-plane-never-serves.md (Cause 2) and 20261023T124140Z-apigatewayv2-websocket-duplex-data-plane-is-not-served.md describe the same surface but do not carry these three checks' replay keys.
Traces: apigatewayv2.websocket-connect-mock-integration-rejected (direct 404 observation), apigatewayv2.mock-integration-response-selected-by-status-code-pattern (hang), apigatewayv2.websocket-post-to-connection-delivers-then-gone-after-disconnect (hang).

## Evidence
Replay of suite simparity-behavioral-ga-prod-v1 against this checkout: 18/194 passed.

apigatewayv2.websocket-connect-mock-integration-rejected [mismatch]: AssertionError: a $connect route bound to a MOCK integration must fail the handshake with 500 on real AWS, got status 404 (b'HTTP/1.1 404 Not Found\r\ncontent-type: application/x-amz-json-1.1\r\nx-amzn-errortype: UnknownService\r\ncontent-length: 149').
Expected: HTTP 500 on the upgrade (and HTTP 101 for the healthy control legs).
Actual: HTTP 404 with x-amzn-errortype: UnknownService — no WebSocket listener behind the announced stage URL.

apigatewayv2.mock-integration-response-selected-by-status-code-pattern [mismatch]: Error: Check timed out after 60.0s — the check creates the API, three IntegrationResponses, a route response, a stage and a deployment, then polls for a live socket that never comes up.

apigatewayv2.websocket-post-to-connection-delivers-then-gone-after-disconnect [mismatch]: Error: Check timed out after 60.0s — same shape; no connectionId can exist because no handshake completes.

None of the three carried an ALREADY FILED or OWNED-by marker in the replay report.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. CreateApi(ProtocolType='WEBSOCKET', RouteSelectionExpression='$request.body.action'); CreateIntegration(IntegrationType='MOCK'); CreateRoute(RouteKey='$default', Target='integrations/<integ>'); CreateRouteResponse(RouteResponseKey='$default'); CreateStage(StageName='s'); CreateDeployment(StageName='s').
3. Open an RFC 6455 upgrade against the wss:// URL the emulator announces for that api id and stage. Expect HTTP 101; observe HTTP 404 with x-amzn-errortype: UnknownService.
4. Repeat with the $default route's MOCK integration bound to $connect instead: expect the handshake to fail with HTTP 500; observe 404.
5. With a connected socket, exercise ApiGatewayManagementApi PostToConnection / GetConnection / DeleteConnection, then close the client and expect GoneException (HTTP 410) from each.
6. Re-replay the three check ids in trace_ids.
