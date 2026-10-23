Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewaymanagementapi.client-initiated-close-converges-to-gone-with-live-twin apigatewaymanagementapi.concurrent-post-to-connection-all-delivered apigatewaymanagementapi.connection-operations-after-delete-return-gone-rejected apigatewaymanagementapi.cross-api-connection-id-returns-gone apigatewaymanagementapi.delete-connection-forces-client-disconnect apigatewaymanagementapi.delete-connection-triggers-disconnect-route apigatewaymanagementapi.forbidden-without-manage-connections-permission-rejected apigatewaymanagementapi.get-connection-last-active-at-advances-on-traffic-not-on-reads apigatewaymanagementapi.get-connection-user-agent-matches-handshake-header apigatewaymanagementapi.multiple-connections-message-isolation apigatewaymanagementapi.reconnect-produces-distinct-connection-and-later-connected-at apigatewaymanagementapi.sequential-posts-preserve-message-order
Issue-Shape-Id: com.amazonaws.apigatewaymanagementapi#ApiGatewayManagementApi
Issue-Key: apigatewaymanagementapi.client-initiated-close-converges-to-gone-with-live-twin|_poll_until#6|Timeout|time.sleep(interval_s)
Issue-Key: apigatewaymanagementapi.concurrent-post-to-connection-all-delivered|_do_ws_handshake#3|ConnectionError|raise ConnectionError(f"websocket handshake failed: {status_line!r}")
Issue-Key: apigatewaymanagementapi.connection-operations-after-delete-return-gone-rejected|_do_ws_handshake#4|ConnectionError|raise ConnectionError(f"websocket handshake failed: {status_line!r}")
Issue-Key: apigatewaymanagementapi.cross-api-connection-id-returns-gone|_do_ws_handshake#4|ConnectionError|raise ConnectionError(f"websocket handshake failed: {status_line!r}")
Issue-Key: apigatewaymanagementapi.delete-connection-forces-client-disconnect|_do_ws_handshake#3|ConnectionError|raise ConnectionError(f"websocket handshake failed: {status_line!r}")
Issue-Key: apigatewaymanagementapi.delete-connection-triggers-disconnect-route|_do_ws_handshake#3|ConnectionError|raise ConnectionError(f"websocket handshake failed: {status_line!r}")
Issue-Key: apigatewaymanagementapi.forbidden-without-manage-connections-permission-rejected|_do_ws_handshake#3|ConnectionError|raise ConnectionError(f"websocket handshake failed: {status_line!r}")
Issue-Key: apigatewaymanagementapi.get-connection-last-active-at-advances-on-traffic-not-on-reads|_poll_until#6|Timeout|time.sleep(interval_s)
Issue-Key: apigatewaymanagementapi.get-connection-user-agent-matches-handshake-header|_do_ws_handshake#4|ConnectionError|raise ConnectionError(f"websocket handshake failed: {status_line!r}")
Issue-Key: apigatewaymanagementapi.multiple-connections-message-isolation|_do_ws_handshake#3|ConnectionError|raise ConnectionError(f"websocket handshake failed: {status_line!r}")
Issue-Key: apigatewaymanagementapi.reconnect-produces-distinct-connection-and-later-connected-at|_do_ws_handshake#4|ConnectionError|raise ConnectionError(f"websocket handshake failed: {status_line!r}")
Issue-Key: apigatewaymanagementapi.sequential-posts-preserve-message-order|_do_ws_handshake#3|ConnectionError|raise ConnectionError(f"websocket handshake failed: {status_line!r}")
Issue-Commit: 836bc7a08cca6ccbbb64b51e180fe3d49cf03502

# ApiGatewayManagementApi is untestable end-to-end: the WebSocket upgrade on a deployed apigatewayv2 WEBSOCKET stage answers HTTP 404, so no connection ever exists for @connections to act on (1 cause, 12 checks)

> _Note: this issue was identified against the code base at commit `836bc7a08cca6ccbbb64b51e180fe3d49cf03502`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetConnection

