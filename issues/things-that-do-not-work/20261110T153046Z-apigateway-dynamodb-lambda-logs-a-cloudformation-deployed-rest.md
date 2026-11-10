Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda__logs.deploy-cfn-apigateway-access-logging-enabled-positive
Issue-Key: apigateway__dynamodb__lambda__logs.deploy-cfn-apigateway-access-logging-enabled-positive|_request_until_logged#10|Timeout|time.sleep(interval_s)
Issue-Commit: f4970b2d8a83c111b3a18edc79ceb1ab4d4e7ac8

# apigateway__dynamodb__lambda__logs: a CloudFormation-deployed REST stage with access logging enabled reaches no verdict — the deploy/serve/log path never settles (1 cause)

> _Note: this issue was identified against the code base at commit `f4970b2d8a83c111b3a18edc79ceb1ab4d4e7ac8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStage

## Description
Cause 1 of 1: the positive access-logging deployment never completes, so no access-log record can be graded.
AWS: CreateStack reaches CREATE_COMPLETE, the stage carries the template's AccessLogSetting (DestinationArn + Format), a request to the stage's invoke URL is served, and the access-log record for that request is delivered to the named log group within seconds, where FilterLogEvents returns it. Recorded behavior only for the record's field set (it is the template's format string).
Emulator: the check produces no verdict at all — 'Error: Check timed out after 60.0s'; the stack/serve/log sequence never reaches the state the check waits for.
Why: a customer deploying a REST API with access logging by template gets no signal that the stage is servable or that its access logs are delivered, so the whole observability wiring is unverifiable.
Fix: the cloudformation AWS::ApiGateway::Stage/Deployment handlers and the apigateway stage access-log emitter they configure. Several open issues under this heading (e.g. 20261107T070300Z-apigateway-dynamodb-lambda-logs-cloudformation-deployed-rest-api-stages-are-unservable-the.md) describe neighbouring defects but do not carry this check's replay key.
Traces: apigateway__dynamodb__lambda__logs.deploy-cfn-apigateway-access-logging-enabled-positive.

## Evidence
Detail: 'Error: Check timed out after 60.0s'. Expected: stack CREATE_COMPLETE, a 2xx from the stage invoke URL and an access-log event readable from the configured log group. Actual: no verdict inside the harness cap.

## Steps to Reproduce
Replay apigateway__dynamodb__lambda__logs.deploy-cfn-apigateway-access-logging-enabled-positive: deploy the template that creates a REST API, a log group and a stage with AccessLogSetting, wait for CREATE_COMPLETE, call the stage URL and then read the log group.
