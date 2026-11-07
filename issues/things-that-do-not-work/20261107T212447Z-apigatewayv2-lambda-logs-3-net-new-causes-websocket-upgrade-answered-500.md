Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__lambda__logs.connect-without-an-invoke-permission-is-rejected-with-500 apigatewayv2__lambda__logs.access-log-record-joins-the-gateway-request-id apigatewayv2__lambda__logs.direct-invoke-returns-handler-status-codes-without-a-function-error
Issue-Key: apigatewayv2__lambda__logs.connect-without-an-invoke-permission-is-rejected-with-500|_ws_connect#7|AssertionError|assert status >= 500 and time.monotonic() < deadline, (
Issue-Key: apigatewayv2__lambda__logs.access-log-record-joins-the-gateway-request-id|_poll_until#28|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: apigatewayv2__lambda__logs.direct-invoke-returns-handler-status-codes-without-a-function-error|_log_messages#9|AssertionError|assert time.monotonic() < deadline, (
Issue-Commit: a5c62b28646ac18350a618623d28fc3117c37127

# 3 net-new causes: WebSocket upgrade answered 500, stage access logs never delivered, function log events missing

> _Note: this issue was identified against the code base at commit `a5c62b28646ac18350a618623d28fc3117c37127`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeployment

## Description
Three net-new root causes. The open issue 20261104T160000Z-apigatewayv2-websocket-execute-api-upgrade-remains-unservable.md covers other checks in this service; none of the three checks below carry its replay key.

Cause 1 of 3 — the WebSocket upgrade to a deployed stage is answered HTTP 500.
AWS: recorded behavior only. An RFC 6455 upgrade to a deployed WebSocket stage is answered `HTTP/1.1 101 Switching Protocols`; when $connect's target grants API Gateway no invoke permission the handshake is refused — the recording's own expectation for that leg is a refusal, not a server fault on the success leg.
Emulator: every upgrade is answered `HTTP/1.1 500 Internal Server Error` with body `{"message":"Internal Server Error"}`. Replay detail: `AssertionError: the WebSocket upgrade to the prod stage answered 500: 'HTTP/1.1 500 Internal Server Error\r\ncontent-type: application/json\r\ncontent-length: 35...'`.
Why: a client can never open a connection, and a 5xx hides whether the cause was a missing permission (a client error) or a service fault, so retry and alarm logic keyed on the class is wrong.
Fix: the execute-api WebSocket data plane — the stage handler that must perform the Upgrade handshake, and the $connect permission-denied path that must answer a client error rather than 500.
Traces: apigatewayv2__lambda__logs.connect-without-an-invoke-permission-is-rejected-with-500.

Cause 2 of 3 — stage access-log records are never written to the configured log group.
AWS: recorded behavior only. With AccessLogSettings on the stage, API Gateway writes one access-log record per served request into the destination log group, and that record carries the gateway request id the integration also observed, so the two can be joined.
Emulator: the destination group stays empty. Replay detail: `AssertionError: access log records in /sim2real/xsvc/... for both integration-observed request ids ['18e5db12832563e5', '86d2a2abbc5282b9'] not met within 120.0s. Last result: []`.
Why: a client cannot correlate a gateway request id with its integration invocation — the join key API Gateway publishes is simply absent, so request tracing and billing/audit pipelines built on access logs see nothing.
Fix: the API Gateway v2 stage request path — emission of one access-log record per request to the stage's AccessLogSettings destination, including $context.requestId.
Traces: apigatewayv2__lambda__logs.access-log-record-joins-the-gateway-request-id.

Cause 3 of 3 — a function's own CloudWatch log group never receives the handler's error output.
AWS: recorded behavior only. A directly invoked handler that raises writes the exception text into /aws/lambda/<function> within seconds, so it is findable with FilterLogEvents.
Emulator: the group holds no matching event. Replay detail: `AssertionError: log group '/aws/lambda/sim2real-flint-msg-...' holds 0 event(s) matching "reading 'replace'" after 120s, expected at least 1`.
Why: a client debugging a handler failure finds nothing in the function's log group, so the only record of why an invocation failed is lost.
Fix: the Lambda invoke path's log delivery into the function's CloudWatch Logs group — handler stdout/stderr and unhandled-exception text must be written as log events.
Traces: apigatewayv2__lambda__logs.direct-invoke-returns-handler-status-codes-without-a-function-error.

## Evidence
Cause 1 — expected `HTTP/1.1 101 Switching Protocols` (or a client-error refusal on the ungranted leg); actual `HTTP/1.1 500 Internal Server Error` with body {"message":"Internal Server Error"}.
Cause 2 — expected one access-log record per request in the AccessLogSettings destination, carrying the gateway request id; actual `Last result: []` after 120 s.
Cause 3 — expected at least 1 event in /aws/lambda/<function> matching the raised exception text; actual 0 events after 120 s.

## Steps to Reproduce
Cause 1: create a WebSocket API with $connect bound to a function that has no apigateway invoke permission, deploy to a stage, send an RFC 6455 upgrade — the response is HTTP 500.
Cause 2: set AccessLogSettings on the stage to a CloudWatch Logs group, serve two requests, then FilterLogEvents on that group — it stays empty.
Cause 3: Invoke a function whose handler raises, then FilterLogEvents on /aws/lambda/<function> for the exception text — no matching event.
