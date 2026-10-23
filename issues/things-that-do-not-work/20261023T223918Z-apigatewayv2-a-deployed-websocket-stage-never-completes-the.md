Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2.websocket-broadcast-to-multiple-connections
Issue-Key: apigatewayv2.websocket-broadcast-to-multiple-connections|_poll_until#20|Timeout|time.sleep(interval_s)
Issue-Commit: 836bc7a08cca6ccbbb64b51e180fe3d49cf03502

# apigatewayv2: a deployed WEBSOCKET stage never completes the RFC 6455 upgrade, so the broadcast-to-multiple-connections check hangs to the 60 s cap (1 cause)

> _Note: this issue was identified against the code base at commit `836bc7a08cca6ccbbb64b51e180fe3d49cf03502`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
$connect (WebSocket data plane)

## Description
Cause 1 of 1: after create_api(ProtocolType=WEBSOCKET) / create_integration / create_route / create_stage / create_deployment all succeed and the deployment leaves PENDING, the stage's WebSocket URL never answers the opening handshake with 101, so the check cannot open even one socket and hangs to the harness cap. Failing check id: apigatewayv2.websocket-broadcast-to-multiple-connections.

AWS: recorded/documented behavior only — the opening handshake is the execute-api data plane and is not a modeled operation. Smithy (apigatewayv2 2018-11-29) establishes the surface: ProtocolType enum [WEBSOCKET, HTTP]; Api.ApiEndpoint doc "The URI of the API, of the form {api-id}.execute-api.{region}.amazonaws.com. The stage name is typically appended to this URI to form a complete path to a deployed API stage"; Api.RouteSelectionExpression required, selecting the route from the frame for WebSocket APIs. On real AWS a client's upgrade request to wss://{api-id}.execute-api.{region}.amazonaws.com/{stage} is answered HTTP/1.1 101 Switching Protocols with a Sec-WebSocket-Accept derived from the client's Sec-WebSocket-Key, after which $connect runs and the socket carries frames.

Emulator: the handshake is never upgraded. The check's helper `_ws_connect` returns the status as a RESULT and is polled for status == 101 for up to 120 s; the 60 s harness cap fires first, so the check reports `Check timed out after 60.0s`. The sibling apigatewaymanagementapi checks in this same suite, driving the identical stack, read the status line directly and report it: b'HTTP/1.1 404 Not Found'.

Why: a client relying on AWS behavior cannot connect to a WebSocket API it just deployed. Every per-connection behaviour built on top — broadcast fan-out to N connections, per-connection isolation, $disconnect on close — is unreachable, and the failure surfaces as a hang or a bare 404 rather than as anything a client can act on.

Fix: the execute-api data-plane front door / request router plus the apigatewayv2 WebSocket session layer: detect Connection: Upgrade / Upgrade: websocket / Sec-WebSocket-Key on a deployed WEBSOCKET API's stage path, complete the RFC 6455 handshake (101 + Sec-WebSocket-Accept) instead of falling through to the generic 404, then mint a connection id and dispatch $connect / $disconnect / the route-selection-expression route to their integrations.

Related open issues, neither of which covers this: "API Gateway v2: CreateStage and CreateDeployment are unimplemented scaffold stubs, so no HTTP or WebSocket API can be deployed (2 causes)" (20261023T113357Z-...) — those calls now succeed in this replay and the check gets past deploy; "API Gateway data plane is dead ... 404 UnknownService" (20261023T210711Z-apigateway-...) is API Gateway v1 (REST), a different service. The suite's other apigatewayv2 failure (websocket-post-to-connection-delivers-then-gone-after-disconnect) is marked ALREADY FILED against the unimplemented CreateIntegrationResponse stub and is not included here.

Traces: apigatewayv2.websocket-broadcast-to-multiple-connections.

## Evidence
Suite: simparity-behavioral-ga-prod-v1, apigatewayv2 0/2 (0%) — 1 ALREADY FILED (CreateIntegrationResponse stub), 1 net-new below.

Expected (real AWS): the upgrade request to the deployed WEBSOCKET stage URL is answered HTTP/1.1 101 Switching Protocols with a valid Sec-WebSocket-Accept; the socket then carries frames and several concurrent connections each receive a broadcast push.

Actual (emulator): `[mismatch] apigatewayv2.websocket-broadcast-to-multiple-connections — Check timed out after 60.0s`. The check reaches the handshake (the deployment poll `the WebSocket deployment to leave PENDING` completed) and then blocks in:
  _poll_until(lambda: _ws_connect(url), lambda r: r["status"] == 101, timeout_s=120.0, interval_s=3.0, description=f"a 101 WebSocket upgrade at {url}")
where `_ws_connect` parses the response status line and, for any status != 101, closes the socket and returns {"status": status, "head": ...}. Corroborating evidence for the status actually returned, same emulator, same stack, 10 checks: `ConnectionError: websocket handshake failed: b'HTTP/1.1 404 Not Found'` (apigatewaymanagementapi.* in this run).

Model facts used: smithy_query shape ProtocolType -> enum [WEBSOCKET, HTTP]; smithy_query shape Api -> ApiEndpoint doc ({api-id}.execute-api.{region}.amazonaws.com + stage appended), RouteSelectionExpression required.

## Steps to Reproduce
1. Boot the emulator and replay apigatewayv2.websocket-broadcast-to-multiple-connections from simparity-behavioral-ga-prod-v1.
2. create_api(ProtocolType=WEBSOCKET, RouteSelectionExpression='$request.body.action'), create an AWS_PROXY integration, create the routes, create_stage and create_deployment, and wait for the deployment to leave PENDING.
3. Send an RFC 6455 opening handshake to the stage's WebSocket URL (GET /{stage} with Connection: Upgrade, Upgrade: websocket, Sec-WebSocket-Key, Sec-WebSocket-Version: 13).
4. Observe that no 101 is ever returned (the status line is HTTP/1.1 404 Not Found), so the 101 poll never converges and the check hangs to the 60 s cap.
