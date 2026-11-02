Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__logs.obs-apigatewayv2-3-websocket-access-log-connect-message-disconnect apigatewayv2__logs.obs-apigatewayv2-3-websocket-connect-rejected-logs-connect-only
Issue-Key: apigatewayv2__logs.obs-apigatewayv2-3-websocket-access-log-connect-message-disconnect|_await_log_delivery#16|Timeout|time.sleep(5.0)
Issue-Key: apigatewayv2__logs.obs-apigatewayv2-3-websocket-connect-rejected-logs-connect-only|_await_log_delivery#12|Timeout|time.sleep(5.0)
Issue-Commit: ec258a82c643322f3f1b10a7ea424abc4c176038

# logs__apigatewayv2: both WebSocket access-log checks hang to the 60 s cap because the stage's access logs are never delivered (1 cause)

> _Note: this issue was identified against the code base at commit `ec258a82c643322f3f1b10a7ea424abc4c176038`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStage

## Description
One root cause. Cause count: 1.

=== Cause 1 of 1: a WEBSOCKET apigatewayv2 stage with AccessLogSettings delivers no access-log record, so both checks poll to the deadline ===
AWS: a WEBSOCKET stage with AccessLogSettings writes one access-log record per route execution into the configured log group — $connect, the message route and $disconnect for a completed session, and only the $connect record when the connection is rejected. The checks poll the group for that record sequence. Recorded behavior only.
Emulator: both checks are terminated with 'Error: Check timed out after 60.0s'. No access-log record is ever delivered, so neither the three-record sequence nor the connect-only sequence can be observed.
Why: WebSocket access logging is inert: a client cannot audit connect/disconnect or distinguish a rejected handshake from a completed session, and any wait on the log record hangs instead of returning.
Fix: the apigatewayv2 WEBSOCKET stage access-log emitter — render the stage's Format per route execution ($connect, routes, $disconnect) and deliver to the stage's DestinationArn.
Traces: logs__apigatewayv2.obs-apigatewayv2-3-websocket-access-log-connect-message-disconnect, logs__apigatewayv2.obs-apigatewayv2-3-websocket-connect-rejected-logs-connect-only.

## Evidence
Both failing checks (verdict: mismatch) report exactly 'Error: Check timed out after 60.0s'. Each creates a WEBSOCKET API with AccessLogSettings pointing at a log group, drives a session (one completed, one rejected at $connect) and polls the log group for the access-log records. Expected (AWS): three records for the completed session, one $connect record for the rejected connection. Actual: the log group stays empty until the harness deadline.

## Steps to Reproduce
1. apigatewayv2 CreateApi ProtocolType=WEBSOCKET with $connect/$disconnect and a message route; CreateStage with AccessLogSettings{DestinationArn=<log group ARN>, Format=<JSON with $context.requestId and $context.routeKey>}; CreateDeployment.
2. Open a WebSocket connection to the stage, send one message, close it; separately, open a connection the $connect route rejects.
3. Poll the destination log group for the access-log records.
   Observed: no record ever arrives (both checks killed at 60 s).
   Expected: $connect + message + $disconnect records for the first session, $connect only for the rejected one.
