Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__lambda__logs.agent-registration-reply-is-never-delivered-without-websocket-endpoint apigatewayv2__lambda__logs.connect-without-an-invoke-permission-is-rejected-with-500 apigatewayv2__lambda__logs.default-route-response-is-silent-until-redeployed-then-frames-the-handler-error-bodies apigatewayv2__lambda__logs.disconnect-route-deletes-the-connection-row-and-marks-the-node-disconnected apigatewayv2__lambda__logs.heartbeat-is-acknowledged-and-advances-the-node-last-seen apigatewayv2__lambda__logs.registration-prunes-a-stale-ui-connection-on-gone-and-still-notifies-the-live-ui apigatewayv2__lambda__logs.registration-reply-carries-the-node-id-stored-for-the-connection apigatewayv2__lambda__logs.registration-write-succeeds-but-no-reply-arrives-when-websocket-endpoint-is-unset apigatewayv2__lambda__logs.reply-is-denied-without-the-manage-connections-grant apigatewayv2__lambda__logs.unmatched-action-without-a-default-route-is-rejected-forbidden apigatewayv2__lambda__logs.unrouted-ping-action-is-swallowed-while-heartbeat-is-answered apigatewayv2__lambda__logs.post-to-a-malformed-connection-id-is-rejected-bad-request apigatewayv2__lambda__logs.update-websocket-api-rejects-an-unexpected-selection-variable-and-keeps-the-old-expression apigatewayv2__lambda__logs.websocket-api-create-rejects-a-cors-configuration
Issue-Key: apigatewayv2__lambda__logs.agent-registration-reply-is-never-delivered-without-websocket-endpoint|_ws_connect#3|AssertionError|assert status >= 500 and time.monotonic() < deadline, (
Issue-Key: apigatewayv2__lambda__logs.connect-without-an-invoke-permission-is-rejected-with-500|run#5|AssertionError|assert seen == [500, 500, 500], f"without an invoke grant the upgrade answered {seen!r}"
Issue-Key: apigatewayv2__lambda__logs.default-route-response-is-silent-until-redeployed-then-frames-the-handler-error-bodies|_ws_connect#4|AssertionError|assert status >= 500 and time.monotonic() < deadline, (
Issue-Key: apigatewayv2__lambda__logs.disconnect-route-deletes-the-connection-row-and-marks-the-node-disconnected|_ws_connect#2|AssertionError|assert status >= 500 and time.monotonic() < deadline, (
Issue-Key: apigatewayv2__lambda__logs.heartbeat-is-acknowledged-and-advances-the-node-last-seen|_ws_connect#2|AssertionError|assert status >= 500 and time.monotonic() < deadline, (
Issue-Key: apigatewayv2__lambda__logs.registration-prunes-a-stale-ui-connection-on-gone-and-still-notifies-the-live-ui|_ws_connect#4|AssertionError|assert status >= 500 and time.monotonic() < deadline, (
Issue-Key: apigatewayv2__lambda__logs.registration-reply-carries-the-node-id-stored-for-the-connection|_ws_connect#2|AssertionError|assert status >= 500 and time.monotonic() < deadline, (
Issue-Key: apigatewayv2__lambda__logs.registration-write-succeeds-but-no-reply-arrives-when-websocket-endpoint-is-unset|_ws_connect#3|AssertionError|assert status >= 500 and time.monotonic() < deadline, (
Issue-Key: apigatewayv2__lambda__logs.reply-is-denied-without-the-manage-connections-grant|_ws_connect#3|AssertionError|assert status >= 500 and time.monotonic() < deadline, (
Issue-Key: apigatewayv2__lambda__logs.unmatched-action-without-a-default-route-is-rejected-forbidden|_ws_connect#2|AssertionError|assert status >= 500 and time.monotonic() < deadline, (
Issue-Key: apigatewayv2__lambda__logs.unrouted-ping-action-is-swallowed-while-heartbeat-is-answered|_ws_connect#2|AssertionError|assert status >= 500 and time.monotonic() < deadline, (
Issue-Key: apigatewayv2__lambda__logs.post-to-a-malformed-connection-id-is-rejected-bad-request|run#5|AssertionError|assert (code, message) == ("BadRequestException", _MALFORMED_MESSAGE), (
Issue-Key: apigatewayv2__lambda__logs.update-websocket-api-rejects-an-unexpected-selection-variable-and-keeps-the-old-expression|_error#7|AssertionError|raise AssertionError(f"real AWS refuses this call, but it succeeded and returned {result!r}")
Issue-Key: apigatewayv2__lambda__logs|CreateApi|BadRequestException
Issue-Commit: 86c9f1e4438a61fbe37b19dfcd3d77efce3d0679

# WebSocket API data plane answers 404 UnknownService on upgrade, and three apigatewayv2/management-api refusals diverge (4 causes)

> _Note: this issue was identified against the code base at commit `86c9f1e4438a61fbe37b19dfcd3d77efce3d0679`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApi

