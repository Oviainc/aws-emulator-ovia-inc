Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2.websocket-two-way-route-needs-a-route-response-to-reach-the-client
Issue-Key: apigatewayv2.websocket-two-way-route-needs-a-route-response-to-reach-the-client|run#23|AssertionError|assert hello.get("marker") == marker, (
Issue-Commit: fce90a673e7875f918905a481813c28a01bba620

# apigatewayv2: a two-way WebSocket route answers the client with the gateway's 'Internal server error' frame instead of the integration's response

> _Note: this issue was identified against the code base at commit `fce90a673e7875f918905a481813c28a01bba620`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRouteResponse

## Description
ONE net-new root cause on the apigatewayv2 service token (the replay's other two apigatewayv2 failures are marked OWNED by 20261109T154749Z-apigatewayv2-5-net-new-root-causes-the-published.md; this one is owned by nobody).

=== Cause 1 of 1: a frame sent on a WebSocket route whose integration is wired and whose route response exists is answered with API Gateway's error frame {"connectionId":...,"message":"Internal server error","requestId":...} instead of the integration's own reply ===

AWS: the WebSocket data plane is not a modelled operation, so this is recorded behavior only (the control-plane shapes involved are apigatewayv2 CreateRoute/CreateIntegration/CreateRouteResponse/CreateDeployment). Recorded: once a route has an AWS_PROXY integration AND a route response (two-way route), the integration's returned body is relayed back to the sending client verbatim as a TEXT frame — the check asserts the frame it receives carries the integration's own marker (a value the check put in the handler), i.e. the handler's JSON reaches the client unchanged. The 'Internal server error' frame is what AWS sends only when the integration genuinely fails; its body is {"message":"Internal server error","connectionId":"<the connection id>","requestId":"<the request id>"} — 'Internal server error' is fixed text and the two ids are per-request values.

Emulator: the upgrade succeeds and a connection id is minted ('EzCyjTUad5uklZ0=' in this run), but the frame delivered to the client is the gateway's error frame: `{'connectionId': 'EzCyjTUad5uklZ0=', 'message': 'Internal server error', 'requestId': 'c240a8375a2d4464'}` where the integration's marker-bearing reply was expected. So the route's integration invocation (or the relay of its result through the route response) fails internally on every frame.

Why: a two-way WebSocket route is the only way a client gets a synchronous answer from its backend. A client always receives 'Internal server error' instead of its own payload, so no request/reply protocol over WebSockets can be developed or tested, and the error is indistinguishable from a real handler fault — the difference between 'needs a route response' and 'the backend is broken' is unobservable.

Fix: the apigatewayv2 WebSocket data-plane route dispatcher — route selection, the AWS_PROXY integration invocation for the selected route, and the route-response relay that must write the integration's body back on the originating socket — not the control-plane handlers, which accepted the wiring.

Traces: apigatewayv2.websocket-two-way-route-needs-a-route-response-to-reach-the-client.

## Evidence
Suite: simparity-behavioral-aws-v1. apigatewayv2 0/3 (0%), 3 failed; two are OWNED, this one is not.

[mismatch] apigatewayv2.websocket-two-way-route-needs-a-route-response-to-reach-the-client
  File "check_apigatewayv2_websocket_two_way_route_needs_a_route_response_to_reach_the_client.py", line 155, in run
    assert hello.get("marker") == marker, (...)
  AssertionError: the control frame must come from this check's integration (marker 'sim2real-behav-ws-67fb594127bb'), got {'connectionId': 'EzCyjTUad5uklZ0=', 'message': 'Internal server error', 'requestId': 'c240a8375a2d4464'}

Expected (recorded AWS): the TEXT frame the client reads back on the two-way route carries the integration's own JSON, including the marker the check planted in the handler.
Actual (emulator): the gateway's 'Internal server error' frame. The marker, the connection id and the request id are per-run values; the requirement is that the integration's body — not an error frame — reaches the client when the route has an integration and a route response.

## Steps to Reproduce
1. CreateApi(ProtocolType='WEBSOCKET', RouteSelectionExpression='$request.body.action'); create one AWS_PROXY Lambda integration whose handler returns a JSON body containing a known marker; create the $connect/$default routes and a two-way route (CreateRoute + CreateRouteResponse '$default'); grant lambda:InvokeFunction to apigateway.amazonaws.com; CreateDeployment + CreateStage.
2. Open an RFC 6455 handshake to the stage URL (it upgrades, 101) and send a text frame selecting the two-way route.
3. Observe the frame read back is {"connectionId":...,"message":"Internal server error","requestId":...} instead of the handler's marker-bearing body.
4. After fixing, re-replay apigatewayv2.websocket-two-way-route-needs-a-route-response-to-reach-the-client.
