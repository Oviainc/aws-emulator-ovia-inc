Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda__logs.apigateway-execution-logging-to-cloudwatch-positive apigateway__dynamodb__lambda__logs.apigateway-missing-cloudwatch-role-negative apigateway__dynamodb__lambda__logs.apigw-stage-logging-level-error-only apigateway__dynamodb__lambda__logs.ddb-esm-lambda-error-logged-to-cloudwatch-positive apigateway__dynamodb__lambda__logs.cli-apigateway-missing-deployment-stage-negative
Issue-Key: apigateway__dynamodb__lambda__logs.apigateway-execution-logging-to-cloudwatch-positive|_invoke_until_logged#10|Timeout|time.sleep(5)
Issue-Key: apigateway__dynamodb__lambda__logs.apigateway-missing-cloudwatch-role-negative|_invoke_and_poll_logs#20|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__dynamodb__lambda__logs.apigw-stage-logging-level-error-only|_poll_log_events_with_traffic#16|Timeout|time.sleep(5)
Issue-Key: apigateway__dynamodb__lambda__logs.ddb-esm-lambda-error-logged-to-cloudwatch-positive|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__dynamodb__lambda__logs.cli-apigateway-missing-deployment-stage-negative|run#28|AssertionError|assert status2 == 200, f"expected 200 after deployment, got {status2}: {body_text2}"
Issue-Commit: b4ead8169e48e6047949699a772a41fd75a6bc4d

# API Gateway stage logging checks never complete and a deployed stage answers 500 after CreateDeployment

> _Note: this issue was identified against the code base at commit `b4ead8169e48e6047949699a772a41fd75a6bc4d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRestApi

## Description
Two net-new root causes. (Related open issue 20261029T180026Z-apigateway-dynamodb-lambda-logs-api-gateway-deployed-stage-invoke-account-logging-role.md owns sibling checks; these five replay keys are not carried by it.)

Cause 1 of 2: API Gateway stage execution/access logging and the DynamoDB-ESM error-logging scenarios never settle.
AWS: recorded behavior only — the recording shows a stage with StageDescription/MethodSettings loggingLevel set, an account CloudWatch role, and the scenario completing: each request writes an execution log group /aws/api-gateway/... and an access-log stream, and FilterLogEvents returns the lines, all well inside the suite's 60s budget.
Emulator: each of these four checks is cut off with "Error: Check timed out after 60.0s" — the emulator accepts the stage/logging configuration but never produces the log group, log stream or events the check then polls for, so the poll never terminates.
Why: a client that enables stage logging to diagnose an API gets an API that reports logging as configured and then never delivers a single log line; any wait-for-logs loop hangs instead of failing fast.
Fix: the API Gateway stage handler's logging delivery path (access logs + execution logs) and the DynamoDB event-source-mapping failure-logging path, plus whatever state records the configured destination log group.
Traces: apigateway__dynamodb__lambda__logs.apigateway-execution-logging-to-cloudwatch-positive, .apigateway-missing-cloudwatch-role-negative, .apigw-stage-logging-level-error-only, .ddb-esm-lambda-error-logged-to-cloudwatch-positive.

Cause 2 of 2: after CreateDeployment creates the missing stage, a request to the stage URL answers 500 instead of 200.
AWS: recorded behavior only — the check first proves the un-deployed stage is refused, then creates the deployment and expects the same path to answer HTTP 200 with the integration's body.
Emulator: the second request answers HTTP 500 with body {"message":"Internal server error"}. The fixed text of that body is AWS's DEFAULT_5XX gateway-response body; the status is 500. AWS answers 200 and the integration body here, so both the status and the body are wrong.
Why: a client cannot tell "stage not deployed" from "deployed and broken": the negative leg and the positive leg of the same scenario both end in a 5xx, so creating the deployment appears to have no effect.
Fix: the execute-api request router for a freshly created deployment/stage, and the integration-invocation path it dispatches to.
Traces: apigateway__dynamodb__lambda__logs.cli-apigateway-missing-deployment-stage-negative.

## Evidence
Expected (AWS): the four logging checks complete and FilterLogEvents returns the stage's execution/access log lines; the redeployed stage answers HTTP 200 with the integration body.
Actual (emulator): four checks end with 'Error: Check timed out after 60.0s'; cli-apigateway-missing-deployment-stage-negative fails with 'AssertionError: expected 200 after deployment, got 500: {"message":"Internal server error"}'.

## Steps to Reproduce
1. Create a REST API with a Lambda/DynamoDB integration, set the account CloudWatch role, and enable stage access logging plus execution logging (loggingLevel INFO/ERROR).
2. Invoke the stage, then poll CloudWatch Logs for the access-log stream and the execution log group. Observe no log group/stream/events ever appear.
3. Separately: create a REST API whose stage does not exist, request the stage URL (refused), call CreateDeployment with that stage name, and request the same URL again. Observe HTTP 500 {"message":"Internal server error"} instead of HTTP 200.
