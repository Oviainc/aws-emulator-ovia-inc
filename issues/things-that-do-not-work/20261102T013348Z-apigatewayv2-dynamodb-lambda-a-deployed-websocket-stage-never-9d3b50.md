Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-unmatched-action-without-default-route-not-delivered
Issue-Key: apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-unmatched-action-without-default-route-not-delivered|_ws_handshake#20|Timeout|return status, text, None
Issue-Commit: b6e56c84474d6259c908f436d81ed99c7ca34d50

# apigatewayv2__dynamodb__lambda: a deployed WEBSOCKET stage never completes the RFC 6455 upgrade, so the no-$default unmatched-action drop path (gateway `Forbidden` frame, zero integration invocations, socket stays open) reaches no verdict (1 cause)

> _Note: this issue was identified against the code base at commit `b6e56c84474d6259c908f436d81ed99c7ca34d50`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApi

## Description
Cause 1 of 1: on a WEBSOCKET API whose RouteSelectionExpression is `$request.body.action`, carrying exactly one custom route `sendmessage` and NO `$default` route, the emulator never serves the deployed stage's WebSocket data plane. The client cannot even open the socket, so a frame whose action matches no route key is neither dispatched nor refused and the entire negative-delivery contract is unobservable.

AWS: the apigatewayv2 Smithy model carries no WebSocket data-plane (per-frame dispatch) operation, so the only modeled hook is the configuration member — Smithy: CreateApiRequest.RouteSelectionExpression, documented "The route selection expression for the API. For HTTP APIs, the routeSelectionExpression must be ${request.method} ${request.path}. If not provided, this will be the default for HTTP APIs. This property is required for WebSocket APIs." Everything beyond that is recorded behavior only. What the recording shows: (1) the wss:// RFC 6455 upgrade to the deployed stage answers 101 Switching Protocols and the connection stays open; (2) a frame whose `$request.body.action` matches a declared route key EXACTLY is dispatched to that route's AWS_PROXY integration and that integration is invoked exactly once for that frame; (3) a frame whose action matches no route key, on an API with no `$default` route, is delivered to NO integration — the integration's own out-of-band per-frame invocation count stays at zero — and the gateway itself answers on the same socket with its own error frame whose message is the fixed text `Forbidden`; the accompanying connectionId and requestId are variable parts supplied by the live connection and the request, never fixed text and never the literal values of one recording; (4) the socket is NOT closed, and a subsequent matching frame is still dispatched. Route-key matching is exact and case-sensitive, so `{"action":"SendMessage"}` is as unmatched as `{"action":"nosuchroute"}`.

Emulator: the check never reaches any of those assertions. It is killed at its budget — `Error: Check timed out after 60.0s` — while the handshake poll (`_poll_until(lambda: _try_open(ctx, api_id, stage, ""), ...)`, itself budgeted 150 s) spins against the deployed `dev` stage, i.e. the stage never produces an open WebSocket connection at all. No frame is ever sent, no `Forbidden` error frame is ever emitted, and the effect-absent half (zero invocations recorded for the unmatched frame, observed as a consistent DynamoDB Query COUNT) can never be evaluated. The expected gateway refusal frame carrying the fixed text `Forbidden` is therefore absent entirely rather than merely worded differently.

Why: a client relying on AWS's behavior cannot distinguish an unmatched-action frame that was correctly dropped from a connection that simply hung. Because nothing is dispatched at all, an emulator that hands unmatched frames to the only integration the API happens to have, or that matches route keys case-insensitively, is indistinguishable here from one that drops them correctly: the negative-delivery guarantee is unverifiable, and `$default` stops being the only fallback. Any client library that opens a WebSocket against a deployed WEBSOCKET stage blocks until its own timeout instead of receiving 101 Switching Protocols.

Fix: the apigatewayv2 WEBSOCKET execute-api data-plane listener (the RFC 6455 upgrade handler for a deployed stage) and, behind it, the route-selection dispatcher's no-match branch, which must emit the gateway's own `Forbidden` error frame, invoke no integration, and leave the connection open. Fixing only the upgrade path is insufficient — the no-match branch must then actually refuse rather than fall through to the single available integration.

