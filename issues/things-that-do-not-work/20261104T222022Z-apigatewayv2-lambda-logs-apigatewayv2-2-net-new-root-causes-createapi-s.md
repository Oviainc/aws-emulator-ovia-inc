Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__lambda__logs.websocket-api-create-rejects-a-cors-configuration apigatewayv2__lambda__logs.post-to-a-malformed-connection-id-is-rejected-bad-request
Issue-Key: apigatewayv2__lambda__logs|CreateApi|BadRequestException
Issue-Key: apigatewayv2__lambda__logs.post-to-a-malformed-connection-id-is-rejected-bad-request|_ws_connect#7|AssertionError|assert status >= 500 and time.monotonic() < deadline, (
Issue-Commit: d9cf74aafe764d4a8806a821a1189d14d75bb9bb

# apigatewayv2: 2 net-new root causes — CreateApi's WEBSOCKET+Cors refusal message drops the trailing space AWS emits, and the RFC 6455 upgrade to a deployed WEBSOCKET stage is still answered 404 UnknownService

> _Note: this issue was identified against the code base at commit `d9cf74aafe764d4a8806a821a1189d14d75bb9bb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApi

## Description
REOPENS 20261102T090835Z-apigatewayv2-lambda-logs-websocket-api-data-plane-answers-404-unknownservice-on.md: its fix left the call refused.

Two causes, both grounded in the two failing checks of this service that no open report carries the replay key of.

Cause 1 of 2 — CreateApi refuses a WEBSOCKET api carrying a CorsConfiguration with the right status and code but the wrong message text: the sentence is one character short of AWS's, missing the trailing space AWS emits. Failing check: apigatewayv2__lambda__logs.websocket-api-create-rejects-a-cors-configuration.
AWS: Smithy (com.amazonaws.apigatewayv2) CreateApi POST /v2/apis (success 201), members ProtocolType (required, enum ProtocolType) and CorsConfiguration (target Cors), error BadRequestException with httpError 400. The model carries no message text, so for the text this is recorded behavior only: a CreateApi whose ProtocolType is not HTTP and which carries a CorsConfiguration is refused HTTP 400 / BadRequestException with the message `Cors is not supported for WEBSOCKET protocolType ` — anatomy: fixed text `Cors is not supported for `, then the ProtocolType taken verbatim from the request, then the fixed text ` protocolType ` INCLUDING the trailing space. Nothing in the message comes from emulator state or from the run, so the whole sentence apart from the protocol type is fixed text that must match byte for byte. The refused call also leaves no API behind (GetApis must not list the requested Name).
Emulator: answers 400 / BadRequestException (status and code correct) with `Cors is not supported for WEBSOCKET protocolType` — the same sentence with the trailing space removed, so an exact-match client never recognises the refusal. The previous fix for this check (cause 4 of the resolved report named above) transcribed the sentence without that trailing space, so the call is still refused with a diverging message.
Why: a client or IaC provider that classifies API Gateway refusals by comparing the message against the documented sentence (the only way to tell this refusal from the generic protocol-restricted-member refusal, since both are 400/BadRequestException) does not match, and reports an unknown provisioning failure instead of "CORS cannot be set on a WEBSOCKET API".
Fix: the shared CORS protocol gate the CreateApi/UpdateApi validation calls — `state::validate_cors_protocol` in the apigatewayv2 crate — which formats this member's own sentence; the format string must end with the trailing space AWS emits.
Traces: apigatewayv2__lambda__logs.websocket-api-create-rejects-a-cors-configuration; emulator source services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/state.rs:1911-1918 (`validate_cors_protocol`, format string `"Cors is not supported for {protocol_type} protocolType"`).

Cause 2 of 2 — an RFC 6455 upgrade to a deployed WEBSOCKET stage is answered 404 with x-amzn-errortype: UnknownService, so the check that first verifies the (now correct) PostToConnection BadRequestException refusal cannot go on to prove that a 400 does not trigger a handler's GoneException-only prune. Failing check: apigatewayv2__lambda__logs.post-to-a-malformed-connection-id-is-rejected-bad-request.
AWS: recorded behavior only (the Smithy model covers only the control plane). An HTTP/1.1 GET carrying `Upgrade: websocket`, `Connection: Upgrade`, `Sec-WebSocket-Key` and `Sec-WebSocket-Version: 13` against a deployed WEBSOCKET API's stage endpoint is answered `HTTP/1.1 101 Switching Protocols` with the derived `Sec-WebSocket-Accept` when $connect authorizes, or a gateway 4xx/5xx otherwise — in every case the request reaches the API, never a service router. ApiGatewayManagementApi PostToConnection then refuses an id the service never issued with 400 BadRequestException (`Invalid connectionId: ` + the ConnectionId from the request), which a handler must log-and-continue on rather than prune, while GoneException (410) is reserved for an issued id whose connection has closed.
Emulator: the front door answers `HTTP/1.1 404 Not Found` with `content-type: application/x-amz-json-1.1` and `x-amzn-errortype: UnknownService` (content-length and date are per-run and are not requirements). The `UnknownService` error type means no service claimed the request, so no stage, route, authorizer or integration was consulted and no connection id is ever issued.
Why: the whole WEBSOCKET data plane is unreachable — a client cannot connect, so $connect/$disconnect/route dispatch, @connections callbacks and every handler behaviour keyed on a live connection are unobservable, and the 400-vs-410 pruning contract cannot be exercised end to end.
Fix: the WEBSOCKET execute-api data-plane seam — the apigatewayv2 crate's `src/service/websocket.rs` resolution plus a front-door/runtime seam that can answer 101 and hand the claiming service the upgraded connection without stripping the handshake's hop-by-hop fields. This is the same root cause as the OPEN report `20261104T160000Z-apigatewayv2-websocket-execute-api-upgrade-remains-unservable.md`, which does NOT carry this check's replay key (that key stayed with the resolved parent report), so this check is unowned and is filed here; closing that report's cause should also clear this one, and this check id must be re-replayed to confirm it.
Traces: apigatewayv2__lambda__logs.post-to-a-malformed-connection-id-is-rejected-bad-request (fails in `_ws_connect`, after its own PostToConnection 400/BadRequestException assertion now passes).

## Evidence
All from the simparity-behavioral-aws-v1 replay against this checkout.

Cause 1 — apigatewayv2__lambda__logs.websocket-api-create-rejects-a-cors-configuration [mismatch]:
  AssertionError: expected 400/BadRequestException/Cors is not supported for WEBSOCKET protocolType, got 400/{'Message': 'Cors is not supported for WEBSOCKET protocolType', 'Code': 'BadRequestException'}
  The check asserts (Code, Message, status) == ("BadRequestException", "Cors is not supported for WEBSOCKET protocolType ", 400) — note the trailing space in the expected message.
  Expected: 400 / BadRequestException / `Cors is not supported for WEBSOCKET protocolType ` (fixed `Cors is not supported for ` + ProtocolType from the request + fixed ` protocolType ` with its trailing space).
  Actual:   400 / BadRequestException / `Cors is not supported for WEBSOCKET protocolType` (trailing space absent).
  A WEBSOCKET positive control without CorsConfiguration is created successfully in the same check, so the divergence is attributable to this member's refusal message alone.

Cause 2 — apigatewayv2__lambda__logs.post-to-a-malformed-connection-id-is-rejected-bad-request [mismatch]:
  File "check_apigatewayv2__lambda__logs_post_to_a_malformed_connection_id_is_rejected_bad_request.py", line 529, in _ws_connect
  AssertionError: the WebSocket upgrade to the prod stage answered 404: 'HTTP/1.1 404 Not Found\r\ncontent-type: application/x-amz-json-1.1\r\nx-amzn-errortype: UnknownService\r\ncontent-length: 149\r\ndate: ...'
  Expected: `HTTP/1.1 101 Switching Protocols` with `Sec-WebSocket-Accept`. Actual: 404 with `x-amzn-errortype: UnknownService`. The host/port, content-length and date on that status line are per-environment and are not requirements; the 404 status and the UnknownService error type are. The assertion sits in a retry loop that treats 5xx as transient, so a 501 from the data plane also spins to the deadline — only a real 101 with frames after it passes.

## Steps to Reproduce
Boot the emulator from this checkout.

Cause 1:
1. aws apigatewayv2 create-api --name t-ws --protocol-type WEBSOCKET --route-selection-expression '$request.body.action' --cors-configuration AllowOrigins=https://example.com
2. Observe 400 BadRequestException with Message 'Cors is not supported for WEBSOCKET protocolType'; AWS answers the same status and code with 'Cors is not supported for WEBSOCKET protocolType ' (trailing space present). Compare the message with byte equality, not a trimmed compare.
3. aws apigatewayv2 get-apis and confirm no API named t-ws was left behind (this part already holds).

Cause 2:
1. Create a WEBSOCKET API with $connect/$disconnect/$default routes backed by a Lambda AWS_PROXY integration, create a stage 'prod' and deploy it.
2. Open a TCP/TLS socket to the stage's execute-api endpoint and send `GET /prod HTTP/1.1` with `Upgrade: websocket`, `Connection: Upgrade`, `Sec-WebSocket-Key: <base64 16 bytes>`, `Sec-WebSocket-Version: 13`.
3. Observe `HTTP/1.1 404 Not Found` with `x-amzn-errortype: UnknownService` where AWS answers `101 Switching Protocols`.

Then re-replay both check ids listed in trace_ids.
