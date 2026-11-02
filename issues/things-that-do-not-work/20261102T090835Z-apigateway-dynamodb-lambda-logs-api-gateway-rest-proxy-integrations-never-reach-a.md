Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda__logs.apigw-log-group-arn-mismatch-negative apigateway__dynamodb__lambda__logs.cli-apigateway-create-deploy-invoke-lambda apigateway__dynamodb__lambda__logs.cli-apigateway-create-deploy-invoke-roundtrip apigateway__dynamodb__lambda__logs.cli-apigateway-lambda-integration-setup-and-test apigateway__dynamodb__lambda__logs.cli-apigateway-missing-lambda-permission-rejected apigateway__dynamodb__lambda__logs.cli-dynamodb-put-item-then-lambda-scan apigateway__dynamodb__lambda__logs.cli-lambda-invoke-missing-permission-negative apigateway__dynamodb__lambda__logs.cli-lambda-update-env-vars-dynamodb-table-name apigateway__dynamodb__lambda__logs.cli-lambda-update-function-code-then-invoke-logs-check apigateway__dynamodb__lambda__logs.cli-apigateway-stage-access-log-missing-destination-rejected apigateway__dynamodb__lambda__logs.ddb-esm-lambda-missing-logs-permission-negative
Issue-Key: apigateway__dynamodb__lambda__logs.apigw-log-group-arn-mismatch-negative|run#20|AssertionError|assert error_message == expected_message, (
Issue-Key: apigateway__dynamodb__lambda__logs.cli-apigateway-create-deploy-invoke-lambda|run#29|AssertionError|assert result["status"] == 200, f"unexpected integration status: {result}"
Issue-Key: apigateway__dynamodb__lambda__logs.cli-apigateway-create-deploy-invoke-roundtrip|run#26|AssertionError|assert status == 200, f"unexpected status {status}: {body_text}"
Issue-Key: apigateway__dynamodb__lambda__logs.cli-apigateway-lambda-integration-setup-and-test|run#28|AssertionError|assert status == 200, f"unexpected status {status}: {body_text}"
Issue-Key: apigateway__dynamodb__lambda__logs.cli-apigateway-missing-lambda-permission-rejected|run#33|AssertionError|assert allowed_result["status"] == 200, f"expected 200 after granting permission, got {allowed_result}"
Issue-Key: apigateway__dynamodb__lambda__logs.cli-dynamodb-put-item-then-lambda-scan|_run_shell#17|AssertionError|raise AssertionError(
Issue-Key: apigateway__dynamodb__lambda__logs.cli-lambda-invoke-missing-permission-negative|run#28|AssertionError|assert match.group(1) == "AccessDeniedException", (
Issue-Key: apigateway__dynamodb__lambda__logs.cli-lambda-update-env-vars-dynamodb-table-name|_run_shell#14|AssertionError|raise AssertionError(
Issue-Key: apigateway__dynamodb__lambda__logs.cli-lambda-update-function-code-then-invoke-logs-check|_run_shell#7|AssertionError|raise AssertionError(
Issue-Key: apigateway__dynamodb__lambda__logs.cli-apigateway-stage-access-log-missing-destination-rejected|_poll_for_log_events#37|Timeout|return False
Issue-Key: apigateway__dynamodb__lambda__logs.ddb-esm-lambda-missing-logs-permission-negative|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Commit: 86c9f1e4438a61fbe37b19dfcd3d77efce3d0679

# API Gateway REST proxy integrations never reach a Lambda backend, Invoke is refused NotImplemented, and UpdateStage's access-log-format refusal is emulator-worded (4 causes)

> _Note: this issue was identified against the code base at commit `86c9f1e4438a61fbe37b19dfcd3d77efce3d0679`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStage

## Description
Four net-new root causes in this service group. Each must be closed before submitting.

