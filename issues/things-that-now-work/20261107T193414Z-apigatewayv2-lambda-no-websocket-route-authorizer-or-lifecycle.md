Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-connect-and-disconnect-routes-fire-their-own-lambdas apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-request-authorizer-allow-passes-context-to-message-routes apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-request-authorizer-deny-policy-forbidden-403-handshake apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-request-authorizer-missing-identity-source-unauthorized-401 apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-route-selection-dispatches-each-action-to-its-own-lambda apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-unmatched-action-without-default-route-rejected-with-error-frame
Issue-Key: apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-connect-and-disconnect-routes-fire-their-own-lambdas|_poll_until#8|Timeout|raise AssertionError(
Issue-Key: apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-request-authorizer-allow-passes-context-to-message-routes|_poll_until#4|Timeout|raise AssertionError(
Issue-Key: apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-request-authorizer-deny-policy-forbidden-403-handshake|_poll_until#2|Timeout|raise AssertionError(
Issue-Key: apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-request-authorizer-missing-identity-source-unauthorized-401|_poll_until#2|Timeout|raise AssertionError(
Issue-Key: apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-route-selection-dispatches-each-action-to-its-own-lambda|_poll_until#8|Timeout|raise AssertionError(
Issue-Key: apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-unmatched-action-without-default-route-rejected-with-error-frame|_poll_until#9|Timeout|raise AssertionError(
Issue-Commit: c3a6f169c346304d6b4c388bf8378b1c143b9d70

# apigatewayv2__lambda: no WebSocket route, authorizer or lifecycle contract can be graded — the deployed WEBSOCKET stage never completes an RFC 6455 upgrade, so all 6 cross-service checks hang to the 180 s cap (1 cause)

> _Note: this issue was identified against the code base at commit `c3a6f169c346304d6b4c388bf8378b1c143b9d70`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStage

## Description
ONE net-new root cause on the apigatewayv2__lambda service token, from the 6 failures the replay marks neither ALREADY FILED nor OWNED.

=== Cause 1 of 1: every WEBSOCKET check blocks in its handshake helper because a deployed stage never answers 101, so $connect/$disconnect dispatch, route selection, the request-authorizer allow/deny/missing-identity-source refusals and the unmatched-action error frame are all ungradeable ===

AWS: recorded behavior only for the socket wire; the control-plane shapes used are modelled (CreateApi ProtocolType=WEBSOCKET with RouteSelectionExpression, CreateAuthorizer AuthorizerType=REQUEST with IdentitySource, CreateRoute with AuthorizationType=CUSTOM and RouteResponse, CreateIntegration AWS_PROXY, CreateDeployment, CreateStage). Recorded handshake and dispatch behavior: a correct RFC 6455 handshake against a deployed stage is answered `HTTP/1.1 101 Switching Protocols` with `Upgrade: websocket`, `Connection: Upgrade` and `Sec-WebSocket-Accept: <base64(sha1(Sec-WebSocket-Key + 258EAFA5-E914-47DA-95CA-C5AB0DC85B11))>`; the `$connect` route's OWN Lambda is invoked with requestContext.eventType "CONNECT", its routeKey "$connect", the query string and the newly minted connectionId; each client frame is dispatched by RouteSelectionExpression to that action's own Lambda under the SAME connectionId; a clean close invokes the `$disconnect` route's own Lambda with eventType "DISCONNECT". With a REQUEST authorizer: an Allow policy completes the handshake and passes authorizer context to the message routes; a Deny policy fails the handshake with `HTTP/1.1 403 Forbidden`; a request missing the configured IdentitySource fails it with `HTTP/1.1 401 Unauthorized` — both before any route Lambda runs. An action with no matching route and no `$default` route is refused with an error frame on the socket rather than silently dropped.

Emulator: none of this is reachable. All 6 checks are cut by the harness at their cap — `Error: Check timed out after 180.0s` — while blocked in their WebSocket-handshake helper (`_ws_open`/`_poll_until` waiting for a 101). The same front door, probed by the apigatewayv2 and apigatewayv2__cloudwatch__lambda checks in this run, answers the upgrade `HTTP/1.1 404 Not Found` with `x-amzn-errortype: UnknownService`. With no 101 there is no connectionId, so no $connect/$disconnect invocation, no route selection, no authorizer evaluation and no unmatched-action frame ever happens.

Why: a client that provisions a WEBSOCKET API with a REQUEST authorizer and per-action Lambdas exactly as AWS documents cannot connect at all, and the handshake-time refusal codes it depends on (403 for an authorizer Deny, 401 for a missing identity source) are indistinguishable on the emulator from "the stage does not exist" (404). Authorization that is never evaluated is also silently over-permissive once the upgrade is implemented without it.