## Description
Four net-new root causes. Each must be closed before submitting.

Cause 1 of 4 — an RFC 6455 upgrade to a deployed WEBSOCKET API stage is answered 404 UnknownService, so the whole WebSocket data plane is unreachable.
AWS: recorded behavior only (the Smithy model covers only the control plane). An HTTP/1.1 GET with `Upgrade: websocket` / `Sec-WebSocket-Key` against wss://<apiId>.execute-api.<region>.amazonaws.com/<stage> on a deployed WEBSOCKET API is answered HTTP 101 Switching Protocols when the $connect route authorizes the connection, and a 4xx/5xx from the gateway (500 when the route's integration cannot be invoked, 403 when the $default route is absent) otherwise — in every case the request reaches the API, not the service router.
Emulator: answers `HTTP/1.1 404 Not Found` with `content-type: application/x-amz-json-1.1` and `x-amzn-errortype: UnknownService`. The error type says the request was never routed to apigatewayv2's data plane at all: the emulator's front door did not recognise the execute-api host/stage path as belonging to a service, so no stage, route, authorizer or integration was ever consulted.
Why: every WebSocket client fails at connect with a service-level 404 rather than the API's own answer, so none of connect authorization, route selection, $default handling, disconnect bookkeeping or management-api fan-out can be observed; and a client branching on the gateway's status (101 vs 403 vs 500) sees a code the API never emits.
Fix: the request router/front door that dispatches execute-api host and /<stage> paths to the apigatewayv2 WEBSOCKET data plane, plus the upgrade handshake handler for a deployed stage. Note: open issue 20261029T023500Z-apigatewayv2-lambda-logs-apigatewayv2-the-rfc-6455-upgrade-to-a-deployed.md records this behavior but carries only one of these checks' keys (registration-reply-is-delivered-via-the-https-callback-url); the eleven checks listed here are not gated by it.
Traces: apigatewayv2__lambda__logs.agent-registration-reply-is-never-delivered-without-websocket-endpoint, .connect-without-an-invoke-permission-is-rejected-with-500, .default-route-response-is-silent-until-redeployed-then-frames-the-handler-error-bodies, .disconnect-route-deletes-the-connection-row-and-marks-the-node-disconnected, .heartbeat-is-acknowledged-and-advances-the-node-last-seen, .registration-prunes-a-stale-ui-connection-on-gone-and-still-notifies-the-live-ui, .registration-reply-carries-the-node-id-stored-for-the-connection, .registration-write-succeeds-but-no-reply-arrives-when-websocket-endpoint-is-unset, .reply-is-denied-without-the-manage-connections-grant, .unmatched-action-without-a-default-route-is-rejected-forbidden, .unrouted-ping-action-is-swallowed-while-heartbeat-is-answered

Cause 2 of 4 — PostToConnection classifies a malformed connectionId as a 410 GoneException with an empty message instead of a 400 BadRequestException naming it.
AWS: ApiGatewayManagementApi PostToConnection; recorded behavior only for the text. A ConnectionId that is not a well-formed connection identifier is refused with HTTP 400 / BadRequestException and the message `Invalid connectionId: <connectionId>` — fixed text `Invalid connectionId: ` plus the ConnectionId copied verbatim from the request. GoneException (410) is reserved for a WELL-FORMED id whose connection is closed.
Emulator: answers GoneException with an EMPTY message for the malformed id, so both the status class (410 vs 400) and the message are wrong.
Why: handlers are written to prune their connection row on 410 and to log-and-continue on 400. Turning a malformed-id programming error into a 410 makes every such handler delete a live row, and the empty message leaves the client nothing to log.
Fix: the connectionId parsing/validation step of PostToConnection in the apigatewaymanagementapi handler — validate the id shape and emit BadRequestException with AWS's sentence before the connection lookup that yields GoneException.
Traces: apigatewayv2__lambda__logs.post-to-a-malformed-connection-id-is-rejected-bad-request

Cause 3 of 4 — UpdateApi accepts a RouteSelectionExpression that names the whole request body on a WEBSOCKET API.
AWS: Smithy (com.amazonaws.apigatewayv2) UpdateApi PATCH /v2/apis/{ApiId}, success 200, member RouteSelectionExpression: SelectionExpression, models BadRequestException with httpError 400. The model carries no message text, so for the text this is recorded behavior only: setting a WEBSOCKET API's RouteSelectionExpression to a bare `$request.body` (no field selector) is refused with HTTP 400 / BadRequestException and the message `Unexpected variable in selection expression: <expression>` — fixed text `Unexpected variable in selection expression: ` plus the rejected expression from the request — and the API keeps the expression it had.
Emulator: answers HTTP 200 and stores the bare `$request.body`, so GetApi then reports it as the live route selection expression.
Why: a WEBSOCKET API whose route selection names the whole body can never match a route key, so the API stops routing messages. AWS refuses the edit precisely so a bad update cannot strand live routing; the emulator applies it, and a caller that relies on the refusal to protect a deployed API silently breaks it.
Fix: the RouteSelectionExpression validation in UpdateApi's (and CreateApi's) validation layer for ProtocolType=WEBSOCKET.
Traces: apigatewayv2__lambda__logs.update-websocket-api-rejects-an-unexpected-selection-variable-and-keeps-the-old-expression

