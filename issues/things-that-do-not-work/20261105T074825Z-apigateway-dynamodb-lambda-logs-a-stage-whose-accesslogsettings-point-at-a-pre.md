Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda__logs.cli-apigateway-stage-access-log-missing-destination-rejected
Issue-Key: apigateway__dynamodb__lambda__logs.cli-apigateway-stage-access-log-missing-destination-rejected|_poll_for_log_events#36|Timeout|time.sleep(2)
Issue-Commit: 4fb69df4492bb482aa671e07d0cac65e9605f0df

# A stage whose accessLogSettings point at a pre-created log group never delivers access-log entries, so the access-log permission-boundary scenario never completes

> _Note: this issue was identified against the code base at commit `4fb69df4492bb482aa671e07d0cac65e9605f0df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStage

## Description
This issue carries 1 net-new root cause for the `apigateway__dynamodb__lambda__logs` service grouping. Cause count: 1 — a fixer must close it and re-replay every id in trace_ids before submitting.

RELATED OPEN ISSUE (named as instructed; it does NOT carry this replay key, which is why this check is not marked OWNED in the current replay report): 20261104T044826Z-apigateway-dynamodb-lambda-logs-api-gateway-stage-logging-checks-never-complete-and.md Cause 1 describes the same family of non-delivery for four sibling checks (apigateway-execution-logging-to-cloudwatch-positive, apigateway-missing-cloudwatch-role-negative, apigw-stage-logging-level-error-only, ddb-esm-lambda-error-logged-to-cloudwatch-positive) and 20261029T180026Z-apigateway-dynamodb-lambda-logs-api-gateway-deployed-stage-invoke-account-logging-role.md owns further siblings; neither lists this check id, so neither re-replays it. If a fixer concludes it is one work item with those, close them together — but this id must still be re-replayed here.

Cause 1 of 1: with the account-level CloudWatch push role configured and a stage's /accessLogSettings/destinationArn pointing at an existing CloudWatch Logs log group, requests to the stage never produce access-log events, and the scenario never reaches a graded answer.
AWS: Smithy (com.amazonaws.apigateway#BackplaneControlService) UpdateStage — PATCH /restapis/{restApiId}/stages/{stageName}, success 200, output shape Stage, which carries accessLogSettings (shape AccessLogSettings with destinationArn and format); UpdateAccount — PATCH /account, success 200, output Account{cloudwatchRoleArn, throttleSettings, features, apiKeyVersion}; both model BadRequestException(400), ConflictException(409), LimitExceededException(429), NotFoundException(404), TooManyRequestsException(429), UnauthorizedException(401). Delivery itself is not spelled by the model, so for it: recorded behavior only — once /account cloudwatchRoleArn names a role API Gateway can assume and a stage's accessLogSettings name an existing log group plus a format (here "$context.requestId $context.status"), each request to that stage is written to that log group, and logs:FilterLogEvents on it returns a non-empty `events` list within the scenario's wait; a sibling stage whose destinationArn names a log group the push role is explicitly DENIED logs:CreateLogGroup on stays absent — logs:DescribeLogGroups with that prefix returns an empty logGroups list and nothing is delivered. The log-group names, request ids and ARNs in any one recording are per-run values, not part of the requirement.
Emulator: the check produces no graded answer at all — it is cut off with `Error: Check timed out after 60.0s`. The two waits it can stall in are both emulator-served: the retry loop around apigateway UpdateAccount replacing /cloudwatchRoleArn with the freshly created push role (the check retries for up to 100 s and only fails loudly if it is never accepted), and the poll that re-issues GETs against the 'good' stage while calling logs FilterLogEvents on the pre-created log group for up to 130 s. Neither terminates inside the suite's 60 s budget, so no status, error code or message is ever compared and the permission-boundary assertion (the denied destination must stay absent) is never reached.
Why: a client that enables stage access logging to a log group it already owns sees UpdateStage report the destination as configured and then receives no log lines, with no error anywhere to explain it; any wait-for-logs loop (the AWS CLI, a deployment smoke test, an alarm on request volume) hangs instead of failing fast. The negative half — that a destination the push role cannot create is never materialized — is equally unverifiable, so an over-permissive emulator could silently create it.
Fix: the API Gateway stage access-log delivery path in the apigateway crate (the execute-api request path must, for a stage with accessLogSettings, render the configured $context format and write one event per request to the named CloudWatch Logs log group via the account cloudwatchRoleArn, and must create the destination only when that role is permitted to), together with the UpdateAccount /cloudwatchRoleArn handler that must accept a valid role ARN promptly rather than leaving the caller retrying.
Traces: apigateway__dynamodb__lambda__logs.cli-apigateway-stage-access-log-missing-destination-rejected (the only surviving failing check for this cause).

## Evidence
Failing check (replay report, service heading `### apigateway__dynamodb__lambda__logs`):
[mismatch] apigateway__dynamodb__lambda__logs.cli-apigateway-stage-access-log-missing-destination-rejected
  Error: Check timed out after 60.0s