Related already-open issues that the suite does NOT report as owning this failure (named here so they are not closed as covering it): 20261030T123305Z-apigatewayv2-dynamodb-lambda-a-deployed-websocket-stage-never-b8efca.md and 20261029T190217Z-apigatewayv2-dynamodb-lambda-a-deployed-websocket-stage-never-2ab070.md both list this check id but their recorded replay keys no longer match this failure frame; 20261029T180026Z-apigatewayv2-dynamodb-lambda-api-gateway-v2-websocket-oversize-frame-not-closed.md bundles an unmatched-action cause but is keyed to the oversize-frame legs; 20261023T223918Z-apigatewayv2-dynamodb-lambda-all-three-websocket-duplex-checks.md and 20261029T065950Z-apigatewayv2-dynamodb-lambda-the-deployed-execute-api-data.md cover sibling duplex / HTTP-$default legs.

Traces: apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-unmatched-action-without-default-route-not-delivered

## Evidence
Failing check (verdict: mismatch):
  apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-unmatched-action-without-default-route-not-delivered
  Error: Check timed out after 60.0s

Expected (AWS, per the recording), after CreateApi(ProtocolType="WEBSOCKET", RouteSelectionExpression="$request.body.action"), CreateIntegration(IntegrationType="AWS_PROXY" -> the Lambda), CreateRoute(RouteKey="sendmessage", Target="integrations/<id>", RouteResponseSelectionExpression="$default"), CreateRouteResponse("$default"), CreateDeployment + CreateStage("dev"):
  1. the wss:// handshake against the deployed stage answers 101 Switching Protocols and the socket stays open;
  2. `{"action":"sendmessage","nonce":N}` is answered by the sendmessage integration (reply carries that function's marker and routeKey="sendmessage"), and the integration records exactly ONE invocation for that frame (out-of-band consistent DynamoDB Query COUNT under pk=<marker>#nonce:N == 1);
  3. `{"action":"nosuchroute",...}` and the case-differing `{"action":"SendMessage",...}` are each answered by the gateway's OWN error frame whose message is the fixed text `Forbidden` (the accompanying connectionId/requestId are per-connection/per-request values, not fixed text), the sendmessage integration records ZERO invocations for those frames held open over a settling window, and the socket is not closed;
  4. a subsequent `{"action":"sendmessage","nonce":N9}` frame on the same socket is still dispatched and recorded exactly once.

Actual (emulator): step 1 never completes. The handshake poll never satisfies its predicate and the check is killed at the 60 s harness cap with `Check timed out after 60.0s`. No frame is sent, so steps 2-4 produce no verdict at all: there is no `Forbidden` refusal frame, no dispatch, and no invocation record of any kind.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Replay the behavioral check apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-unmatched-action-without-default-route-not-delivered.
3. The check creates a DynamoDB table, an IAM role trusting lambda.amazonaws.com (inline policy allowing logs:* and dynamodb:PutItem on the table), one Lambda function that writes one DynamoDB row per invocation keyed by <marker>#<frame key>, an apigatewayv2 WEBSOCKET API with RouteSelectionExpression="$request.body.action", an AWS_PROXY integration pointing at that function, a single route RouteKey="sendmessage" (and NO $default route), a $default route response, a deployment and a stage "dev", plus lambda:AddPermission for apigateway.amazonaws.com on arn:aws:execute-api:<region>:<account>:<api-id>/*/*.
4. Observe that opening a WebSocket connection to the deployed "dev" stage never succeeds: the handshake poll loop spins until the harness cap and the check reports `Check timed out after 60.0s`.
5. Expected instead: 101 Switching Protocols; the matching `sendmessage` frame dispatched exactly once; `{"action":"nosuchroute"}` and `{"action":"SendMessage"}` each answered by the gateway's own error frame with message `Forbidden`, with ZERO integration invocations and the socket still open; then a further matching frame still dispatched exactly once.
