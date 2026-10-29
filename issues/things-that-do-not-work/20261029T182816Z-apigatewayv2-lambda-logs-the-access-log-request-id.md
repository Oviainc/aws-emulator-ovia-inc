Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__lambda__logs.access-log-record-joins-the-gateway-request-id
Issue-Key: apigatewayv2__lambda__logs.access-log-record-joins-the-gateway-request-id|_poll_http#21|Timeout|time.sleep(interval_s)
Issue-Commit: ec258a82c643322f3f1b10a7ea424abc4c176038

# apigatewayv2__lambda__logs: the access-log/request-id correlation check hangs to the 60 s cap because HTTP API access logs are never delivered (1 cause)

> _Note: this issue was identified against the code base at commit `ec258a82c643322f3f1b10a7ea424abc4c176038`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStage

## Description
One root cause. Cause count: 1.

=== Cause 1 of 1: an HTTP API stage with AccessLogSettings delivers no access-log record, so the request-id join can never be made ===
AWS: an apigatewayv2 stage with AccessLogSettings{DestinationArn, Format} writes one JSON access-log record per request into the destination log group, and the record's $context.requestId equals the x-amzn-RequestId the caller saw. Recorded behavior only (access logging is a data-plane behavior, not modeled in Smithy).
Emulator: the check is terminated with 'Error: Check timed out after 60.0s' — the destination log group receives no record at all, so the bounded poll for the record carrying the gateway request id never completes.
Why: access logging on HTTP APIs is inert: a client cannot correlate a response with its access-log record, and anything that waits for the record (log-based tracing, request auditing) hangs rather than failing.
Fix: the apigatewayv2 stage access-log emitter in the HTTP API data plane — render the configured Format and deliver one event per request to the stage's DestinationArn.
Traces: apigatewayv2__lambda__logs.access-log-record-joins-the-gateway-request-id.

## Evidence
Failing check apigatewayv2__lambda__logs.access-log-record-joins-the-gateway-request-id (verdict: mismatch). Replay detail: 'Error: Check timed out after 60.0s'. The check creates an HTTP API, sets AccessLogSettings with a JSON format containing $context.requestId, drives a request, captures the response's request id and then polls the destination log group for the matching record. Expected (AWS): exactly one access-log record whose requestId equals the response's; actual: the log group stays empty until the harness deadline.

## Steps to Reproduce
1. apigatewayv2 CreateApi ProtocolType=HTTP with a Lambda/HTTP integration; CreateStage (or UpdateStage) with AccessLogSettings{DestinationArn=<log group ARN>, Format='{"requestId":"$context.requestId"}'}.
2. Invoke the stage's endpoint and note the x-amzn-RequestId of the response.
3. Poll logs for events in the destination log group. Observed: no event ever arrives (check killed at 60 s). Expected: one JSON record whose requestId equals the response's request id.
