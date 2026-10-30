Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.update-account-cloudwatch-role-enforced-for-logging
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway.update-account-cloudwatch-role-enforced-for-logging|_client_error#9|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Commit: 52da73ed85c6d1f654ba3daa926c435304ddf8f6

# UpdateStage accepts enabling stage execution logging while the account has no cloudwatchRoleArn, instead of refusing with BadRequestException

> _Note: this issue was identified against the code base at commit `52da73ed85c6d1f654ba3daa926c435304ddf8f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStage

## Description
One net-new root cause in the apigateway group (1 of 1).

Cause 1 of 1: UpdateStage does not enforce the account-level CloudWatch Logs role precondition when a patch turns on execution logging for a stage. Failing check: apigateway.update-account-cloudwatch-role-enforced-for-logging.

AWS: API Gateway enforces the account's logging role at CONFIGURATION time. With the account singleton's cloudwatchRoleArn unset (GetAccount reports it absent/empty, e.g. right after UpdateAccount replaces /cloudwatchRoleArn with ""), an UpdateStage patch that sets a method-setting logging level — {"op": "replace", "path": "/*/*/logging/loglevel", "value": "INFO"} — is refused with HTTP 400, error code BadRequestException and the message `CloudWatch Logs role ARN must be set in account settings to enable logging`. That message is entirely fixed text for this error class: it carries no variable parts from the request or from state. The refused patch must leave no trace — an independent GetStage must still report methodSettings["*/*"].loggingLevel as absent/OFF. Once UpdateAccount sets a valid role ARN (a role trusting apigateway.amazonaws.com with AmazonAPIGatewayPushToCloudWatchLogs attached), the same UpdateStage patch must be accepted with no further API Gateway call. Smithy: UpdateStage (PATCH /restapis/{restApiId}/stages/{stageName}) models BadRequestException (httpError 400); UpdateAccount/GetAccount both carry the account's cloudwatchRoleArn member. The precondition itself and its wording are not in the model — recorded behavior only for the message text, which the recording shows as the fixed string above.

Emulator: the UpdateStage call SUCCEEDS. The replay shows HTTP 200 with a full Stage document whose methodSettings["*/*"] already carries loggingLevel = "INFO" (plus the default throttling/caching members), so no BadRequestException is raised at all and the stage state is mutated by a call AWS refuses. The check's helper that demands a refusal fails with `expected a rejection, but the call SUCCEEDED and returned {...'HTTPStatusCode': 200... 'loggingLevel': 'INFO'...}`.

Why: a client that enables execution logging without the account role is told the configuration was applied. It then waits for execution log events that can never be delivered, and the operator gets no synchronous signal that the account-level role is missing — a boundary AWS enforces at configuration time becomes an invisible silent-delivery failure. Any negative test that distinguishes "refused at configuration" from "accepted but undelivered" cannot pass, and the over-permissive accept also corrupts the stage representation (loggingLevel INFO on a stage AWS would have left OFF).

Related but not covering: the open issue 20261030T163201Z-apigateway-dynamodb-lambda-logs-updatestage-enables-stage-execution-logging-with-no-account.md describes the same precondition from the apigateway__dynamodb__lambda__logs check apigw-missing-cloudwatch-role-arn-no-logs-negative; the replay report does not mark this apigateway-group check as owned by it, so it is filed here under the apigateway service with its own replay key. A fixer closing one should re-replay both check ids.

Fix: the UpdateStage handler's patch validation for method-setting logging paths (/*/*/logging/loglevel and the per-method equivalents) in the apigateway service — it must read the apigateway account singleton's cloudwatchRoleArn and refuse the patch with 400 BadRequestException and the fixed message above when that setting is unset, leaving the stage's methodSettings unmodified; once the account role is set the same patch must be accepted without any extra propagation call.

Traces: apigateway.update-account-cloudwatch-role-enforced-for-logging

## Evidence
Replay of apigateway.update-account-cloudwatch-role-enforced-for-logging [mismatch]:

  File "check_apigateway_update_account_cloudwatch_role_enforced_for_logging.py", line 39, in run
    err = _client_error(
  File "check_apigateway_update_account_cloudwatch_role_enforced_for_logging.py", line 126, in _client_error
    raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
  AssertionError: expected a rejection, but the call SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'deploymentId': '...', 'stageName': '...', 'methodSettings': {'*/*': {'metricsEnabled': False, 'loggingLevel': 'INFO', 'dataTraceEnabled': False, ...}}, ...}

Expected (AWS), with the account's cloudwatchRoleArn cleared:
  UpdateStage patchOperations=[{op: replace, path: /*/*/logging/loglevel, value: INFO}]
    -> HTTP 400, Error.Code = BadRequestException,
       Error.Message = "CloudWatch Logs role ARN must be set in account settings to enable logging"
       (entirely fixed text for this error class — no request/state values)
  followed by GetStage -> methodSettings["*/*"].loggingLevel absent / OFF (patch had no effect).

Actual (emulator):
  UpdateStage -> HTTP 200 with the Stage document, methodSettings["*/*"].loggingLevel = "INFO".
  No exception is raised, and the stage now reports execution logging enabled even though the account has no logging role.

## Steps to Reproduce
1. Boot the emulator and replay apigateway.update-account-cloudwatch-role-enforced-for-logging.
2. Or by hand, with the AWS CLI pointed at the emulator:
   a. apigateway update-account --patch-operations op=replace,path=/cloudwatchRoleArn,value=''
      then apigateway get-account and confirm cloudwatchRoleArn is empty/absent.
   b. Create a minimal deployed API: create-rest-api; put-method (GET, authorizationType NONE) on the root resource; put-integration --type MOCK --request-templates '{"application/json":"{\"statusCode\": 200}"}'; put-method-response --status-code 200; put-integration-response --status-code 200; create-deployment; create-stage.
   c. apigateway update-stage --rest-api-id <id> --stage-name <stage> --patch-operations op=replace,path=/*/*/logging/loglevel,value=INFO
      Expected: HTTP 400 BadRequestException "CloudWatch Logs role ARN must be set in account settings to enable logging". Observed: HTTP 200 with the Stage document.
   d. apigateway get-stage --rest-api-id <id> --stage-name <stage> and observe methodSettings["*/*"].loggingLevel — AWS leaves it OFF/absent because the patch was refused; the emulator reports INFO.
   e. Then create an IAM role trusting apigateway.amazonaws.com with arn:aws:iam::aws:policy/service-role/AmazonAPIGatewayPushToCloudWatchLogs attached, set it with apigateway update-account --patch-operations op=replace,path=/cloudwatchRoleArn,value=<roleArn>, and re-issue the step (c) patch — it must now succeed with no further API Gateway call.
