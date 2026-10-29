Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2.websocket-two-way-route-needs-a-route-response-to-reach-the-client
Issue-Key: apigatewayv2.websocket-two-way-route-needs-a-route-response-to-reach-the-client|_poll_until#14|Timeout|time.sleep(interval_s)
Issue-Commit: c8d80ea15f1454d07f388eb2b932b3a5430349f4

# apigatewayv2: a WEBSOCKET two-way route's RouteResponse semantics are unobservable — the deployed stage never completes the RFC 6455 upgrade (1 cause)

> _Note: this issue was identified against the code base at commit `c8d80ea15f1454d07f388eb2b932b3a5430349f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRouteResponse

## Description
Cause 1 of 1: a client WebSocket opening handshake against a deployed WEBSOCKET-protocol apigatewayv2 stage never reaches 101 Switching Protocols, so the route-response contract of a two-way route cannot be exercised at all.

AWS: recorded behavior only for the handshake itself — the opening upgrade is the execute-api data plane and is not a modelled operation. The modelled surface around it is apigatewayv2 2018-11-29: an Api with ProtocolType=WEBSOCKET and a required RouteSelectionExpression ($request.body.action here), Route with RouteResponseSelectionExpression (what declares a route two-way), CreateRouteResponse / GetRouteResponses, Stage with AutoDeploy, and AWS_PROXY integrations. Recorded: the upgrade request (GET /<stage> with Connection: Upgrade, Upgrade: websocket, Sec-WebSocket-Key, Sec-WebSocket-Version: 13) to the deployed stage is answered HTTP/1.1 101 Switching Protocols with a Sec-WebSocket-Accept derived from the client key; $connect runs, a connection id is minted, and a frame whose action selects a two-way route is delivered to the integration. A route declared two-way but with NO RouteResponse runs its integration yet sends nothing back on the socket; after CreateRouteResponse the identical frame is answered as a text frame. (Docs: apigateway-websocket-api-integration-responses.)
Emulator: the check never obtains a socket. It blocks in `_poll_until(lambda: _ws_upgraded(ctx, api_id, 'dev'), lambda s: s is not None, timeout_s=150.0, interval_s=5.0, description="a 101 WebSocket upgrade against stage dev")`, where `_ws_upgraded` only returns a socket when the status line contains ' 101 '; the harness cuts it at the 60 s cap and reports "Check timed out after 60.0s". Every control-plane step before it (create_api, create_integration, four create_route calls including the two-way `echo` route, add-permission, create_stage AutoDeploy=True, plus the Lambda and role) succeeded, so the divergence is isolated to the upgrade on the deployed stage.
Why: a client relying on AWS behavior cannot open a WebSocket to this API, so the whole two-way/one-way distinction is invisible: an application that forgets CreateRouteResponse gets silence from AWS (a real and commonly-hit production trap) and an application that adds one gets its integration's response back — neither is reproducible on the emulator, and out-of-band PostToConnection pushes from the integration cannot be observed either because there is no socket to receive them.
Fix: the execute-api data-plane front door / request router plus the apigatewayv2 WebSocket session layer — recognise an RFC 6455 upgrade on a deployed WEBSOCKET API's stage path and answer 101 + Sec-WebSocket-Accept instead of falling through to the generic 404/non-101 path, then mint a connection id, dispatch $connect/$disconnect and the RouteSelectionExpression-selected route, and gate the integration response on the presence of a RouteResponse for that route.
Related: the open issues 20261024T120951Z-apigatewayv2-the-execute-api-data-plane-never-serves.md and 20261025T023548Z-apigatewayv2-websocket-stage-never-completes-the-rfc-6455.md own the suite's two other apigatewayv2 WebSocket checks and do not carry this check's replay key; a fix for them should fix this check too, but neither covers it today.
Traces: apigatewayv2.websocket-two-way-route-needs-a-route-response-to-reach-the-client.

## Evidence
Suite: simparity-behavioral-aws-v1, apigatewayv2 0/3 (0%).

[mismatch] apigatewayv2.websocket-two-way-route-needs-a-route-response-to-reach-the-client
Error: Check timed out after 60.0s

Expected (real AWS): the upgrade to the deployed stage 'dev' answers HTTP/1.1 101 Switching Protocols; a {"action":"whoami"} frame triggers the $default integration which PostToConnection's the caller its own connectionId back as a text frame (opcode 0x1) carrying this run's marker; GetRoute reports RouteResponseSelectionExpression='$default' on the echo route and GetRouteResponses returns no items; an {"action":"echo"} frame then produces SILENCE on the socket while the integration runs; after CreateRouteResponse the identical frame arrives as a text frame.

Actual (emulator): no socket is ever obtained. The check's first data-plane step is
  sock = _poll_until(lambda: _ws_upgraded(ctx, api_id, _STAGE), lambda s: s is not None, timeout_s=150.0, interval_s=5.0, description=f"a 101 WebSocket upgrade against stage {_STAGE}")
and `_ws_upgraded` returns None for every non-101 status line, so the 150 s poll is truncated by the 60 s harness cap. The sibling apigatewaymanagementapi checks in this same run show what the upgrade answers on the wire: `ConnectionError: websocket handshake failed: b'HTTP/1.1 404 Not Found'`.

## Steps to Reproduce
1. Boot the emulator and replay apigatewayv2.websocket-two-way-route-needs-a-route-response-to-reach-the-client from simparity-behavioral-aws-v1.
2. Create a Lambda (with execute-api:ManageConnections in its role) and an apigatewayv2 API with ProtocolType=WEBSOCKET, RouteSelectionExpression=$request.body.action.
3. Create one AWS_PROXY integration pointing at the Lambda; create routes $connect, $disconnect, $default, and an `echo` route with RouteResponseSelectionExpression='$default' and NO route response; add the lambda invoke permission; create stage 'dev' with AutoDeploy=True.
4. Open a TCP/TLS connection to the stage URL and send an RFC 6455 opening handshake (GET /dev with Connection: Upgrade, Upgrade: websocket, Sec-WebSocket-Key, Sec-WebSocket-Version: 13).
5. Observe that the status line is never HTTP/1.1 101 Switching Protocols (the emulator answers 404 Not Found on this path), so the poll for a 101 runs out and the check is cut at the 60 s cap.
6. Consequently the two-way-route contract — silence with no RouteResponse, a text frame after CreateRouteResponse — is never reached.