Cause 1 of 4 — UpdateStage's access-log-format refusal message is the emulator's own prose and names only one of the two accepted context variables.
AWS: Smithy (com.amazonaws.apigateway#BackplaneControlService) UpdateStage, PATCH /restapis/{restApiId}/stages/{stageName}, success 200, input patchOperations: ListOfPatchOperation, models BadRequestException with httpError 400. The model carries no message text, so for the text this is recorded behavior only: when the /accessLogSettings/format value contains neither $context.requestId nor $context.extendedRequestId, API Gateway answers HTTP 400 / BadRequestException with the message `Access Log format must include either $context.requestId or $context.extendedRequestId : '<format>'`. Fixed text: `Access Log format must include either $context.requestId or $context.extendedRequestId : '` and the closing `'`. The only variable part is the rejected format string, copied verbatim from the patch operation's value.
Emulator: answers 400 / BadRequestException (status and code correct) with `/accessLogSettings/format must include at least $context.requestId, got "<format>"` — a different fixed sentence that states only $context.requestId as acceptable, names the JSON patch path instead of the AWS phrase "Access Log format", and quotes the format with JSON escaping instead of single quotes.
Why: a caller that reads the refusal to learn which context variables satisfy the rule is told that only $context.requestId will do, so tooling that logs $context.extendedRequestId instead (a legal AWS configuration) is pushed to rewrite a working format; anything asserting API Gateway's sentence fails on the whole string.
Fix: the accessLogSettings/format validation in UpdateStage's validation layer (the stage patch validator in the apigateway crate) — emit AWS's fixed sentence naming both variables and interpolate only the rejected format value. Note: the FIXED issue 20261030T085052Z-apigateway-api-gateway-400-404-refusal-messages-are-emulator.md collected other UpdateStage refusal messages (the copy-patch grammar); it does not carry this refusal site, so this is a distinct, still-open cause rather than a regression of that fix.
Traces: apigateway__dynamodb__lambda__logs.apigw-log-group-arn-mismatch-negative

Cause 2 of 4 — an AWS_PROXY integration on a deployed REST stage answers 500 API_CONFIGURATION_ERROR instead of the backend function's response.
AWS: recorded behavior only (the Smithy model describes only the control plane). A REST API with a deployed stage and an AWS_PROXY integration pointing at a Lambda function, with lambda:InvokeFunction granted to apigateway.amazonaws.com, answers the backend handler's status, headers and body — HTTP 200 with the handler's JSON for these checks — both through the stage's execute-api URL and through TestInvokeMethod.
Emulator: answers HTTP 500 with body `{"message":"Internal server error"}` and an execution log stating `Execution failed due to configuration error: the AWS_PROXY integration names a backend this emulator cannot call: no deployment package runs in process and no outbound connection is made, so no backend answer exists to map` followed by `Answered through this API's API_CONFIGURATION_ERROR gateway response`.
Why: every REST-API-in-front-of-Lambda scenario is unobservable — a client cannot distinguish a genuine integration misconfiguration (which AWS also answers 500) from the emulator's blanket inability to call a backend, and a negative check that grants the permission and expects 200 can never see the difference the permission makes.
Fix: the AWS_PROXY integration execution path of the REST data plane (the integration dispatcher that maps a method request onto the Lambda Invoke path), together with the Lambda invocation backend named in cause 3. Note: open issue 20261029T182816Z-apigateway-lambda-logs-proxy-integrations-answer-500-the.md records this behavior for the apigateway__lambda__logs group and carries none of these checks' keys.
Traces: apigateway__dynamodb__lambda__logs.cli-apigateway-create-deploy-invoke-lambda, apigateway__dynamodb__lambda__logs.cli-apigateway-create-deploy-invoke-roundtrip, apigateway__dynamodb__lambda__logs.cli-apigateway-lambda-integration-setup-and-test, apigateway__dynamodb__lambda__logs.cli-apigateway-missing-lambda-permission-rejected

Cause 3 of 4 — lambda Invoke refuses every real invocation with NotImplemented, so an IAM refusal is never reached.
AWS: Invoke on an existing, Active function returns HTTP 200 with the handler's payload; when the caller is not authorized to invoke it, AWS answers AccessDeniedException. Recorded behavior only for the payload.
Emulator: answers error code `NotImplemented` with `Invoke resolved <function arn> at version $LATEST, but running its <runtime> deployment package is not implemented in this emulator: there is no in-process runtime for it, and no response payload is returned that the function did not produce.` for python3.12 packages here, and the same for every runtime. Because the refusal is emitted before authorization is evaluated, cli-lambda-invoke-missing-permission-negative sees `NotImplemented` where AWS answers `AccessDeniedException`.
Why: no Lambda-backed workflow can be exercised, and — worse — a missing-permission negative check cannot tell an authorization failure from the emulator's own refusal, so a caller relying on the error code to branch (retry, re-grant, fail) takes the wrong branch.
Fix: the Invoke handler in the lambda crate (the invocation backend). At minimum the authorization decision must be made BEFORE the not-implemented refusal, so AccessDeniedException is still observable. Note: open issue 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md records this root cause and carries none of these four checks' keys.
Traces: apigateway__dynamodb__lambda__logs.cli-dynamodb-put-item-then-lambda-scan, apigateway__dynamodb__lambda__logs.cli-lambda-invoke-missing-permission-negative, apigateway__dynamodb__lambda__logs.cli-lambda-update-env-vars-dynamodb-table-name, apigateway__dynamodb__lambda__logs.cli-lambda-update-function-code-then-invoke-logs-check

Cause 4 of 4 — access-log delivery and DynamoDB-stream event-source processing never happen, so the checks that poll for them never terminate.
AWS: recorded behavior only. A stage with accessLogSettings pointing at an existing log group delivers one access-log event per request to that group; a Lambda event source mapping on a DynamoDB stream delivers inserted records to the function and the function's output reaches its results table. Both are observable within the checks' polling windows.
Emulator: neither effect ever appears, so the checks' bounded polls never satisfy their predicate and the harness ends them with `Check timed out after 60.0s` — no access-log event is written to the configured destination group, and no stream record is processed.
Why: a client that waits on vended access logs or on stream-driven processing waits forever; and because the wait is indefinite rather than a refusal, a caller gets no signal at all about what failed.
Fix: the stage access-log delivery path in the apigateway crate and the DynamoDB-stream poller of the lambda event-source-mapping subsystem.
Traces: apigateway__dynamodb__lambda__logs.cli-apigateway-stage-access-log-missing-destination-rejected, apigateway__dynamodb__lambda__logs.ddb-esm-lambda-missing-logs-permission-negative

## Evidence
All from the simparity-behavioral-aws-v1 replay against this checkout.

Cause 1 — apigw-log-group-arn-mismatch-negative:
  expected message: "Access Log format must include either $context.requestId or $context.extendedRequestId : '{\"httpMethod\":\"$context.httpMethod\",\"status\":\"$context.status\"}'"
  actual message:   '/accessLogSettings/format must include at least $context.requestId, got "{\\"httpMethod\\":\\"$context.httpMethod\\",\\"status\\":\\"$context.status\\"}"'
  Status 400 and code BadRequestException matched. Message anatomy (AWS): fixed 'Access Log format must include either $context.requestId or $context.extendedRequestId : ' + "'" + VARIABLE rejected format + "'". The format string shown is this run's value; it is the template slot, not part of the requirement.

Cause 2 — cli-apigateway-create-deploy-invoke-lambda / -roundtrip / cli-apigateway-lambda-integration-setup-and-test / cli-apigateway-missing-lambda-permission-rejected:
  AssertionError: unexpected integration status: {'status': 500, 'body': '{"message":"Internal server error"}', ... 'log': "... Integration type: AWS_PROXY ... Execution failed due to configuration error: the AWS_PROXY integration names a backend this emulator cannot call: no deployment package runs in process and no outbound connection is made, so no backend answer exists to map ... Answered through this API's API_CONFIGURATION_ERROR gateway response ... Method completed with status: 500"}
  AssertionError: unexpected status 500: {"message":"Internal server error"} (expected 200)
  AssertionError: expected 200 after granting permission, got {'status': 500, ...}

Cause 3 — cli-lambda-invoke-missing-permission-negative:
  AssertionError: expected AccessDeniedException, CLI reported 'NotImplemented' (stderr: An error occurred (NotImplemented) when calling the Invoke operation: Invoke resolved arn:aws:lambda:<region>:<account>:function:<name> at version $LATEST, but running its python3.12 deployment package is not implemented in this emulator ...)
  cli-dynamodb-put-item-then-lambda-scan / cli-lambda-update-env-vars-dynamodb-table-name / cli-lambda-update-function-code-then-invoke-logs-check: `aws lambda invoke` exits rc=254 with the same NotImplemented text.

Cause 4 — cli-apigateway-stage-access-log-missing-destination-rejected and ddb-esm-lambda-missing-logs-permission-negative:
  Error: Check timed out after 60.0s (the checks poll the access-log destination group / the ESM results table for an effect that never arrives).

## Steps to Reproduce
Boot the emulator from this checkout and point the AWS CLI/SDK at its endpoint.

Cause 1: create a REST API, deploy a stage, create a log group, then:
  aws apigateway update-stage --rest-api-id <apiId> --stage-name prod --patch-operations op=replace,path=/accessLogSettings/destinationArn,value=<log group arn> op=replace,path=/accessLogSettings/format,value='{"httpMethod":"$context.httpMethod","status":"$context.status"}'
  -> 400 BadRequestException with '/accessLogSettings/format must include at least $context.requestId, got ...'; AWS answers "Access Log format must include either $context.requestId or $context.extendedRequestId : '<format>'".

Cause 2: create a Lambda function, a REST API with an AWS_PROXY integration naming it, add-permission for apigateway.amazonaws.com, deploy a stage, then GET the stage URL (and aws apigateway test-invoke-method). -> 500 {"message":"Internal server error"} with API_CONFIGURATION_ERROR in the execution log; AWS answers the handler's 200.

Cause 3: aws lambda invoke --function-name <fn> /tmp/out.json -> NotImplemented. Then strip lambda:InvokeFunction from the caller and repeat: still NotImplemented, where AWS answers AccessDeniedException.

Cause 4: configure accessLogSettings on a deployed stage pointing at an existing log group, drive traffic, and poll filter-log-events on that group — no event ever appears. Create a DynamoDB stream table, a function and an event source mapping, put an item, and poll the results table — no record is ever processed.

Then re-replay: apigateway__dynamodb__lambda__logs.apigw-log-group-arn-mismatch-negative, .cli-apigateway-create-deploy-invoke-lambda, .cli-apigateway-create-deploy-invoke-roundtrip, .cli-apigateway-lambda-integration-setup-and-test, .cli-apigateway-missing-lambda-permission-rejected, .cli-dynamodb-put-item-then-lambda-scan, .cli-lambda-invoke-missing-permission-negative, .cli-lambda-update-env-vars-dynamodb-table-name, .cli-lambda-update-function-code-then-invoke-logs-check, .cli-apigateway-stage-access-log-missing-destination-rejected, .ddb-esm-lambda-missing-logs-permission-negative
