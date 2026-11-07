Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2.websocket-connect-mock-integration-rejected apigatewayv2.mock-integration-response-selected-by-status-code-pattern apigatewayv2.websocket-post-to-connection-delivers-then-gone-after-disconnect
Issue-Key: apigatewayv2.websocket-connect-mock-integration-rejected|_poll_until#30|Timeout|time.sleep(interval_s)
Issue-Key: apigatewayv2.mock-integration-response-selected-by-status-code-pattern|_poll_until#14|Timeout|time.sleep(interval_s)
Issue-Key: apigatewayv2.websocket-post-to-connection-delivers-then-gone-after-disconnect|_poll_until#18|Timeout|time.sleep(interval_s)
Issue-Commit: 5d61882ef55489b75c810fe19d4c281a0249999c

# apigatewayv2: the emulator's front door cannot switch a WebSocket handshake to the api's own socket, so a client that reaches the SERVICE endpoint still gets no 101 (1 cause)

> _Split out of `issues/things-that-now-work/20261025T030504Z-apigatewayv2-the-websocket-execute-api-data-plane-still.md`. That report's cause is now SERVED — a WEBSOCKET api is given a socket of its own, which completes the RFC 6455 handshake, dispatches each frame by `RouteSelectionExpression`, selects a MOCK integration's reply among its `IntegrationResponses` by the synthesized status, and registers the connection in the `@connections` registry so a `PostToConnection` is delivered over that socket and every later call is `GoneException` once the client closes (`services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/service/websocket/transport.rs`, driven end to end over a real connection in `tests/all/websocket_transport_data_plane.rs`). This report carries the replay keys that still do NOT pass, re-filed here so they are not recorded as working, and what the fix established about them._

## Operation
WebSocket upgrade (execute-api data plane)

## Description
One cause, and it is a CAPABILITY OF THE CORE rather than a wrong answer from a service.

A WEBSOCKET api is now reached, in full, at the address its `ApiEndpoint` announces: the socket the core opened for that api (`engine.listeners().tcp_plaintext`), which answers `101 Switching Protocols` and carries frames. The three checks below never dial that address. Each resolves the announced URI onto the ENDPOINT UNDER TEST — the emulator's one front door — and sends the opening handshake there, carrying the api id in the rewritten path (`/_aws/execute-api/{api-id}/{stage}`). That is observable in the replay: before the path form was read, the front door answered `404` `UnknownService`; with it read, the same request is answered from the api's records (`500` for the `$connect` MOCK the first check asserts, which that check's `run#16` assertion now passes). But the front door cannot answer `101`: it serves one BUFFERED request per call, so the two checks that poll for a live socket still spin to the suite's 60 s cap.

Why it cannot be closed from `services/` as the tree stands — the client reaches the one buffered front door, and nothing in the vendored path can hand a service the connection's byte stream:
* `vendor/smithy-server-runtime/src/service.rs:337-454` — the whole `Service` trait. `can_handle(&Parts)`, `override_operation(&RtRequest) -> Option<&dyn ErasedOperation>`, `override_routing_error(&RtRequest) -> Option<RtResponse>`: every seam takes an already-buffered request and returns a buffered response. There is no upgrade seam.
* `vendor/smithy-server-runtime/src/multi_service_router.rs:584-603` — `http_response_from_runtime_api` strips `connection` and `upgrade` from every response, so even a hand-built `101` would reach the client without the fields RFC 6455 §4.1 requires of the server's handshake.
* `vendor/server/src/lib.rs:1290-1297` — the production listener is `axum_server::from_tcp_rustls(...).serve(...)`: the fallback buffers the body and returns one `http::Response<Bytes>`. The hyper `OnUpgrade` extension survives into `OperationContext::parts()`, but only as `&Parts`, and `OnUpgrade` cannot be taken out of an immutable borrow, so no handler can await the upgrade.
* `vendor/server/src/listeners.rs:588` — the core's own HTTP listener refuses a WebSocket upgrade with `501`, so the `http()` listener is closed for this protocol too (the `tcp_plaintext()` one is what the api's socket uses, and that is the path that now works).

Required core change: a seam that lets the CLAIMING service answer `101` and then receive the upgraded connection (an `override_upgrade(&Parts) -> Option<UpgradeHandler>`, or a `Service` hook handed the `OnUpgrade` by value), plus a response path that does not strip a `101`'s hop-by-hop fields. With it, `websocket::serve` would hand the connection to the same `transport::carry_frames` the api's own socket already uses, and these three checks would pass without any further service work. Answering `101` without such a seam would be a fabricated success — the client would see the upgrade and then read EOF, with no socket to carry `$connect` and no session a minted connection id could address.

Alternatively, a client that dials the announced `ApiEndpoint` as given needs nothing: the socket is there today.

Traces: apigatewayv2.websocket-connect-mock-integration-rejected (now passes its `500` assertion and hangs on the healthy leg's `101`), apigatewayv2.mock-integration-response-selected-by-status-code-pattern (hang), apigatewayv2.websocket-post-to-connection-delivers-then-gone-after-disconnect (hang).

## Evidence
Replay of the three checks against the branch that served the parent report (3 of 3 replayed):

* `apigatewayv2.websocket-connect-mock-integration-rejected` — the `run#16` assertion (`result["status"] == 500`, a `$connect` route bound to a MOCK integration) is satisfied; the check then fails at `_poll_until#30` with "Check timed out after 60.0s", polling for the healthy control leg's `101`.
* `apigatewayv2.mock-integration-response-selected-by-status-code-pattern` — "Check timed out after 60.0s" at `_poll_until#14`: it polls for a live socket on the endpoint under test.
* `apigatewayv2.websocket-post-to-connection-delivers-then-gone-after-disconnect` — "Check timed out after 60.0s" at `_poll_until#18`: same shape; no connectionId can exist until a handshake completes there.

The same three behaviours pass over the api's OWN socket, in the owning crate's suite (`tests/all/websocket_transport_data_plane.rs`): `the_announced_socket_answers_the_handshake_from_the_apis_records`, `a_message_is_answered_by_the_integration_response_its_status_selects`, and `a_carried_connection_is_posted_to_and_then_gone_after_the_client_closes`.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. CreateApi(ProtocolType='WEBSOCKET', RouteSelectionExpression='$request.body.action'); CreateIntegration(IntegrationType='MOCK'); CreateRoute(RouteKey='$default', Target='integrations/<integ>'); CreateRouteResponse(RouteResponseKey='$default'); CreateStage(StageName='s'); CreateDeployment(StageName='s').
3. Open an RFC 6455 upgrade against the `ApiEndpoint` the api announces, AS GIVEN: expect — and observe — `101 Switching Protocols`, then a frame dispatched by the route selection expression.
4. Open the same upgrade against the emulator's own data port, with the path `/_aws/execute-api/<api-id>/s`: expect `101`; observe `501 NotImplemented` naming the announced socket.
5. Re-replay the three check ids in trace_ids.
