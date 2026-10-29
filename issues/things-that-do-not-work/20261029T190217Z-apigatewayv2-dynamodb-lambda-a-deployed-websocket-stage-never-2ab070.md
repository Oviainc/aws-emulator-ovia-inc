Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-unmatched-action-without-default-route-not-delivered
Issue-Key: apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-unmatched-action-without-default-route-not-delivered|_ws_handshake#20|Timeout|sock = ssl.create_default_context().wrap_socket(sock, server_hostname=http_host)
Issue-Commit: 57a21f5ddc052f99eaa78f0f6151a394ad8bec07

# apigatewayv2__dynamodb__lambda: a deployed WEBSOCKET stage never answers an unmatched route-selection action, so the no-$default drop path reaches no verdict (1 cause)

> _Note: this issue was identified against the code base at commit `57a21f5ddc052f99eaa78f0f6151a394ad8bec07`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InvokeWebSocketRoute

## Description
Cause 1 of 1: on a WEBSOCKET API whose RouteSelectionExpression is `$request.body.action`, with a custom `sendmessage` route and NO `$default` route, a frame whose action matches no route is never answered on the socket and the drop is never observable.

AWS: recorded behavior only — the apigatewayv2 model carries no WebSocket data-plane (frame dispatch) operation; the only modeled hook is the configuration member, Smithy: CreateApiRequest.RouteSelectionExpression ("The route selection expression for the API. ... This property is required for WebSocket APIs."). What the recording shows is: the RFC 6455 upgrade to the deployed stage completes (101 Switching Protocols) and the connection stays open; a frame whose `$request.body.action` matches a declared route key EXACTLY is dispatched to that route's AWS_PROXY integration; a frame whose action matches no route key — on an API with no `$default` route — is NOT delivered to any integration (the integration's own out-of-band DynamoDB write count for that frame stays at zero), the gateway itself answers on the same socket with an error frame whose message is the fixed text `Forbidden` plus the per-connection/per-request variable parts the gateway supplies (`connectionId`, `requestId` — those values come from the live connection and the request, never from a recording), the socket is NOT closed, and the next matching frame is still dispatched. Route-key matching is exact and case-sensitive, so `{"action":"SendMessage"}` is as unmatched as `{"action":"nosuchroute"}`.

Emulator: the check never reaches any of those assertions. It exhausts its budget — `Error: Check timed out after 60.0s` — while polling `_poll_until(... _try_open(ctx, api_id, stage, "") ...)`, i.e. the deployed stage never produces an open WebSocket connection to send the first (positive-control) frame on. No frame is dispatched, no `Forbidden` frame is emitted, and the "zero invocations for the unmatched frame" half can never be evaluated.

Why: a client relying on AWS's behavior cannot distinguish an unmatched-action frame that was correctly dropped from a connection that hung, and cannot rely on the gateway refusing the frame rather than handing it to the only integration the API happens to have. Because nothing is dispatched at all, an emulator that routes unmatched frames to the single available integration, or that matches route keys case-insensitively, is indistinguishable here from one that drops them correctly — the negative-delivery guarantee is unverifiable and silently broken.

Fix: the apigatewayv2 WEBSOCKET execute-api data-plane listener (the RFC 6455 upgrade handler for a deployed stage) and, behind it, the route-selection dispatcher's no-match path, which must emit the gateway's own `Forbidden` error frame and leave the connection open instead of invoking a route.

Related already-open issues that do NOT carry this failure: 20261029T180026Z-apigatewayv2-dynamodb-lambda-api-gateway-v2-websocket-oversize-frame-not-closed.md names this check id in its trace list but its recorded replay key no longer matches this check's failure frame, so the suite does not report this check as owned by it; 20261023T223918Z-apigatewayv2-dynamodb-lambda-all-three-websocket-duplex-checks.md and 20261029T065950Z-apigatewayv2-dynamodb-lambda-the-deployed-execute-api-data.md cover sibling duplex/HTTP-default legs, not this one. Fixing the upgrade path alone is not enough: the no-match dispatch branch must then be asserted.

Traces: apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-unmatched-action-without-default-route-not-delivered

## Evidence
Failing check (verdict: mismatch):
  apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-unmatched-action-without-default-route-not-delivered
  Error: Check timed out after 60.0s

Expected (AWS, per the recording): after CreateApi(ProtocolType="WEBSOCKET", RouteSelectionExpression="$request.body.action"), CreateRoute(RouteKey="sendmessage", Target="integrations/<id>"), CreateRouteResponse($default), CreateDeployment + CreateStage("dev"):
  1. the wss:// handshake against the deployed stage returns 101 Switching Protocols;
  2. `{"action":"sendmessage","nonce":N}` is answered by the sendmessage integration (reply carries the integration's marker and routeKey="sendmessage"), and the integration records exactly one invocation for that frame;
  3. `{"action":"nosuchroute",...}` and `{"action":"SendMessage",...}` are answered by the gateway with an error frame whose fixed text is `Forbidden` (variable parts: the connection's connectionId and the request's requestId), the sendmessage integration records ZERO invocations for those frames, and the socket remains open;
  4. a subsequent matching frame is still dispatched.

Actual (emulator): step 1 never completes. The check's `_poll_until` on `_try_open(...)` for the stage handshake never satisfies its predicate and the check is killed at the 60 s cap, so no frame is ever sent and steps 2-4 produce no verdict. Status/error code/message of the expected gateway refusal frame (`Forbidden`) are therefore absent entirely, not merely wrong.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay the behavioral check apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-unmatched-action-without-default-route-not-delivered.
3. The check creates a DynamoDB table, an IAM role, one Lambda function (AWS_PROXY integration that writes one DynamoDB row per invocation keyed by marker + frame key), an apigatewayv2 WEBSOCKET API with RouteSelectionExpression="$request.body.action", a single route with RouteKey="sendmessage" (no $default route), a $default route response, a deployment and stage "dev".
4. Observe that opening a WebSocket connection to the deployed "dev" stage never succeeds: the check's handshake poll loop spins until the 60 s cap and reports `Check timed out after 60.0s`.
5. Expected instead: the handshake returns 101, the matching `sendmessage` frame is dispatched (one DynamoDB row), and the unmatched `nosuchroute` / case-differing `SendMessage` frames get a gateway `Forbidden` error frame with zero integration invocations while the connection stays open.
