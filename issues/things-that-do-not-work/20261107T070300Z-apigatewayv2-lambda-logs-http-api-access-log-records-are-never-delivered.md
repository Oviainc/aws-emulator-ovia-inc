Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__lambda__logs.access-log-record-joins-the-gateway-request-id apigatewayv2__lambda__logs.direct-invoke-returns-handler-status-codes-without-a-function-error
Issue-Key: apigatewayv2__lambda__logs.access-log-record-joins-the-gateway-request-id|_poll_until#28|Timeout|time.sleep(interval_s)
Issue-Key: apigatewayv2__lambda__logs.direct-invoke-returns-handler-status-codes-without-a-function-error|_log_messages#9|Timeout|time.sleep(4)
Issue-Commit: 67bb1f0baca61c1b5b95e0062b2898063e06b421

# HTTP API access-log records are never delivered, and a direct Lambda Invoke of the scenario handler reaches no verdict (2 causes)

> _Note: this issue was identified against the code base at commit `67bb1f0baca61c1b5b95e0062b2898063e06b421`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Two net-new root causes. Both checks are killed by the harness at its 60 s cap, and the suite reports only `Check timed out after 60.0s` for each; what each was waiting for is below.

Cause 1 of 2 — an apigatewayv2 stage configured with AccessLogSettings delivers no access-log record, so the record can never be joined to the request id the gateway returned to the client.
AWS: a stage whose `AccessLogSettings` carries a `DestinationArn` naming a CloudWatch Logs log group and a `Format` naming `$context.requestId` writes exactly one access-log record per request into that group, and the `requestId` in the record equals the value the gateway reported for that same request (the `x-amzn-RequestId`/`apigw-requestid` the client received, and the id the integration can read from `requestContext.requestId`). The modeled contract is `AccessLogSettings{Format, DestinationArn}` on the stage; the content of the record is whatever the `Format` string renders, so the join key is the request's own id, not any fixed text — recorded behavior only for the delivery itself.
Emulator: no access-log record is delivered to the destination log group, so the check's poll for a record carrying the gateway's request id never terminates and the harness kills it at 60 s. The correlation between a served request and its access-log record is therefore unobservable.
Why: access logs are the only server-side record of an HTTP API request; a client that configures a destination and a `$context.requestId` format gets an empty log group, cannot correlate a client-visible request id with server-side state, and cannot build any request-level auditing or troubleshooting on it.
Fix: the apigatewayv2 execute-api HTTP data plane's access-log emitter — it must render the stage's `AccessLogSettings.Format` per request and PutLogEvents it into the group named by `DestinationArn`, using the same request id it returned to the client. Related open issue 20261029T182816Z-apigatewayv2-lambda-logs-the-access-log-request-id.md describes the same symptom for a different check and does not carry this check's replay key.
Traces: apigatewayv2__lambda__logs.access-log-record-joins-the-gateway-request-id.

Cause 2 of 2 — a direct `Invoke` of the scenario's Node handler never yields the handler-reported status codes and the caught exception's stack trace, so the "handler status code is not a function error" contract reaches no verdict.
AWS: Smithy `Invoke` (POST /2015-03-31/functions/{FunctionName}/invocations) answers `StatusCode` on the HTTP response code, `FunctionError` in the `X-Amz-Function-Error` header and the handler's return value as the `Payload` httpPayload. A handler that RETURNS `{"statusCode": 400, "body": "Unknown action"}` for an unknown action is a successful invocation: StatusCode 200, NO `FunctionError`, and that object verbatim in Payload. A handler that CATCHES its own exception and returns `{"statusCode": 500, "body": "Internal server error"}` is likewise StatusCode 200 with no `FunctionError` — a handler-chosen 500 is not a function error. The caught exception's stack trace still reaches the function's declared CloudWatch log group, naming the frame that raised (here the `handleHeartbeat` path) and the runtime's own `reading 'replace'` TypeError text; the frame names and message are values from the handler's code and the thrown error, not fixed text.
Emulator: the check reaches no verdict inside the harness budget — of the two `Invoke` calls and the subsequent `FilterLogEvents` poll for the logged stack trace, one never answers as AWS does, so the check runs to the 60 s cap. Nothing in the emulator's response set distinguishes the handler's own 400/500 from a function error, and the stack trace is not readable from logs.
Why: a client cannot tell a handler-reported HTTP status from a Lambda function error — the two are opposite outcomes for retry, alarms and dead-letter routing — and cannot read the handler's stack trace from CloudWatch to diagnose the caught exception.
Fix: the lambda `Invoke` handler (StatusCode/FunctionError/Payload framing for a handler that returns normally) and the runtime's stdout/stderr → CloudWatch Logs delivery into the function's declared log group. Related open issue 20261106T052537Z-apigatewayv2-lambda-logs-the-nodejs22-x-lambda-runtime.md records that the nodejs22.x runtime ships no AWS SDK for JavaScript v3, which this handler requires at module load; that issue does not carry this check's replay key.
Traces: apigatewayv2__lambda__logs.direct-invoke-returns-handler-status-codes-without-a-function-error.

## Evidence
access-log-record-joins-the-gateway-request-id — Error: Check timed out after 60.0s.
Expected: one access-log record per request in the log group named by the stage's `AccessLogSettings.DestinationArn`, rendered from its `Format`, whose `$context.requestId` equals the request id the gateway returned to the client.
Actual: no record is delivered, so the correlation poll never terminates.

direct-invoke-returns-handler-status-codes-without-a-function-error — Error: Check timed out after 60.0s.
Expected: `Invoke` with an unknown action returns (StatusCode 200, FunctionError absent, Payload {"statusCode": 400, "body": "Unknown action"}); `Invoke` on the path whose handler catches its own exception returns (StatusCode 200, FunctionError absent, Payload {"statusCode": 500, "body": "Internal server error"}); and the function's declared log group holds the caught stack trace naming the raising frame (`handleHeartbeat`) and the `reading 'replace'` TypeError.
Actual: no verdict within the harness budget.

## Steps to Reproduce
Cause 1:
1. CreateApi (apigatewayv2) with a route backed by an AWS_PROXY Lambda integration; CreateStage with `AccessLogSettings` = {DestinationArn: <pre-created log group ARN>, Format: a JSON line including `$context.requestId`}; deploy it.
2. Send a request to the stage's invoke URL and record the request id the gateway returns to the client.
3. FilterLogEvents on the destination log group: AWS holds one record per request whose `requestId` is that id; the emulator holds none.

Cause 2:
1. CreateFunction with a Node handler that, for one input, RETURNS `{statusCode: 400, body: 'Unknown action'}` and, for another, catches an exception it raises and RETURNS `{statusCode: 500, body: 'Internal server error'}`, with a declared CloudWatch log group.
2. Call `Invoke` (RequestResponse) with each input and read StatusCode, FunctionError and Payload — both must be StatusCode 200 with no FunctionError and the handler's object as Payload.
3. FilterLogEvents on the declared group for the caught exception's stack: AWS shows the trace naming the raising frame and the TypeError text; the emulator does not reach either assertion inside the budget.
