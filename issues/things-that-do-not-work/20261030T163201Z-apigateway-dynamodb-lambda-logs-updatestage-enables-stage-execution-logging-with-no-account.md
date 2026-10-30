Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda__logs.apigw-missing-cloudwatch-role-arn-no-logs-negative
Issue-Key: apigateway__dynamodb__lambda__logs.apigw-missing-cloudwatch-role-arn-no-logs-negative|run#11|AssertionError|assert raised, "expected UpdateStage to reject enabling logging with no account cloudWatchRoleArn"
Issue-Commit: 47f4067126cea87038b612b9d5cde693ce54e59e

# UpdateStage enables stage execution logging with no account cloudWatchRoleArn configured, where AWS refuses with BadRequestException

> _Note: this issue was identified against the code base at commit `47f4067126cea87038b612b9d5cde693ce54e59e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStage

## Description
One net-new root cause in the apigateway__dynamodb__lambda__logs group.

Cause 1 of 1: UpdateStage accepts a patch that turns on execution logging for a stage while the account has no CloudWatch Logs role configured.

AWS: API Gateway enforces the account-level logging role at CONFIGURATION time. When the account's `cloudwatchRoleArn` is unset (GetAccount reports it absent/empty, e.g. after `UpdateAccount` replaces `/cloudwatchRoleArn` with ""), a `UpdateStage` patch that sets a logging level — `{"op": "replace", "path": "/*/*/logging/loglevel", "value": "INFO"}` — is refused with HTTP 400 `BadRequestException` and the message `CloudWatch Logs role ARN must be set in account settings to enable logging`. That message is entirely fixed text for this error class; it carries no values from the request or from state. The patch must leave no trace: an independent `GetStage` must still report `methodSettings["*/*"].loggingLevel` as absent/`OFF`. Smithy: `UpdateStage` (PATCH /restapis/{restApiId}/stages/{stageName}) models `BadRequestException` (httpError 400) among its errors; the account-role precondition itself is documented behavior, not modeled — recorded behavior only for the message text, which the recording shows as the fixed string above.

Emulator: the `UpdateStage` call SUCCEEDS (HTTP 200 with the Stage document) — no `BadRequestException` is raised at all. The check's assertion `expected UpdateStage to reject enabling logging with no account cloudWatchRoleArn` fails because nothing was raised. Note this is a different root cause from Cause 4 of the open issue `20261029T180026Z-apigateway-dynamodb-lambda-logs-api-gateway-deployed-stage-invoke-account-logging-role.md` (UpdateAccount rejecting a well-formed cloudwatchRoleArn): the call that previously failed now gets through, and the divergence has moved to the UpdateStage precondition, which that issue does not describe and which the replay report does not report as owned.

Why: a client that configures logging without the account role is told the configuration was applied. It then waits for execution log events that can never be delivered, and the operator gets no signal that the account-level role is missing — the boundary AWS enforces synchronously at configuration time becomes an invisible silent-delivery failure. Negative tests that distinguish "refused at configuration" from "accepted but undelivered" cannot pass.

Fix: the UpdateStage handler's validation for method-setting logging patches — it must read the account setting (the apigateway account singleton's cloudwatchRoleArn) and refuse the patch with BadRequestException when that setting is unset, leaving the stage's methodSettings unmodified.

Traces: apigateway__dynamodb__lambda__logs.apigw-missing-cloudwatch-role-arn-no-logs-negative

## Evidence
Replay of apigateway__dynamodb__lambda__logs.apigw-missing-cloudwatch-role-arn-no-logs-negative [mismatch]:

  File "check_apigateway__dynamodb__lambda__logs_apigw_missing_cloudwatch_role_arn_no_logs_negative.py", line 49, in run
    assert raised, "expected UpdateStage to reject enabling logging with no account cloudWatchRoleArn"
  AssertionError: expected UpdateStage to reject enabling logging with no account cloudWatchRoleArn

Expected (AWS): UpdateStage -> HTTP 400, error code `BadRequestException`, message `CloudWatch Logs role ARN must be set in account settings to enable logging` (all fixed text for this error class). Follow-up GetStage must report loggingLevel absent / `OFF` for `*/*`.
Actual (emulator): UpdateStage returns success (no exception, HTTP 200 Stage document), so the check's `except apigw.exceptions.BadRequestException` branch is never taken and `raised` stays False.

## Steps to Reproduce
1. Boot the emulator and replay apigateway__dynamodb__lambda__logs.apigw-missing-cloudwatch-role-arn-no-logs-negative.
2. Or by hand: create a REST API with a Lambda-proxy GET method and deploy a stage (e.g. `prod`).
3. Clear the account logging role: `apigateway update-account --patch-operations op=replace,path=/cloudwatchRoleArn,value=''` (confirm with `get-account` that cloudwatchRoleArn is unset).
4. Call `apigateway update-stage --rest-api-id <id> --stage-name prod --patch-operations op=replace,path=/*/*/logging/loglevel,value=INFO`.
   Expected: HTTP 400 BadRequestException `CloudWatch Logs role ARN must be set in account settings to enable logging`. Observed: the call succeeds.
5. Call `apigateway get-stage --rest-api-id <id> --stage-name prod` and observe methodSettings["*/*"].loggingLevel — AWS leaves it OFF/absent because the patch was refused.