Fix: the emulator's front-door HTTP router for the execute-api data plane must complete the upgrade (101 + correct Sec-WebSocket-Accept) and hand the hijacked connection to an apigatewayv2 WEBSOCKET data plane that: evaluates a REQUEST authorizer at handshake time (401 when a configured IdentitySource is absent, 403 when the policy denies, otherwise pass the authorizer context into every later route event), mints a connectionId and invokes the $connect route's integration with eventType CONNECT, dispatches each frame by RouteSelectionExpression to that route's integration under the same connectionId, invokes the $disconnect route's integration with eventType DISCONNECT on close, and refuses an unmatched action with an error frame when no $default route exists.

Related (does NOT cover these six checks): 20261104T160000Z-apigatewayv2-websocket-execute-api-upgrade-remains-unservable.md and 20261107T173512Z-apigatewayv2-dynamodb-lambda-a-deployed-websocket-api-stage.md carry the same missing upgrade for other service tokens / check ids; the replay reports no owner for the six listed here.

Traces: apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-connect-and-disconnect-routes-fire-their-own-lambdas, apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-request-authorizer-allow-passes-context-to-message-routes, apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-request-authorizer-deny-policy-forbidden-403-handshake, apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-request-authorizer-missing-identity-source-unauthorized-401, apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-route-selection-dispatches-each-action-to-its-own-lambda, apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-unmatched-action-without-default-route-rejected-with-error-frame.

## Evidence
Suite: simparity-behavioral-aws-v1. Totals: 0/47 passed (0.0%); apigatewayv2__lambda 0/6 (0%), 6 failed. All six are marked neither ALREADY FILED nor OWNED, and all six report the same terminal detail:

[mismatch] apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-connect-and-disconnect-routes-fire-their-own-lambdas — Error: Check timed out after 180.0s
[mismatch] apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-request-authorizer-allow-passes-context-to-message-routes — Error: Check timed out after 180.0s
[mismatch] apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-request-authorizer-deny-policy-forbidden-403-handshake — Error: Check timed out after 180.0s
[mismatch] apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-request-authorizer-missing-identity-source-unauthorized-401 — Error: Check timed out after 180.0s
[mismatch] apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-route-selection-dispatches-each-action-to-its-own-lambda — Error: Check timed out after 180.0s
[mismatch] apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-unmatched-action-without-default-route-rejected-with-error-frame — Error: Check timed out after 180.0s

Each check's run() completes its control plane (CreateApi ProtocolType=WEBSOCKET, per-route Lambdas for $connect/$disconnect/$default/ping, CreateDeployment/CreateStage) and then blocks in `_ws_open(ctx, api_id, stage, ...)` — the helper that polls for a 101 upgrade — which is why no assertion is ever reached. The same emulator front door, probed by the apigatewayv2 checks in this same run, answers that upgrade: 'HTTP/1.1 404 Not Found', content-type: application/x-amz-json-1.1, x-amzn-errortype: UnknownService.

Expected (real AWS):
- Handshake: HTTP/1.1 101 Switching Protocols + Upgrade: websocket + Connection: Upgrade + Sec-WebSocket-Accept: base64(sha1(key + '258EAFA5-E914-47DA-95CA-C5AB0DC85B11')).
- $connect event: requestContext.routeKey '$connect', requestContext.eventType 'CONNECT', the handshake query string, and a minted connectionId; a later frame reaches the matching action's own Lambda under the SAME connectionId; a clean close delivers eventType 'DISCONNECT' to the $disconnect route's own Lambda.
- REQUEST authorizer: Deny policy -> handshake refused HTTP 403 Forbidden; request lacking the configured IdentitySource -> handshake refused HTTP 401 Unauthorized; Allow -> handshake succeeds and the authorizer context is visible to message routes.
- An action with no matching route and no $default route -> the socket receives an error frame.
Actual (emulator): no 101 is ever produced (404 UnknownService / nothing), so none of the above occurs and every check is cut at the 180 s cap with no verdict.

## Steps to Reproduce
1. Clone the repo, build and boot the emulator, and replay the six apigatewayv2__lambda check ids in trace_ids from simparity-behavioral-aws-v1.
2. CreateApi ProtocolType=WEBSOCKET with RouteSelectionExpression=$request.body.action; create one Lambda per route key ($connect, $disconnect, $default, ping) and bind each as an AWS_PROXY integration; CreateDeployment; CreateStage.
3. Send a correct RFC 6455 handshake to the stage's execute-api invoke URL. Observe that no 'HTTP/1.1 101 Switching Protocols' is returned (the front door answers 'HTTP/1.1 404 Not Found' with x-amzn-errortype: UnknownService), so the check blocks in its handshake helper until the 180 s cap.
4. Repeat with a REQUEST authorizer on $connect: with a Deny policy AWS refuses the handshake 403, with the configured IdentitySource header omitted AWS refuses it 401; the emulator produces neither, only the 404/no-answer.
5. After fixing, re-replay exactly the six check ids in trace_ids: the handshake must answer 101; $connect/$disconnect must invoke their own Lambdas with eventType CONNECT/DISCONNECT and a stable connectionId; frames must be dispatched by action to each route's own Lambda; the authorizer must refuse 403 on Deny and 401 on a missing identity source and otherwise pass its context to message routes; and an unmatched action with no $default route must be answered with an error frame.
