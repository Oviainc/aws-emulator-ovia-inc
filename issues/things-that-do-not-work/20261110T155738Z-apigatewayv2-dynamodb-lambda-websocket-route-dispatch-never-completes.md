Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-connect-request-authorizer-deny-refuses-before-integration apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-selection-expression-update-redirects-dispatch apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-unmatched-action-without-default-route-not-delivered
Issue-Key: apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-connect-request-authorizer-deny-refuses-before-integration|_poll_until#28|Timeout|time.sleep(interval_s)
Issue-Key: apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-selection-expression-update-redirects-dispatch|_send_settled#22|Timeout|reply = _send_json(ws, make_doc(nonce), tolerate_non_json=tolerate_non_json)
Issue-Key: apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-unmatched-action-without-default-route-not-delivered|_send_settled#25|Timeout|reply = _send_json(ws, make_doc(nonce))
Issue-Commit: b11785bf9bf8af57a00ad7e4d814b1c5f1d66117

# apigatewayv2__dynamodb__lambda: WebSocket route dispatch never completes — REQUEST-authorizer deny on $connect, a route-selection-expression update and an unmatched action with no $default route all hang out the check budget

> _Note: this issue was identified against the code base at commit `b11785bf9bf8af57a00ad7e4d814b1c5f1d66117`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApi, CreateRoute, CreateAuthorizer, UpdateApi

## Description
Cause 1 of 1: the API Gateway v2 WebSocket data plane never produces an observable outcome for a route-dispatch decision, so three route-dispatch checks exhaust the suite's budget instead of finishing.

AWS: a WebSocket API's `$connect` route with an AUTHORIZER of type REQUEST invokes the authorizer BEFORE the integration — an IAM `Deny` policy makes the handshake fail (HTTP 401/403 on the upgrade) and the `$connect` integration is never invoked; a `RouteSelectionExpression` update changes which route a frame's body selects on subsequent connections; and a frame whose selected action matches no route, with no `$default` route declared, is NOT delivered to any integration. Smithy: ApiGatewayV2 models CreateApi (ProtocolType WEBSOCKET, RouteSelectionExpression), CreateRoute (RouteKey, AuthorizationType AUTHORIZER, AuthorizerId) and CreateAuthorizer (AuthorizerType REQUEST) — the model states the wiring; the dispatch/handshake semantics above are AWS documented/recorded behavior. Each outcome is observable to a client: an upgrade that is refused, a frame that reaches one handler, or a frame that reaches none while the connection stays open.

Emulator: none of the three outcomes is ever observable — each check ends with `Check timed out after 60.0s`, with no refusal, no close frame and no integration invocation recorded. The checks poll for a terminal signal (the handshake's status, or a DynamoDB row written by the authorizer/`$connect`/custom-route handler) and nothing ever arrives, so there is no response to compare. Hypothesis: the WebSocket upgrade or the per-frame route dispatch produces no answer at all on the emulator's own listener, so every poll in these checks is unsatisfiable.

Why: a client of the emulator's WebSocket APIs cannot determine whether a connection was authorized, which route a frame selected, or whether an unmatched frame was dropped. Code that waits on a connect refusal or on a handler side effect hangs rather than failing, which is the worst failure mode for an emulator: an unbounded wait instead of a verdict.

Fix: the WebSocket data plane of the apigatewayv2 service crate — the `$connect` upgrade path (REQUEST-authorizer invocation and its Allow/Deny translation into an accepted or refused handshake) and the per-frame route-selection dispatch (RouteSelectionExpression evaluation, custom route vs `$default`, and the no-match drop), so each produces a terminal, observable result. Open issue 20261109T154749Z-apigatewayv2-dynamodb-lambda-3-net-new-causes-the.md and 20261023T223918Z-apigatewayv2-dynamodb-lambda-all-three-websocket-duplex-checks.md carry other apigatewayv2 WebSocket checks' replay keys; the three checks here are not covered by either.

Traces: the three `apigatewayv2__dynamodb__lambda` checks in trace_ids.

## Evidence
Expected (per check):
  - deep-apigatewayv2-1-wsroute-connect-request-authorizer-deny-refuses-before-integration: with a DENY_TOKEN the upgrade is refused and the `$connect` integration leaves NO row; with ALLOW_TOKEN it connects and leaves one.
  - deep-apigatewayv2-1-wsroute-selection-expression-update-redirects-dispatch: after UpdateApi changes RouteSelectionExpression, a frame selects the newly addressed route.
  - deep-apigatewayv2-1-wsroute-unmatched-action-without-default-route-not-delivered: a frame whose action matches no route and no `$default` reaches no integration.
Actual, all three: `Error: Check timed out after 60.0s` — no handshake verdict, no close frame, no handler side effect, no response of any kind to compare.

## Steps to Reproduce
1. CreateApi(ProtocolType="WEBSOCKET", RouteSelectionExpression="$request.body.action").
2. CreateAuthorizer(AuthorizerType="REQUEST", identity source on a query string token) backed by a Lambda that returns Allow for one token and an explicit Deny for another, recording each decision in DynamoDB.
3. CreateRoute(RouteKey="$connect", AuthorizationType="AUTHORIZER") with a Lambda integration that records one row per invocation; deploy a stage.
4. Open a WebSocket connection to the stage's published wss endpoint with the deny token and wait for the handshake verdict — nothing terminal arrives.
5. Repeat with a custom route plus a RouteSelectionExpression update, and with a frame whose action matches no route and no `$default`: neither produces an observable dispatch outcome.
Replay the three checks in trace_ids.
