Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__lambda__logs.post-to-a-malformed-connection-id-is-rejected-bad-request apigatewayv2__lambda__logs.update-websocket-api-rejects-an-unexpected-selection-variable-and-keeps-the-old-expression apigatewayv2__lambda__logs.websocket-api-create-rejects-a-cors-configuration
Issue-Key: apigatewayv2__lambda__logs.post-to-a-malformed-connection-id-is-rejected-bad-request|run#5|AssertionError|assert (code, message) == ("BadRequestException", _MALFORMED_MESSAGE), (
Issue-Key: apigatewayv2__lambda__logs.update-websocket-api-rejects-an-unexpected-selection-variable-and-keeps-the-old-expression|_error#7|AssertionError|raise AssertionError(f"real AWS refuses this call, but it succeeded and returned {result!r}")
Issue-Key: apigatewayv2__lambda__logs|CreateApi|BadRequestException
Issue-Commit: 86c9f1e4438a61fbe37b19dfcd3d77efce3d0679

# Three apigatewayv2/management-api refusals diverge: an invalid connectionId is 410 instead of 400, UpdateApi takes a route selection expression that can never match, and the Cors refusal uses the emulator's own sentence (3 causes)

> _Note: this issue was identified against the code base at commit `86c9f1e4438a61fbe37b19dfcd3d77efce3d0679`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._
>
> _This report originally carried a fourth cause — the RFC 6455 upgrade to a deployed WEBSOCKET stage, answered `404 UnknownService`, with eleven `_ws_connect` keys of its own. That cause is NOT fixed: it needs a core seam that hands the claiming service the upgraded connection, which no service crate can add. It was split out and re-filed, with its eleven replay keys and the vendor seams it needs, as `issues/things-that-do-not-work/20261104T160000Z-apigatewayv2-websocket-execute-api-upgrade-remains-unservable.md`, so it is not recorded as working. The check `post-to-a-malformed-connection-id-is-rejected-bad-request` now passes its own assertion (cause 2 below) and then fails on that upgrade._

## Operation
CreateApi

## Description
Three net-new root causes.

Cause 2 of 4 (original numbering) — PostToConnection classifies a connectionId that identifies no connection as a 410 GoneException with an empty message instead of a 400 BadRequestException naming it.
AWS: ApiGatewayManagementApi PostToConnection; recorded behavior only for the text. A ConnectionId that is not a well-formed connection identifier is refused with HTTP 400 / BadRequestException and the message `Invalid connectionId: <connectionId>` — fixed text `Invalid connectionId: ` plus the ConnectionId copied verbatim from the request. GoneException (410) is reserved for a WELL-FORMED id whose connection is closed.
Emulator: answered GoneException with an EMPTY message (the modelled shape carries no members) for every id the registry did not hold, so both the status class (410 vs 400) and the message were wrong.
Why: handlers are written to prune their connection row on 410 and to log-and-continue on 400. Turning an id that never identified a connection into a 410 makes every such handler delete a live row, and the empty message leaves the client nothing to log.
Fixed: a connection id is one the service ISSUES when a handshake completes, so the management API now records every id it issued (`state::issued`) and keeps it after `DeleteConnection` closes the connection. An id it never issued is refused `400 BadRequestException` / `Invalid connectionId: <id>` before any connection is looked up, in all three operations that bind `{ConnectionId}`; an id it issued whose connection has closed is still the modelled `GoneException`. Pinned by `tests/all/connection_id_validity.rs`.
Traces: apigatewayv2__lambda__logs.post-to-a-malformed-connection-id-is-rejected-bad-request

Cause 3 of 4 (original numbering) — UpdateApi accepts a RouteSelectionExpression that names the whole request body on a WEBSOCKET API.
AWS: Smithy (com.amazonaws.apigatewayv2) UpdateApi PATCH /v2/apis/{ApiId}, success 200, member RouteSelectionExpression: SelectionExpression, models BadRequestException with httpError 400. The model carries no message text, so for the text this is recorded behavior only: setting a WEBSOCKET API's RouteSelectionExpression to a bare `$request.body` (no field selector) is refused with HTTP 400 / BadRequestException and the message `Unexpected variable in selection expression: <expression>` — fixed text plus the rejected reference from the request — and the API keeps the expression it had.
Emulator: answered HTTP 200 and stored the bare `$request.body`, so GetApi then reported it as the live route selection expression.
Why: a WEBSOCKET API whose route selection names the whole body can never match a route key, so the API stops routing messages. AWS refuses the edit precisely so a bad update cannot strand live routing.
Fixed: `state::selection_expression::validate_route_selection_expression` holds every writer of the member to one rule — on a WEBSOCKET api every variable reference must name a FIELD of the message body (`$request.body.<path>`) — and both `CreateApi` and `UpdateApi` apply it, the patch on the merged value, so a refused patch leaves the stored expression in place. Pinned by `tests/all/route_selection_expression_validation.rs`.
Traces: apigatewayv2__lambda__logs.update-websocket-api-rejects-an-unexpected-selection-variable-and-keeps-the-old-expression

Cause 4 of 4 (original numbering) — CreateApi's WEBSOCKET+CorsConfiguration refusal used the emulator's own sentence.
AWS: Smithy CreateApi POST /v2/apis (success 201), members ProtocolType (required, enum) and CorsConfiguration: Cors, models BadRequestException httpError 400. Model text only; the recorded refusal for a WEBSOCKET API created with a CorsConfiguration is HTTP 400 / BadRequestException with the message `Cors is not supported for WEBSOCKET protocolType` — entirely fixed text apart from the protocol type.
Emulator: answered 400 / BadRequestException (status and code correct) with `Currently, CorsConfiguration is not permitted for APIs with a protocol type of WEBSOCKET` — a different fixed sentence.
Why: a client or IaC provider matching the documented sentence does not recognise the refusal; the divergence is message-only.
Fixed: `state::validate_cors_protocol` owns both the gate and this member's own sentence, and `CreateApi` and `UpdateApi` call it instead of the generic protocol-restricted wording the other members keep. Pinned by `tests/all/cors_protocol_refusal.rs`.
Traces: apigatewayv2__lambda__logs.websocket-api-create-rejects-a-cors-configuration

## Evidence
All from the simparity-behavioral-aws-v1 replay against this checkout.

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

Cause 2: aws apigatewaymanagementapi post-to-connection --endpoint-url <stage mgmt endpoint> --connection-id 'AAAAAAAAAAAAAAA=' --data x -> GoneException with an empty message; AWS answers 400 BadRequestException 'Invalid connectionId: AAAAAAAAAAAAAAA='.

Cause 3: aws apigatewayv2 create-api --name t --protocol-type WEBSOCKET --route-selection-expression '$request.body.action' ; then aws apigatewayv2 update-api --api-id <id> --route-selection-expression '$request.body' -> 200 and the expression is replaced; AWS answers 400 BadRequestException 'Unexpected variable in selection expression: $request.body' and keeps '$request.body.action'.

Cause 4: aws apigatewayv2 create-api --name t --protocol-type WEBSOCKET --route-selection-expression '$request.body.action' --cors-configuration AllowOrigins=* -> 400 with 'Currently, CorsConfiguration is not permitted ...'; AWS answers 'Cors is not supported for WEBSOCKET protocolType'.

Then re-replay the three check ids listed in trace_ids.
