Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__events__sqs.xsvc-apigateway-1-near-miss-detail-type-is-not-delivered-to-rest-stage apigateway__events__sqs.xsvc-apigateway-1-rule-delivers-transformed-detail-with-path-and-header-to-rest-stage apigateway__events__sqs.xsvc-apigateway-1-stage-500-is-retried-then-dead-lettered-with-error-code apigateway__events__sqs.xsvc-apigateway-1-target-role-without-execute-api-invoke-is-denied
Issue-Key: apigateway__events__sqs.xsvc-apigateway-1-near-miss-detail-type-is-not-delivered-to-rest-stage|_poll_signed_post#7|Timeout|time.sleep(5)
Issue-Key: apigateway__events__sqs.xsvc-apigateway-1-rule-delivers-transformed-detail-with-path-and-header-to-rest-stage|_poll_signed_post#7|Timeout|time.sleep(5)
Issue-Key: apigateway__events__sqs.xsvc-apigateway-1-stage-500-is-retried-then-dead-lettered-with-error-code|_poll_signed_post#8|Timeout|time.sleep(5)
Issue-Key: apigateway__events__sqs.xsvc-apigateway-1-target-role-without-execute-api-invoke-is-denied|_poll_signed_post#8|Timeout|time.sleep(5)
Issue-Commit: 8bb3e31d2bc3a99611966740f58be573e66087fa

# apigateway__events__sqs: all four EventBridge-to-REST-stage checks hang to the 60 s cap

> _Note: this issue was identified against the code base at commit `8bb3e31d2bc3a99611966740f58be573e66087fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeployment

## Description
Cause 1 of 1: a request delivered to a deployed REST API stage never returns, so no EventBridge-to-API-Gateway delivery can complete.
AWS: an EventBridge rule whose target is a deployed REST API stage delivers the (optionally transformed) event to the stage, which answers with the integration's response; a matching rule delivers and a near-miss detail-type does not; a stage that answers 5xx causes EventBridge to retry and then dead-letter the event with an error code; a target role without execute-api:Invoke is denied. Recorded behavior only for the per-case statuses; what is structural is that every delivery to a deployed stage produces an HTTP response.
Emulator: no response arrives at the stage. All four checks exhaust the harness budget and are reported as "Check timed out after 60.0s".
Why: EventBridge's delivery, retry and dead-letter behaviour against an API Gateway target cannot be observed, and the positive and negative legs are indistinguishable because neither ever completes.
Fix: the execute-api data-plane request path — stage/route resolution and integration dispatch must always produce a response.
Traces: apigateway__events__sqs.xsvc-apigateway-1-near-miss-detail-type-is-not-delivered-to-rest-stage, apigateway__events__sqs.xsvc-apigateway-1-rule-delivers-transformed-detail-with-path-and-header-to-rest-stage, apigateway__events__sqs.xsvc-apigateway-1-stage-500-is-retried-then-dead-lettered-with-error-code, apigateway__events__sqs.xsvc-apigateway-1-target-role-without-execute-api-invoke-is-denied.

## Evidence
Detail for all four checks is exactly "Error: Check timed out after 60.0s".

## Steps to Reproduce
1. Deploy a REST API stage, create an EventBridge rule targeting it with an input transformer and a DLQ, and put a matching event.
2. Observe that the stage never produces a response and the check exhausts its 60 s budget.
3. Or replay the check ids in trace_ids.