## Description
Cause 1 of 1: a client WebSocket handshake against a deployed WEBSOCKET-protocol API stage is answered with HTTP/1.1 404 Not Found instead of a 101 upgrade, so no connection is ever established and every @connections behaviour (GetConnection / PostToConnection / DeleteConnection against a live, a closed or a foreign connection) is unobservable. All 12 apigatewaymanagementapi checks in this suite fail on this single step. Failing check ids: apigatewaymanagementapi.concurrent-post-to-connection-all-delivered, apigatewaymanagementapi.connection-operations-after-delete-return-gone-rejected, apigatewaymanagementapi.cross-api-connection-id-returns-gone, apigatewaymanagementapi.delete-connection-forces-client-disconnect, apigatewaymanagementapi.delete-connection-triggers-disconnect-route, apigatewaymanagementapi.forbidden-without-manage-connections-permission-rejected, apigatewaymanagementapi.get-connection-user-agent-matches-handshake-header, apigatewaymanagementapi.multiple-connections-message-isolation, apigatewaymanagementapi.reconnect-produces-distinct-connection-and-later-connected-at, apigatewaymanagementapi.sequential-posts-preserve-message-order (these raise ConnectionError on the 404 immediately), plus apigatewaymanagementapi.client-initiated-close-converges-to-gone-with-live-twin and apigatewaymanagementapi.get-connection-last-active-at-advances-on-traffic-not-on-reads (same step, but their helper polls for a 101 for up to 120 s and is cut by the 60 s harness cap). The two symptom shapes differ only in whether the check's handshake helper raises on the first non-101 or polls for one; the emulator response is identical.

AWS: recorded/documented behavior only — the opening handshake is the execute-api data plane and is not a modeled operation, so the Smithy model does not describe it; the model does establish the surface it belongs to. Smithy (apigatewayv2 2018-11-29): ProtocolType is an enum with values WEBSOCKET and HTTP; Api.ApiEndpoint doc: "The URI of the API, of the form {api-id}.execute-api.{region}.amazonaws.com. The stage name is typically appended to this URI to form a complete path to a deployed API stage"; Api.RouteSelectionExpression is required and for WebSocket APIs selects the route from the frame (here $request.body.action). Smithy (ApiGatewayManagementApi 2018-11-29): protocol restJson1, endpointPrefix/sigv4Name execute-api, operations GET /@connections/{ConnectionId} (GetConnection -> GetConnectionResponse{ConnectedAt, LastActiveAt, Identity}), POST /@connections/{ConnectionId} (PostToConnection), DELETE /@connections/{ConnectionId} (code 204), common errors ForbiddenException / GoneException (@httpError 410, client, "The connection with the provided id no longer exists.") / LimitExceededException. On real AWS the client's upgrade request to the deployed stage URL is answered HTTP/1.1 101 Switching Protocols with a Sec-WebSocket-Accept derived from the client key, the $connect route integration is invoked, and the server-minted connection id is what the @connections operations then address.

Emulator: the upgrade request to the deployed stage returns the status line b'HTTP/1.1 404 Not Found' — no 101, no Sec-WebSocket-Accept, no socket. Replay output for the ten [error] checks is identically `ConnectionError: websocket handshake failed: b'HTTP/1.1 404 Not Found'`; the two [mismatch] checks report `Check timed out after 60.0s` while polling the same handshake for a 101.

Why: a client relying on AWS behavior cannot open a WebSocket at all, so the entire bidirectional serverless surface is dead on this emulator. $connect/$default/$disconnect integrations never run, the connection store stays empty, and because no connection id can ever be minted, a server-push application cannot be exercised even in the negative direction: GoneException-on-a-stale-connection, 403 Forbidden without execute-api:ManageConnections, per-connection message isolation and ConnectedAt/LastActiveAt semantics are all unreachable. A developer testing against the emulator sees a plain 404 on wss:// and cannot distinguish a missing data plane from a misconfigured stage.

Fix: the execute-api data-plane front door / request router and the apigatewayv2 WebSocket session layer. The router must recognise an HTTP/1.1 request carrying Connection: Upgrade / Upgrade: websocket / Sec-WebSocket-Key on a deployed WEBSOCKET-protocol API's stage path and complete the RFC 6455 handshake (101 + Sec-WebSocket-Accept) rather than falling through to the generic 404 path; a session layer must then mint a connection id, dispatch $connect / $disconnect / the route-selection-expression route to their integrations, and register the connection in the store that ApiGatewayManagementApi's GetConnection/PostToConnection/DeleteConnection handlers read. This is a capability gap, not an error-shape bug.

