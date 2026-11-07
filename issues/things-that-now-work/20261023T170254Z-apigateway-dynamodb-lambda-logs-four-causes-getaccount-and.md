Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda__logs.cli-apigateway-stage-access-log-missing-destination-rejected apigateway__dynamodb__lambda__logs.cli-dynamodb-put-item-then-lambda-scan apigateway__dynamodb__lambda__logs.cli-lambda-update-env-vars-dynamodb-table-name apigateway__dynamodb__lambda__logs.cli-lambda-update-function-code-then-invoke-logs-check apigateway__dynamodb__lambda__logs.cli-lambda-invoke-missing-permission-negative apigateway__dynamodb__lambda__logs.iam-role-wrong-principal-service-negative
Issue-Key: apigateway__dynamodb__lambda__logs.cli-apigateway-stage-access-log-missing-destination-rejected|_run_shell#6|AssertionError|raise AssertionError(
Issue-Key: apigateway__dynamodb__lambda__logs.cli-dynamodb-put-item-then-lambda-scan|_run_shell#16|AssertionError|raise AssertionError(
Issue-Key: apigateway__dynamodb__lambda__logs.cli-lambda-update-env-vars-dynamodb-table-name|_run_shell#13|AssertionError|raise AssertionError(
Issue-Key: apigateway__dynamodb__lambda__logs.cli-lambda-update-function-code-then-invoke-logs-check|_run_shell#6|AssertionError|raise AssertionError(
Issue-Key: apigateway__dynamodb__lambda__logs.cli-lambda-invoke-missing-permission-negative|run#27|AssertionError|assert match.group(1) == "AccessDeniedException", (
Issue-Key: apigateway__dynamodb__lambda__logs.iam-role-wrong-principal-service-negative|run#15|AssertionError|assert code == "InvalidParameterValueException", (
Issue-Commit: f9ff4020ff731d5f2129f1ceb9f16685ad9310db

# apigateway__dynamodb__lambda__logs: four causes — GetAccount and Lambda Invoke are 501 stubs, Invoke answers NotImplemented instead of AccessDeniedException, and Lambda accepts a role whose trust policy names the wrong service principal

> _Note: this issue was identified against the code base at commit `f9ff4020ff731d5f2129f1ceb9f16685ad9310db`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Four distinct root causes across six failing checks in this service.

Cause 1 of 4: apigateway GetAccount answers 501 NotImplemented.
AWS: Smithy GetAccount (GET /account, success 200) returns Account (cloudwatchRoleArn, throttleSettings, features, apiKeyVersion); modeled errors are BadRequestException (400), NotFoundException (404), TooManyRequestsException (429), UnauthorizedException (401).
Emulator: HTTP 501, error code NotImplemented ('operation GetAccount is scaffolded but not implemented in this emulator'); through the CLI this surfaces as rc=254.
Why: a client that reads the account-level CloudWatch role before configuring stage access logging gets an unmodeled 501 instead of the Account document, so the whole access-log-destination validation flow cannot start. Existing issue 20261023T125454Z-apigateway-getaccount-stub.md names the stub but does not carry this replay key.
Fix: the GetAccount handler (services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/get_account.rs).
Traces: cli-apigateway-stage-access-log-missing-destination-rejected.

Cause 2 of 4: lambda Invoke answers 501 NotImplemented for every real invocation.
AWS: Invoke returns HTTP 200 carrying the function's response payload (or HTTP 200 with X-Amz-Function-Error when the handler raises).
Emulator: HTTP 501, code NotImplemented — 'Invoke resolved <function arn> at version $LATEST, but running its python3.12 deployment package is not implemented in this emulator'; CLI rc=254.
Why: no function body ever runs, so DynamoDB reads, env-var reads and updated code cannot be observed through the function's own output; a client receives a 501 where AWS always returns 200.
Fix: the Invoke handler's execution path (lambda service src/ops/invoke.rs). Related open issue 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md covers the family but not these replay keys.
Traces: cli-dynamodb-put-item-then-lambda-scan, cli-lambda-update-env-vars-dynamodb-table-name, cli-lambda-update-function-code-then-invoke-logs-check.

