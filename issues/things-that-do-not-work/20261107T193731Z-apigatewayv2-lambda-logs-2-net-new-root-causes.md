Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__lambda__logs.access-log-record-joins-the-gateway-request-id apigatewayv2__lambda__logs.direct-invoke-returns-handler-status-codes-without-a-function-error
Issue-Key: apigatewayv2__lambda__logs.access-log-record-joins-the-gateway-request-id|_poll_until#28|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: apigatewayv2__lambda__logs.direct-invoke-returns-handler-status-codes-without-a-function-error|_log_messages#9|AssertionError|assert time.monotonic() < deadline, (
Issue-Commit: c3a6f169c346304d6b4c388bf8378b1c143b9d70

# apigatewayv2__lambda__logs: 2 net-new root causes — HTTP API access-log records never reach the configured log group, and a Lambda handler's caught-exception stack trace never reaches /aws/lambda/<fn>

> _Note: this issue was identified against the code base at commit `c3a6f169c346304d6b4c388bf8378b1c143b9d70`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStage (AccessLogSettings) + execute-api GET; Invoke + FilterLogEvents

## Description
Two root causes observed under the apigatewayv2__lambda__logs checks. Related open issue that does not carry these checks' replay keys: 20261107T070300Z-apigatewayv2-lambda-logs-http-api-access-log-records-are-never-delivered.md (and 20261029T182816Z-apigatewayv2-lambda-logs-the-access-log-request-id.md).

Cause 1 of 2: a stage with AccessLogSettings emits no access log record at all.
AWS: when a stage carries AccessLogSettings (DestinationArn + Format), every served request appends one record to that log group, and the record's $context.requestId equals the gateway request id the integration saw — so a client that reads the id back from the integration can join to the access log.
Emulator: after two served requests whose integration-observed gateway request ids were known, the configured log group held zero matching records for 120 s (last result: []), i.e. the access-log sink is never written.
Why: request-level observability is absent — nothing can be correlated between a client, its integration and the gateway, and any alarm/metric-filter built on access logs never fires.
Fix: the execute-api data plane's access-log emitter for a stage's AccessLogSettings (format rendering of $context.requestId and friends) and its delivery into the logs service.
Traces: apigatewayv2__lambda__logs.access-log-record-joins-the-gateway-request-id.

Cause 2 of 2: a handler's caught-exception stack trace never reaches the function's log group.
AWS: a direct Invoke returns the handler's own envelope in Payload with no FunctionError when the handler catches its exception, and everything the handler wrote to stdout/stderr — including the traceback text of the caught exception — is delivered to /aws/lambda/<function-name> and is readable with FilterLogEvents.
Emulator: the function's log group held 0 events matching the traceback text (the AttributeError message raised and caught inside the handler) after 120 s, where at least one was expected — the handler's stderr/traceback is not captured into the log group. The requirement is that handler-written log output lands in the declared group; the function name, group name and the exception text are per-run values.
Why: a client debugging a Lambda cannot see caught-exception tracebacks or any handler log output, so error diagnosis and log-based assertions are impossible even though the invoke itself succeeds.
Fix: the lambda invoke path's log capture — the runtime's stdout/stderr must be written as log events into the function's /aws/lambda/<name> group before the invoke returns.
Traces: apigatewayv2__lambda__logs.direct-invoke-returns-handler-status-codes-without-a-function-error.

## Evidence
Cause 1: "access log records in /sim2real/xsvc/<group> for both integration-observed request ids ['18e5d59d2b41abee', '86d2ac241600b9db'] not met within 120.0s. Last result: []" (ids are per-run values; the requirement is that one record per served request exists and carries the gateway request id).
Cause 2: "log group '/aws/lambda/<fn>' holds 0 event(s) matching \"reading 'replace'\" after 120s, expected at least 1".

## Steps to Reproduce
Replay the two checks in trace_ids. Cause 1: create an HTTP API with a Lambda integration, UpdateStage with AccessLogSettings pointing at a CloudWatch Logs group with a $context format including requestId, send two requests that echo back $context.requestId, then FilterLogEvents on the group for those ids. Cause 2: Invoke the scenario function directly with an event whose handler catches an AttributeError, then FilterLogEvents on /aws/lambda/<fn> for the traceback text.
