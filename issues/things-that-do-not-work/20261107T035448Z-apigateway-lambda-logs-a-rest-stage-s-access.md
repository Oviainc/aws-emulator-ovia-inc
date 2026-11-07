Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__lambda__logs.apigw-stage-access-logs-delivered-to-log-group apigateway__lambda__logs.apigw-method-level-log-level-overrides-the-stage-wide-level
Issue-Key: apigateway__lambda__logs.apigw-stage-access-logs-delivered-to-log-group|_request_until_logged#12|Timeout|status, _, text = _poll_http(
Issue-Key: apigateway__lambda__logs.apigw-method-level-log-level-overrides-the-stage-wide-level|run#34|Timeout|assert status == 200, f"{verb} /echo returned {status}, expected 200"
Issue-Commit: 48b98241475aee57ea7962587dcd25c49f07ef51

# apigateway__lambda__logs: a REST stage's access logs and method-level execution-log overrides never reach the CloudWatch Logs destination

> _Note: this issue was identified against the code base at commit `48b98241475aee57ea7962587dcd25c49f07ef51`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStage

## Description
Cause 1 of 1: neither a stage's accessLogSettings destination nor its per-method loggingLevel override produces any CloudWatch Logs record.
AWS: with the account-level CloudWatch role installed (UpdateAccount cloudwatchRoleArn pointing at a role carrying AmazonAPIGatewayPushToCloudWatchLogs) a stage configured through UpdateStage with accessLogSettings {destinationArn, format} writes one access-log record per request into that log group, rendered by the stage's $context format, within seconds of the request. A methodSettings entry for a specific resource/method with loggingLevel=INFO overrides the stage-wide loggingLevel=ERROR for that method only: a successful request to the overridden method emits its execution trace (ending 'Method completed with status: 200') into the stage's execution log group /aws/api-gateway/<api-id>/<stage> while a successful request to a sibling method at the stage-wide ERROR level emits nothing. The fixed requirement is per-request delivery and the level gating; group names, api ids and formats are values of the request.
Emulator: both checks never complete inside the suite window — the poll for the stage's access-log record / the method's INFO execution trace never settles, so neither the delivered records nor the 'nothing logged at ERROR' negative side can be graded.
Why: a client relying on access logs for request accounting and on per-method log levels for targeted debugging gets empty log groups, so it cannot tell a working stage from a broken one, and a method-level override appears to have no effect at all.
Fix: the API Gateway stage logging components — the access-log emitter that renders accessLogSettings.format into the configured destination, and the execution-log emitter that applies the methodSettings loggingLevel/dataTrace override over the stage-wide level — both reached through the account CloudWatch role. Open issues 20261029T182816Z-apigateway-lambda-logs-proxy-integrations-answer-500-the.md and 20261104T044826Z-apigateway-lambda-logs-a-stage-scoped-lambda-permission-check-never-completes.md cover other checks of this service and do not claim these two.
Traces: apigateway__lambda__logs.apigw-stage-access-logs-delivered-to-log-group, apigateway__lambda__logs.apigw-method-level-log-level-overrides-the-stage-wide-level.

## Evidence
Expected: one access-log record per request in the stage's accessLogSettings destination, and the INFO execution trace of the overridden method (with no trace for the sibling method left at the stage-wide ERROR level).
Actual: both checks end with 'Error: Check timed out after 60.0s' while polling the destination/execution log groups; no record of either kind is observable.

## Steps to Reproduce
1. Create a role trusted by apigateway.amazonaws.com with AmazonAPIGatewayPushToCloudWatchLogs and install it via apigateway.update_account(patchOperations=[{op:'replace', path:'/cloudwatchRoleArn', value:<arn>}]).
2. Create a REST API with a Lambda proxy integration, deploy a stage, and update_stage with accessLogSettings destinationArn=<log group arn> and a $context JSON format, plus methodSettings loggingLevel=ERROR stage-wide and loggingLevel=INFO for one resource/method.
3. Send successful requests to the overridden method and to a sibling method.
4. Poll the access-log destination group and /aws/api-gateway/<api-id>/<stage> — no access-log record and no execution trace ever appear.
