Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__cloudwatch.obs-apigatewayv2-3-http-api-5xx-alarm-actions-disabled-notification-blocked apigatewayv2__cloudwatch.obs-apigatewayv2-3-http-api-5xx-alarm-notifies-sqs-via-sns
Issue-Key: apigatewayv2__cloudwatch.obs-apigatewayv2-3-http-api-5xx-alarm-actions-disabled-notification-blocked|run#25|AssertionError|assert text == marker, (
Issue-Key: apigatewayv2__cloudwatch.obs-apigatewayv2-3-http-api-5xx-alarm-notifies-sqs-via-sns|run#25|AssertionError|assert text == marker, (
Issue-Commit: ec258a82c643322f3f1b10a7ea424abc4c176038

# HTTP API returns its own generic '{"message":"Internal Server Error"}' body instead of the integration's 5xx response, so 5XX alarm checks cannot attribute the error

> _Note: this issue was identified against the code base at commit `ec258a82c643322f3f1b10a7ea424abc4c176038`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
This issue carries 1 net-new root cause.

Cause 1 of 1: an HTTP API route whose integration deliberately answers a 5xx has that response replaced by the gateway's own canned error body.
AWS: recorded behavior only (the apigatewayv2 model does not describe data-plane bodies). When a Lambda proxy integration returns a 500 with its own body, API Gateway passes the integration's statusCode and BODY through to the caller; the gateway only substitutes `{"message":"Internal Server Error"}` when the integration itself failed (no response, malformed proxy response, or permission failure). The checks assert the settling 500 carries the integration's marker string so the 5XXError datapoint they then alarm on is attributable to this check's own traffic.
Emulator: the response body is the gateway's canned `{"message":"Internal Server Error"}`, with the integration's marker (`sim2real-xsvc-5xx-<id>`) absent — so the emulator is answering on the integration's behalf rather than returning what the integration produced.
Why: a client cannot tell a genuine integration 5xx from a gateway-side failure, and any error-triage or alarm-attribution logic keyed on the integration's own error payload breaks. Both the "alarm notifies SQS via SNS" path and its actions-disabled twin are blocked on this, so the 5XXError alarm behaviour behind it is unverifiable.
Fix: the apigatewayv2 HTTP-API data-plane proxy-response handling — pass the integration's statusCode and body through, and reserve the canned body for integration failures.
Traces: cloudwatch__apigatewayv2.obs-apigatewayv2-3-http-api-5xx-alarm-notifies-sqs-via-sns, cloudwatch__apigatewayv2.obs-apigatewayv2-3-http-api-5xx-alarm-actions-disabled-notification-blocked.

## Evidence
Replay output:

cloudwatch__apigatewayv2.obs-apigatewayv2-3-http-api-5xx-alarm-notifies-sqs-via-sns —
  AssertionError: the settling 500 must come from this check's integration (marker 'sim2real-xsvc-5xx-8f03fbb8bf0b'), got '{"message":"Internal Server Error"}'

cloudwatch__apigatewayv2.obs-apigatewayv2-3-http-api-5xx-alarm-actions-disabled-notification-blocked —
  AssertionError: the settling 500 must come from this check's integration (marker 'sim2real-xsvc-5xx-fd5d0b1eaa44'), got '{"message":"Internal Server Error"}'

Expected: HTTP 500 whose body is the integration's own payload containing the per-run marker. Actual: HTTP 500 with the gateway's canned body. (The marker value itself is per-run; the requirement is that the body is the integration's, not the gateway's.)

## Steps to Reproduce
Replay the two check ids. Each deploys an HTTP API with a Lambda proxy integration that returns statusCode 500 and a body containing a unique marker, invokes the route over HTTP until the response settles, then asserts the body is the integration's before alarming on the AWS/ApiGateway 5XXError series.
