Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__lambda__logs.apigw-stage-access-logs-delivered-to-log-group
Issue-Key: apigateway__lambda__logs.apigw-stage-access-logs-delivered-to-log-group|_http#12|Timeout|with urllib.request.urlopen(req, timeout=timeout_s) as resp:
Issue-Commit: 7f27151474f220c715589d81820913dcd4b7b62b

# apigateway__lambda__logs: a stage with accessLogSettings and an AWS_PROXY Lambda integration delivers no access-log record, so no $context variable is ever resolved into the log group

> _Note: this issue was identified against the code base at commit `7f27151474f220c715589d81820913dcd4b7b62b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStage

## Description
Cause 1 of 1: a REST stage configured with accessLogSettings (destinationArn = a CloudWatch Logs group, format = a custom $context JSON document) delivers no access-log record for the requests it serves.
  AWS: Smithy: Stage.accessLogSettings -> AccessLogSettings{destinationArn, format}, set at CreateStage/UpdateStage (and refused unless the account-level cloudwatchRoleArn is configured, which this check sets first). Recorded behavior: once the stage serves a request, API Gateway writes exactly one access-log record per request into the named log group, with every $context variable in the configured format resolved for that request — $context.httpMethod = the request verb, $context.status = the status actually returned, $context.stage = the stage name, $context.resourcePath = the un-prefixed resource path ("/ping"), $context.path = the STAGE-QUALIFIED request path ("/<stage>/ping"), and $context.requestId = the non-empty id minted for that request. The record's values come from the request and the stage being served, not from any fixed recording.
  Emulator: the stage with accessLogSettings serves the scenario and no matching access-log record ever appears in the configured log group. The check re-issues the request and re-reads the log group until its budget is gone and reaches no verdict ("Check timed out after 180.0s"), so none of the $context resolutions above can be observed. This is an AWS_PROXY (Lambda) integration stage, and the record is missing for it as a whole — not merely one unresolved variable.
  Why: an operator who configures stage access logging gets an empty log group: per-request status, path, stage and request id are unavailable, request-id correlation between the gateway and the integration is impossible, and any alarm, metric filter or audit built on access logs never fires.
  Fix: the execute-api data plane's access-log emitter for a stage carrying accessLogSettings — it must render the configured format with the per-request $context values and PutLogEvents one record per served request into destinationArn.
  Traces: apigateway__lambda__logs.apigw-stage-access-logs-delivered-to-log-group

Note: several open issues report missing access-log delivery for neighbouring surfaces — 20261026T072730Z-apigateway-dynamodb-lambda-logs-apigateway-a-stage-with-accesslogsettings-custom-context-json.md, 20261105T074825Z-apigateway-dynamodb-lambda-logs-a-stage-whose-accesslogsettings-point-at-a-pre.md, 20261107T035448Z-apigateway-lambda-logs-a-rest-stage-s-access.md and the apigatewayv2 ones — but none of them carries this check's replay key, so it is filed here; a fixer should expect the same emitter to be the common component.

## Evidence
Check apigateway__lambda__logs.apigw-stage-access-logs-delivered-to-log-group
  Setup: Lambda (AWS_PROXY) behind GET /ping, account cloudwatchRoleArn configured, stage 'test' deployed with accessLogSettings{destinationArn=<log group arn>, format={"requestId":"$context.requestId","httpMethod":"$context.httpMethod","path":"$context.path","status":"$context.status","stage":"$context.stage","marker":"$context.resourcePath"}}.
  Expected: GET /test/ping returns 200 and the log group receives one record parseable as that JSON with httpMethod='GET', status='200', stage='test', marker='/ping', path='/test/ping' and a non-empty requestId.
  Actual: no record containing the needle '"/test/ping"' ever appears in the log group; the request/read loop runs out and the harness reports "Error: Check timed out after 180.0s". No $context assertion is ever reached.

## Steps to Reproduce
1. Create a Lambda returning {"statusCode":200,"body":"{\"ok\":true}"} and a CloudWatch Logs group.
2. UpdateAccount so cloudwatchRoleArn is set (otherwise UpdateStage refuses the accessLogSettings patch).
3. CreateRestApi, CreateResource 'ping', PutMethod GET (authorizationType=NONE), PutIntegration type=AWS_PROXY integrationHttpMethod=POST uri=arn:aws:apigateway:<region>:lambda:path/2015-03-31/functions/<fn-arn>/invocations, AddPermission for apigateway.amazonaws.com on arn:aws:execute-api:<region>:<account>:<api-id>/*/*.
4. Deploy stage 'test' with accessLogSettings{destinationArn=<log group arn>, format=<the $context JSON above>}.
5. GET https://<api-id>.execute-api.<region>.<host>/test/ping (200), then FilterLogEvents/GetLogEvents on the log group from 60 s before the request. No access-log record is ever delivered.
