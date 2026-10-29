Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__lambda__logs.apigw-method-level-log-level-overrides-the-stage-wide-level apigateway__lambda__logs.obs-apigateway-2-execution-log-level-and-data-trace-gate-delivered-lines apigateway__lambda__logs.apigw-lambda-authorizer-deny-blocks-backend-invocation apigateway__lambda__logs.apigw-stage-access-logs-delivered-to-log-group
Issue-Key: apigateway__lambda__logs.apigw-method-level-log-level-overrides-the-stage-wide-level|run#34|AssertionError|assert status == 200, f"{verb} /echo returned {status}, expected 200"
Issue-Key: apigateway__lambda__logs.obs-apigateway-2-execution-log-level-and-data-trace-gate-delivered-lines|_poll#30|AssertionError|raise AssertionError(f"timed out after {timeout_s}s waiting for {what}; last={last!r}")
Issue-Key: apigateway__lambda__logs.apigw-lambda-authorizer-deny-blocks-backend-invocation|_poll_http#19|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__lambda__logs.apigw-stage-access-logs-delivered-to-log-group|_poll_http#12|Timeout|time.sleep(interval_s)
Issue-Commit: ec258a82c643322f3f1b10a7ea424abc4c176038

# apigateway__lambda__logs: proxy integrations answer 500, the execution log group is never created, and two logging checks hang to the 60 s cap (3 causes)

> _Note: this issue was identified against the code base at commit `ec258a82c643322f3f1b10a7ea424abc4c176038`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStage

## Description
Three root causes. Cause count: 3.

=== Cause 1 of 3: a request through a deployed stage with a Lambda proxy integration answers 500 instead of 200 ===
AWS: a deployed stage whose method has an AWS_PROXY integration to an existing, permitted function answers 200 with the function's payload. Recorded behavior only.
Emulator: 'AssertionError: GET /echo returned 500, expected 200' — the integration cannot be executed, so the API's API_CONFIGURATION_ERROR gateway response (HTTP 500, body {"message":"Internal server error"}) is served for every request.
Why: the API is unusable; and because the error is identical to the one AWS emits for a genuinely broken integration, a client cannot distinguish "backend not runnable here" from "integration misconfigured".
Fix: the AWS_PROXY integration execution stage of the method-invoke pipeline (the API Gateway face of open issue 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md, whose replay keys do not cover this check).
Traces: apigateway__lambda__logs.apigw-method-level-log-level-overrides-the-stage-wide-level.

=== Cause 2 of 3: enabling execution logging never creates the stage's execution log group ===
AWS: once a stage has loggingLevel ERROR or INFO, API Gateway creates the log group API-Gateway-Execution-Logs_<api-id>/<stage> and delivers the level's lines to it. Recorded behavior only.
Emulator: the group never comes into existence — 'AssertionError: timed out after 30s waiting for execution log group "API-Gateway-Execution-Logs_<api-id>/err" to exist after enabling logging (before any request); last=None'.
Why: a client that reads the execution log group to confirm logging is on, or that attaches a retention policy/subscription filter to it, finds nothing; the per-level line-delivery contract cannot be observed at all.
Fix: the stage logging configuration path (UpdateStage MethodSettings) plus the execution-log emitter: provision API-Gateway-Execution-Logs_<api-id>/<stage> when logging is enabled and write the level-appropriate lines.
Traces: apigateway__lambda__logs.obs-apigateway-2-execution-log-level-and-data-trace-gate-delivered-lines.

=== Cause 3 of 3: the authorizer-deny and stage-access-log checks hang to the 60 s harness cap ===
AWS: a REQUEST/TOKEN Lambda authorizer that denies answers 403 without invoking the backend, and a stage with accessLogSettings delivers one access-log record per request to the configured log group; both checks poll for that observable and return.
Emulator: both are terminated with 'Error: Check timed out after 60.0s'. No access-log record is delivered and the authorizer/backend decision never materialises, so the bounded polls run to the deadline.
Why: access logging is inert and the authorizer gate is unobservable — a client waiting on an access-log record or on a deny-before-backend effect hangs instead of getting an answer.
Fix: the stage access-log delivery emitter and the authorizer stage of the method-invoke pipeline.
Traces: apigateway__lambda__logs.apigw-lambda-authorizer-deny-blocks-backend-invocation, apigateway__lambda__logs.apigw-stage-access-logs-delivered-to-log-group.

## Evidence
Cause 1 — replay: 'AssertionError: GET /echo returned 500, expected 200' (check apigw-method-level-log-level-overrides-the-stage-wide-level). Expected (AWS): 200 with the proxy function's payload.
Cause 2 — replay: 'AssertionError: timed out after 30s waiting for execution log group "API-Gateway-Execution-Logs_8sh3tqaaaa/err" to exist after enabling logging (before any request); last=None' (the api id in that string is this run's generated value; the group-name template API-Gateway-Execution-Logs_<api-id>/<stage> is the requirement).
Cause 3 — replay: 'Error: Check timed out after 60.0s' for both apigw-lambda-authorizer-deny-blocks-backend-invocation and apigw-stage-access-logs-delivered-to-log-group; the unsatisfied condition in each is the arrival of the access-log record / the deny decision's effect.

## Steps to Reproduce
Cause 1: create a REST API with an AWS_PROXY integration to an existing function, grant apigateway.amazonaws.com lambda:InvokeFunction, deploy, GET the method path. Observe 500 {"message":"Internal server error"}; expected 200.
Cause 2: UpdateStage patch replace /*/*/logging/loggingLevel=ERROR on a deployed stage, then poll logs DescribeLogGroups for API-Gateway-Execution-Logs_<api-id>/<stage>. Observe the group never appears.
Cause 3: (a) attach a Lambda authorizer that returns Deny and call the method — expected 403 with no backend invocation; (b) set accessLogSettings on the stage, send a request, poll the destination log group for the access-log record — observe nothing arrives within the 60 s cap.