The scenario, all through the `aws` CLI against the emulator: iam CreateRole (trust apigateway.amazonaws.com) + PutRolePolicy (Allow logs:CreateLogGroup/CreateLogStream/DescribeLogGroups/DescribeLogStreams/PutLogEvents/GetLogEvents/FilterLogEvents on '*', plus an explicit Deny on logs:CreateLogGroup for one named log-group ARN); apigateway update-account replacing /cloudwatchRoleArn with that role (retried up to 100 s); create-rest-api; create-resource /ping; put-method(GET, authorization-type NONE); put-integration(type MOCK, request-templates {"application/json":"{\"statusCode\": 200}"}); put-method-response 200; put-integration-response 200; create-deployment(stage 'good') and create-stage('bad'); logs create-log-group for the GOOD destination; update-stage on 'good' replacing /accessLogSettings/destinationArn with that log group's ARN and /accessLogSettings/format with "$context.requestId $context.status"; update-stage on 'bad' pointing at the denied (never created) ARN.

Expected (AWS, recorded behavior): the GET on the 'good' stage URL answers HTTP 200 from the MOCK integration; logs filter-log-events on the pre-created log group returns a non-empty `events` list inside the scenario's wait; logs describe-log-groups --log-group-name-prefix <denied name> keeps returning an empty logGroups list, so the check asserts the denied destination was never materialized.
Actual (emulator): the check never reaches any of those assertions — it is killed at the 60 s harness cap ('Error: Check timed out after 60.0s') while waiting either for update-account to accept the push role or for access-log events to appear in the configured log group. No log group, log stream or event is produced for the configured destination.

## Steps to Reproduce
Boot the emulator and point the AWS CLI / botocore at it.
1. iam CreateRole with a trust policy for apigateway.amazonaws.com; PutRolePolicy allowing logs:CreateLogGroup, CreateLogStream, DescribeLogGroups, DescribeLogStreams, PutLogEvents, GetLogEvents, FilterLogEvents on '*', plus an explicit Deny of logs:CreateLogGroup on one specific log-group ARN (the 'bad' destination, never created).
2. apigateway update-account --patch-operations replace /cloudwatchRoleArn with that role's ARN.
   -> OBSERVE whether it is accepted promptly; the scenario retries for up to 100 s.
3. apigateway create-rest-api; get-resources to find '/'; create-resource --path-part ping; put-method GET --authorization-type NONE; put-integration --type MOCK --request-templates '{"application/json":"{\"statusCode\": 200}"}'; put-method-response --status-code 200; put-integration-response --status-code 200 --selection-pattern ''.
4. create-deployment --stage-name good; create-stage --stage-name bad --deployment-id <id>.
5. logs create-log-group <good>; apigateway update-stage on 'good' with patch operations replacing /accessLogSettings/destinationArn with the good log group's ARN and /accessLogSettings/format with "$context.requestId $context.status"; apigateway update-stage on 'bad' pointing /accessLogSettings/destinationArn at the denied ARN.
6. GET <good stage url>/ping (expect HTTP 200), then poll logs filter-log-events --log-group-name <good>, re-issuing the GET periodically.
   -> OBSERVE: `events` stays empty; no log stream or event is ever created, and the scenario never completes within its budget.
   -> EXPECT: a non-empty `events` list containing one entry per request, rendered from the configured $context format.
7. logs describe-log-groups --log-group-name-prefix <bad> -> EXPECT an empty logGroups list for the whole window (the denied destination must never be materialized).
Replay: apigateway__dynamodb__lambda__logs.cli-apigateway-stage-access-log-missing-destination-rejected