Cause 3 of 4: Invoke answers NotImplemented where AWS answers AccessDeniedException for a caller without lambda:InvokeFunction.
AWS: an Invoke by a principal whose policy does not allow lambda:InvokeFunction is refused with HTTP 403 and error code AccessDeniedException, before any function execution is attempted.
Emulator: the CLI reports error code 'NotImplemented' instead — i.e. the authorization decision is never made because execution is attempted (and refused) first.
Why: the ordering is wrong and the code is wrong: a client testing that its IAM policy denies invocation sees NotImplemented and cannot distinguish 'denied' from 'unsupported'. Negative IAM tests silently pass or fail for the wrong reason.
Fix: evaluate the caller's invoke authorization in the Invoke handler before the execution path, and return the modeled 403 AccessDeniedException (lambda service src/ops/invoke.rs).
Traces: cli-lambda-invoke-missing-permission-negative.

Cause 4 of 4: re-pointing a function at a role whose trust policy names ec2.amazonaws.com is accepted instead of refused.
AWS: Lambda refuses a function role whose trust policy does not allow lambda.amazonaws.com to assume it, with HTTP 400 and error code InvalidParameterValueException. This applies to UpdateFunctionConfiguration as well as CreateFunction.
Emulator: the call is accepted — the check reports that no error was raised at all ('expected InvalidParameterValueException ... got None'), so the function ends up configured with a role Lambda could never assume.
Why: a client's misconfigured execution role is accepted at write time, so a defect real AWS surfaces immediately as a 400 instead appears much later as an unexplained runtime failure; negative IaC tests that assert the rejection pass against the emulator and fail against AWS.
Fix: trust-policy principal validation for the Role member in the UpdateFunctionConfiguration handler (and the shared role-validation helper it should share with CreateFunction).
Traces: iam-role-wrong-principal-service-negative.

## Evidence
Cause 1: aws: [ERROR]: An error occurred (NotImplemented) when calling the GetAccount operation: operation GetAccount is scaffolded but not implemented in this emulator; cmd ['aws','--output','json','apigateway','get-account']; rc=254 (report lines 82773-82776). Expected HTTP 200 with an Account document.
Cause 2: aws: [ERROR]: An error occurred (NotImplemented) when calling the Invoke operation: Invoke resolved arn:aws:lambda:...:function:... at version $LATEST, but running its python3.12 deployment package is not implemented in this emulator; rc=254 (lines 83122-83125, 83869-83872, 84121-84124). Expected HTTP 200 with the function payload.
Cause 3: AssertionError: expected AccessDeniedException, CLI reported 'NotImplemented' (line 83404). Expected HTTP 403 / AccessDeniedException.
Cause 4: AssertionError: expected InvalidParameterValueException re-pointing the function at an ec2.amazonaws.com-trusted role, got None (line 90536). Expected HTTP 400 / InvalidParameterValueException.

## Steps to Reproduce
Cause 1: aws apigateway get-account -> expect 200 with cloudwatchRoleArn/throttleSettings, observe rc=254 / 501 NotImplemented.
Cause 2: create a python3.12 function and aws lambda invoke --function-name <fn> out.json -> expect 200 with the payload, observe rc=254 / 501 NotImplemented.
Cause 3: invoke a function as a principal with no lambda:InvokeFunction permission -> expect 403 AccessDeniedException, observe NotImplemented.
Cause 4: create an IAM role whose trust policy allows only ec2.amazonaws.com, then aws lambda update-function-configuration --function-name <fn> --role <that role arn> -> expect 400 InvalidParameterValueException, observe success.
Re-replay the six check ids in trace_ids.