Related open issues, neither of which covers this: "ApiGatewayManagementApi @connections requests all answer 500 AmbiguousService instead of the modeled error envelope" (20261023T144653Z-...) is the control-path router ambiguity for /@connections requests — these checks never reach a @connections call, they fail on the socket upgrade; "API Gateway data plane is dead: 30 checks driving a deployed stage hang to the 60 s cap, the invoke URL answers 404 UnknownService" (20261023T210711Z-apigateway-...) is API Gateway v1 (REST), a different service.

Traces: the 12 failing check ids listed above.

## Evidence
Suite: simparity-behavioral-ga-prod-v1, apigatewaymanagementapi 0/12 (0%).

Expected (real AWS): the upgrade request to the deployed WEBSOCKET stage URL is answered HTTP/1.1 101 Switching Protocols with a valid Sec-WebSocket-Accept; $connect runs; the server-minted connectionId is then addressable at GET/POST/DELETE /@connections/{ConnectionId}, with HTTP 410 GoneException once the connection is gone and HTTP 403 ForbiddenException without execute-api:ManageConnections.

Actual (emulator), 10 checks, identical detail:
  File "check_apigatewaymanagementapi_concurrent_post_to_connection_all_delivered.py", line 399, in _open_ws_connection
    return _do_ws_handshake(wss_url, extra_headers)
  File "...", line 380, in _do_ws_handshake
    raise ConnectionError(f"websocket handshake failed: {status_line!r}")
  ConnectionError: websocket handshake failed: b'HTTP/1.1 404 Not Found'
Same line and same 404 status line in connection-operations-after-delete-return-gone-rejected, cross-api-connection-id-returns-gone, delete-connection-forces-client-disconnect, delete-connection-triggers-disconnect-route, forbidden-without-manage-connections-permission-rejected, get-connection-user-agent-matches-handshake-header, multiple-connections-message-isolation, reconnect-produces-distinct-connection-and-later-connected-at, sequential-posts-preserve-message-order.

Actual (emulator), 2 checks: `Check timed out after 60.0s`. Both block in the helper `_open_and_identify`, whose first step is `_poll_until(lambda: _ws_upgraded(ctx, api_id, stage), lambda s: s is not None, timeout_s=120.0, interval_s=4.0, description="a 101 WebSocket upgrade against API {api_id} stage {stage}")` — a 120 s poll for a 101 that the 60 s harness cap cuts short. The control plane up to that point (sts, iam role, lambda create/Active, create_api/create_integration/create_route/create_deployment/create_stage) completed, so the divergence is isolated to the upgrade.

Model facts used: smithy_query digest (ApiGatewayManagementApi) -> restJson1, 3 ops, common_errors [ForbiddenException, GoneException, LimitExceededException]; smithy_query shape GoneException -> errorKind client, httpError 410; shape GetConnectionResponse -> ConnectedAt/LastActiveAt/Identity; smithy_query shape ProtocolType (apigatewayv2) -> [WEBSOCKET, HTTP]; shape Api -> ApiEndpoint/RouteSelectionExpression docs.

## Steps to Reproduce
1. Boot the emulator and replay the apigatewaymanagementapi checks of simparity-behavioral-ga-prod-v1.
2. Create a Lambda and an apigatewayv2 API with ProtocolType=WEBSOCKET and RouteSelectionExpression=$request.body.action; create an AWS_PROXY integration and the $connect/$disconnect/$default routes; create a deployment and a stage.
3. Open a TCP (or TLS) connection to the stage's WebSocket URL and send an RFC 6455 opening handshake (GET /{stage} with Connection: Upgrade, Upgrade: websocket, Sec-WebSocket-Key, Sec-WebSocket-Version: 13).
4. Observe the status line HTTP/1.1 404 Not Found with no Sec-WebSocket-Accept, instead of HTTP/1.1 101 Switching Protocols.
5. Consequently no connection id is minted, so GetConnection/PostToConnection/DeleteConnection cannot be exercised against a live, closed or foreign connection at all.
