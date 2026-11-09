Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__events__sqs.xsvc-apigateway-1-stage-500-is-retried-then-dead-lettered-with-error-code
Issue-Key: apigateway__events__sqs.xsvc-apigateway-1-stage-500-is-retried-then-dead-lettered-with-error-code|_poll_signed_post#8|AssertionError|raise AssertionError(f"POST {url} never returned {expected}; last={last!r}")
Issue-Commit: 39b863d101414613449d16e44f0b23fd775a8d87

# apigateway__events__sqs: an EventBridge-delivered request to a REST stage is answered 500, so the retry-then-dead-letter contract is unreachable

> _Note: this issue was identified against the code base at commit `39b863d101414613449d16e44f0b23fd775a8d87`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
Cause 1 of 1: a POST to a deployed REST API stage never returns 200, so an EventBridge rule whose target is that stage can never be observed delivering, retrying or dead-lettering.
AWS: an EventBridge rule with an API-destination/execute-api target POSTs the (optionally transformed) event to the stage path; the stage answers from its integration. When the stage answers 5xx, EventBridge retries per the target's RetryPolicy and then writes the event to the target's dead-letter queue with the delivery error code recorded in the message attributes.
Emulator: the stage itself answers HTTP 500 with the default gateway response {"message":"Internal server error"} for every POST, including the control request the check uses to confirm the stage works at all. The stage never returns 200, so the retry/dead-letter path is never reached and the "stage 500 is retried then dead-lettered" contract cannot be graded.
Why: a caller cannot tell a genuine 5xx-driven dead-letter from a stage that never serves; every EventBridge->API Gateway pipeline is effectively dead, and the DLQ record with its error code is never produced.
Fix: the execute-api invoke path's integration execution for the deployed stage (the unconditional 500 must be removed before the EventBridge retry/DLQ behaviour can be exercised).
Traces: apigateway__events__sqs.xsvc-apigateway-1-stage-500-is-retried-then-dead-lettered-with-error-code

## Evidence
"AssertionError: POST https://<host>/_aws/execute-api/<apiId>/v1/accepted/w-1 never returned 200; last=(500, '{\"message\":\"Internal server error\"}')". Expected: 200 from the stage for the control request, then a deliberate 5xx leg retried and dead-lettered with the delivery error code on the SQS message. Host/port/api id are per-run values.

## Steps to Reproduce
1. Create a REST API with POST /v1/accepted/{id} over a working integration and deploy it.
2. POST directly to the stage URL and confirm 200.
3. Create an EventBridge rule targeting the stage with a RetryPolicy and an SQS dead-letter queue; PutEvents a matching event against a leg that answers 5xx.
Expected: step 2 returns 200, and the 5xx leg is retried then dead-lettered with the error code on the DLQ message. Actual: step 2 returns 500 {"message":"Internal server error"}.
