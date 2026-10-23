Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-request-authorizer-allow-passes-context-to-message-routes apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-request-authorizer-deny-policy-forbidden-403-handshake apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-unmatched-action-without-default-route-rejected-with-error-frame
Issue-Key: apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-request-authorizer-allow-passes-context-to-message-routes|_poll_until#4|Timeout|time.sleep(interval_s)
Issue-Key: apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-request-authorizer-deny-policy-forbidden-403-handshake|_poll_until#2|Timeout|time.sleep(interval_s)
Issue-Key: apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-unmatched-action-without-default-route-rejected-with-error-frame|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Commit: 836bc7a08cca6ccbbb64b51e180fe3d49cf03502

# apigatewayv2__lambda: all three WebSocket authorizer/routing checks hang to the 60 s cap at the deployed WEBSOCKET stage (1 cause)

> _Note: this issue was identified against the code base at commit `836bc7a08cca6ccbbb64b51e180fe3d49cf03502`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
$connect (WebSocket data plane)

## Description
Cause 1 of 1: three cross-service WebSocket checks never complete — each hangs to the 60 s harness cap without reaching its assertions. Failing check ids: apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-request-authorizer-allow-passes-context-to-message-routes, apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-request-authorizer-deny-policy-forbidden-403-handshake, apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-unmatched-action-without-default-route-rejected-with-error-frame.

AWS: recorded/documented behavior only for the data-plane legs — the opening handshake and frame routing are the execute-api data plane and are not modeled operations. Smithy (apigatewayv2 2018-11-29) establishes the surface: ProtocolType enum [WEBSOCKET, HTTP]; Api.ApiEndpoint doc "The URI of the API, of the form {api-id}.execute-api.{region}.amazonaws.com. The stage name is typically appended to this URI to form a complete path to a deployed API stage"; Api.RouteSelectionExpression required and, for WebSocket APIs, selecting the route from the frame. On real AWS: a REQUEST authorizer on $connect returning an Allow policy admits the handshake with 101 and binds its principalId and context map to that connection, so every later frame reaches its route's Lambda with requestContext.authorizer carrying them; a Deny policy refuses the handshake with HTTP 403 and no upgrade header, and the $connect integration never runs; and a frame whose action matches no route on an API with no $default route is refused with an error frame on the still-open socket (recorded message example: {"message": "Forbidden"}).

Emulator: none of the three checks produces a verdict within the cap; the harness reports only `Check timed out after 60.0s`, so it does not name the blocking step. In all three the only unbounded waits are polls — a 180 s poll for a 101 WebSocket upgrade on the deployed stage (`_poll_until(_try, lambda r: r[0] == 101, timeout_s=180.0, interval_s=5.0, description=f"WebSocket 101 upgrade on stage {stage} with query {query!r}")`, reached via `_open_watcher` at the very start of each run) and 120 s Lambda State/LastUpdateStatus polls. Hypothesis: the same WebSocket-handshake gap this run proves elsewhere — the sibling apigatewaymanagementapi checks read the status line for that upgrade directly on this emulator and report b'HTTP/1.1 404 Not Found', and apigatewayv2 / apigatewayv2__dynamodb__lambda checks hang in the equivalent 101 poll.

Why: a client relying on AWS behavior can neither connect nor get an authorization decision. The $connect authorizer layer is unobservable in both directions (an Allow that must admit and propagate context, a Deny that must refuse with 403 before the integration runs), and the unmatched-action refusal that must leave the socket usable cannot be graded. The client sees a hang or a bare 404 instead of 101/403.

Fix: the execute-api data-plane front door / request router and the apigatewayv2 WebSocket session layer — complete the RFC 6455 handshake (101 + Sec-WebSocket-Accept) on a deployed WEBSOCKET API's stage path instead of falling through to the generic 404; then run the $connect REQUEST authorizer before the integration (403 and no upgrade on Deny), bind its principalId/context to the connection so later frames carry requestContext.authorizer, and route frames by the route-selection expression, refusing an unmatched action with an error frame rather than closing the socket.

Related open issue, which does not cover this: "apigatewayv2__lambda: WebSocket REQUEST-authorizer checks abort because apigatewayv2 CreateAuthorizer answers NotImplemented (1 cause)" (20261023T124140Z-...) describes an ABORT on a control-plane call; in this replay the two authorizer checks no longer abort, they hang at the data plane, and the third check (unmatched action) is not in it at all.

Traces: the three failing check ids listed above.

## Evidence
Suite: simparity-behavioral-ga-prod-v1, apigatewayv2__lambda 0/3 (0%).

Expected (real AWS):
  - allow-passes-context-to-message-routes: the $connect REQUEST authorizer returning Allow admits the handshake (101); two connections admitted with different query-string tenants each see their OWN principalId/context on every later frame.
  - deny-policy-forbidden-403-handshake: `status == 403` on the handshake, with no `upgrade: websocket` in the response head; the authorizer ran for that attempt but $connect did not; the control attempt upgrades (101) and its $connect sees the authorizer's principalId.
  - unmatched-action-without-default-route: an action matching no route with no $default is refused with an error frame (recorded example {"message": "Forbidden"}) and the same socket still routes the next valid action to its Lambda.

Actual (emulator), all three: `Check timed out after 60.0s` — no assertion is ever reached. Each run begins with `_open_watcher(...)` -> `_ws_open(...)` -> `_poll_until(_try, lambda r: r[0] == 101, timeout_s=180.0, interval_s=5.0, description=f"WebSocket 101 upgrade on stage {stage} with query {query!r}")`, whose `_ws_handshake` closes the socket and returns the parsed status for anything other than 101; the harness cap (60 s) is well inside that 180 s poll.

Corroborating evidence from the same replay for what that upgrade returns on this emulator: 10 apigatewaymanagementapi checks report `ConnectionError: websocket handshake failed: b'HTTP/1.1 404 Not Found'`, and apigatewayv2.websocket-broadcast-to-multiple-connections plus all three apigatewayv2__dynamodb__lambda checks hang in their own 101 polls. The harness does not state which poll consumed the budget in these three, so that attribution is labelled Hypothesis above.

Model facts used: smithy_query shape ProtocolType -> [WEBSOCKET, HTTP]; smithy_query shape Api -> ApiEndpoint/RouteSelectionExpression docs.

## Steps to Reproduce
1. Boot the emulator and replay the apigatewayv2__lambda WebSocket checks of simparity-behavioral-ga-prod-v1.
2. Create the Lambdas, create_api(ProtocolType=WEBSOCKET), a REQUEST authorizer on $connect, the AWS_PROXY integrations and routes, then create_deployment and create_stage.
3. Send an RFC 6455 opening handshake to the stage's WebSocket URL with the authorizer's allow query string and poll for a 101.
4. Observe that no 101 arrives within the poll (the upgrade on this emulator answers HTTP/1.1 404 Not Found), so the checks never reach their 403/101 and frame-routing assertions and hang to the 60 s cap.