Cause 4 of 4 — CreateApi's WEBSOCKET+CorsConfiguration refusal uses the emulator's own sentence.
AWS: Smithy CreateApi POST /v2/apis (success 201), members ProtocolType (required, enum) and CorsConfiguration: Cors, models BadRequestException httpError 400. Model text only; the recorded refusal for a WEBSOCKET API created with a CorsConfiguration is HTTP 400 / BadRequestException with the message `Cors is not supported for WEBSOCKET protocolType` — entirely fixed text, no variable part.
Emulator: answers 400 / BadRequestException (status and code correct) with `Currently, CorsConfiguration is not permitted for APIs with a protocol type of WEBSOCKET` — a different fixed sentence.
Why: a client or IaC provider matching the documented sentence does not recognise the refusal; the divergence is message-only.
Fix: the CreateApi CorsConfiguration/ProtocolType cross-validation message in the apigatewayv2 validation layer.
Traces: apigatewayv2__lambda__logs.websocket-api-create-rejects-a-cors-configuration

## Evidence
All from the simparity-behavioral-aws-v1 replay against this checkout.

Cause 1 — eleven checks fail in _ws_connect with:
  AssertionError: the WebSocket upgrade to the prod stage answered 404: 'HTTP/1.1 404 Not Found\r\ncontent-type: application/x-amz-json-1.1\r\nx-amzn-errortype: UnknownService\r\ncontent-length: 149'
  connect-without-an-invoke-permission-is-rejected-with-500 additionally records: AssertionError: without an invoke grant the upgrade answered [404, 404, 404] (expected [500, 500, 500]).
  The host/port and date in these lines are per-environment and are not requirements; the 404 status and the UnknownService error type are.

Cause 2 — post-to-a-malformed-connection-id-is-rejected-bad-request:
  expected ('BadRequestException', 'Invalid connectionId: <connectionId>')
  actual   ('GoneException', '')
  Message anatomy (AWS): fixed 'Invalid connectionId: ' + VARIABLE ConnectionId from the request.

Cause 3 — update-websocket-api-rejects-an-unexpected-selection-variable-and-keeps-the-old-expression:
  AssertionError: real AWS refuses this call, but it succeeded and returned {... 'ProtocolType': 'WEBSOCKET', 'RouteSelectionExpression': '$request.body' ...}
  expected ('BadRequestException', 'Unexpected variable in selection expression: $request.body'); the API's prior '$request.body.action' was overwritten.

Cause 4 — websocket-api-create-rejects-a-cors-configuration:
  AssertionError: expected 400/BadRequestException/Cors is not supported for WEBSOCKET protocolType, got 400/{'Message': 'Currently, CorsConfiguration is not permitted for APIs with a protocol type of WEBSOCKET', 'Code': 'BadRequestException'}

## Steps to Reproduce
Boot the emulator from this checkout.

Cause 1: create a WEBSOCKET API with routes ($connect/$disconnect/$default) backed by a Lambda integration, create a stage and deploy it, then open a raw socket to the stage's execute-api endpoint and send an RFC 6455 upgrade (GET /<stage> with Upgrade: websocket, Connection: Upgrade, Sec-WebSocket-Key, Sec-WebSocket-Version: 13). -> HTTP/1.1 404 Not Found with x-amzn-errortype: UnknownService. AWS answers 101 (or the API's own 4xx/5xx).

Cause 2: aws apigatewaymanagementapi post-to-connection --endpoint-url <stage mgmt endpoint> --connection-id 'AAAAAAAAAAAAAAA=' --data x -> GoneException with an empty message; AWS answers 400 BadRequestException 'Invalid connectionId: AAAAAAAAAAAAAAA='.

Cause 3: aws apigatewayv2 create-api --name t --protocol-type WEBSOCKET --route-selection-expression '$request.body.action' ; then aws apigatewayv2 update-api --api-id <id> --route-selection-expression '$request.body' -> 200 and the expression is replaced; AWS answers 400 BadRequestException 'Unexpected variable in selection expression: $request.body' and keeps '$request.body.action'.

Cause 4: aws apigatewayv2 create-api --name t --protocol-type WEBSOCKET --route-selection-expression '$request.body.action' --cors-configuration AllowOrigins=* -> 400 with 'Currently, CorsConfiguration is not permitted ...'; AWS answers 'Cors is not supported for WEBSOCKET protocolType'.

Then re-replay the fourteen check ids listed in trace_ids.
