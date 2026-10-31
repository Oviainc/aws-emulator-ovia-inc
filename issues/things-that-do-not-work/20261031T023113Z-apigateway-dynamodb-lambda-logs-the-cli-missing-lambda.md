Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda__logs.cli-apigateway-missing-lambda-permission-rejected
Issue-Key: apigateway__dynamodb__lambda__logs.cli-apigateway-missing-lambda-permission-rejected|_wait_for_permission_effective#31|Timeout|time.sleep(2)
Issue-Commit: 4a88e8365a42250444744c30ae29769cc0d2c461

# apigateway__dynamodb__lambda__logs: the CLI missing-Lambda-permission check reaches no verdict — TestInvokeMethod never answers status 200 after lambda:InvokeFunction is granted, so the run exhausts the 60 s cap (1 cause)

> _Note: this issue was identified against the code base at commit `4a88e8365a42250444744c30ae29769cc0d2c461`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TestInvokeMethod

## Description
Cause 1 of 1: after the Lambda resource policy that authorizes API Gateway is added, TestInvokeMethod on the same AWS_PROXY method never converges to status 200, so the positive control of the missing-permission check never completes and the run is killed at the harness cap.

AWS: Smithy (com-amazonaws-apigateway-backplanecontrolservice.json) models TestInvokeMethod as POST /restapis/{restApiId}/resources/{resourceId}/methods/{httpMethod} with output members status (Integer, @default 0), body, headers, multiValueHeaders, log and latency, and errors BadRequestException (400), NotFoundException (404), UnauthorizedException (401), TooManyRequestsException (429). The permission semantics are recorded behavior only: with an AWS_PROXY integration whose uri is arn:aws:apigateway:{region}:lambda:path/2015-03-31/functions/{fn-arn}/invocations and NO lambda:InvokeFunction statement for principal apigateway.amazonaws.com, TestInvokeMethod answers status 500, its body does not carry anything the function produced, and the function does not execute (no event in /aws/lambda/{function-name}). After `lambda add-permission --action lambda:InvokeFunction --principal apigateway.amazonaws.com --source-arn arn:aws:execute-api:{region}:{account-id}:{api-id}/*/GET/echo`, the identical TestInvokeMethod answers status 200 with the function's body, and the invocation's printed line appears in the function's log group and is returned by logs FilterLogEvents. The status/code/message values here are the recorded ones; the ARNs, ids and names above are template slots filled from the request and account, not literals.

Emulator: the check never produces a verdict — the only replay output is `Error: Check timed out after 60.0s`. The two convergence loops AWS satisfies within seconds both run to their deadlines: the positive control polls apigateway test-invoke-method for status 200 for up to 30 s and never sees it, and the log leg polls logs filter-log-events for the handler's marker for up to 60 s and never sees it. Together they overrun the 60 s harness cap, so the assertions on the granted-permission 200, on the function body marker, and on the execution log line are never evaluated. Related: the suite separately reports FilterLogEvents as a scaffolded-but-unimplemented stub (services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/filter_log_events.rs) with its own issue already filed — that stub is why the log-marker leg cannot converge and must NOT be filed again; the net-new part of this cause is TestInvokeMethod on an AWS_PROXY method never reaching status 200 once the resource policy allows the invoke.

Why: a client that uses TestInvokeMethod to verify its Lambda permission wiring (the standard console/CLI loop) can never observe the transition from the refused 500 to the authorized 200, so the permission boundary is indistinguishable from a broken integration, and an emulator user cannot tell a correctly-granted policy from a missing one.

Fix: the TestInvokeMethod handler and the integration-execution path it drives for AWS_PROXY integrations — it must actually invoke the target function when the function's resource policy admits apigateway.amazonaws.com for the method's source ARN and return status 200 with the function's body (and the emitted log in the `log` member). Open issues 20261030T003023Z-apigateway-dynamodb-iam-lambda-testinvokemethod-never-returns-so-the-stage-scoped-lambda.md and 20261030T191000Z-apigateway-dynamodb-iam-lambda-the-testinvokemethod-vs-lambda.md describe the same symptom for a different service token and do not carry this check's replay key, so this check is filed here.

Traces: apigateway__dynamodb__lambda__logs.cli-apigateway-missing-lambda-permission-rejected (verdict: mismatch, `Check timed out after 60.0s`).

## Evidence
Expected (AWS): (a) before add-permission, `aws apigateway test-invoke-method --rest-api-id <id> --resource-id <id> --http-method GET` returns status 500 with no function output and no log event for the marker; (b) after `aws lambda add-permission --function-name <fn> --statement-id apigw-invoke --action lambda:InvokeFunction --principal apigateway.amazonaws.com --source-arn arn:aws:execute-api:<region>:<account>:<api-id>/*/GET/echo`, the identical test-invoke-method returns status 200 with the function's body containing the handler's marker, and `aws logs filter-log-events --log-group-name /aws/lambda/<fn>` returns the handler's printed marker line.

Actual (emulator): no status is ever obtained for the authorized call. test-invoke-method is polled for status 200 for 30 s without converging and filter-log-events is polled for the marker for up to 60 s without converging; the harness terminates the run with `Error: Check timed out after 60.0s`. The report also lists FilterLogEvents as an unimplemented scaffold stub (already filed separately), which accounts for the log leg.

Grounding: failing check apigateway__dynamodb__lambda__logs.cli-apigateway-missing-lambda-permission-rejected in the behavioral suite report; it carries neither an "ALREADY FILED" nor an "OWNED by" marker.

## Steps to Reproduce
1. Create an IAM role trusting lambda.amazonaws.com with AWSLambdaBasicExecutionRole attached.
2. `aws lambda create-function` (python3.12) with a handler that prints a marker and returns {"statusCode": 200, "body": {"marker": "<marker>"}}; wait for State=Active.
3. `aws apigateway create-rest-api`; `get-resources` to find the root id; `create-resource --path-part echo`; `put-method --http-method GET --authorization-type NONE`.
4. `aws apigateway put-integration --type AWS_PROXY --integration-http-method POST --uri arn:aws:apigateway:<region>:lambda:path/2015-03-31/functions/<fn-arn>/invocations`; `create-deployment --stage-name test`.
5. `aws apigateway test-invoke-method --rest-api-id <id> --resource-id <id> --http-method GET` with NO lambda permission granted. Expected status 500 and no function execution.
6. `aws lambda add-permission --statement-id apigw-invoke --action lambda:InvokeFunction --principal apigateway.amazonaws.com --source-arn arn:aws:execute-api:<region>:<account>:<api-id>/*/GET/echo`.
7. Re-run the identical test-invoke-method. Expected status 200 with the function's body; observed: 200 is never returned within the 30 s convergence window.
8. `aws logs filter-log-events --log-group-name /aws/lambda/<fn> --filter-pattern <marker>`. Expected the handler's marker line; observed: nothing (FilterLogEvents is an unimplemented stub — already filed separately). The combined windows exceed the 60 s cap and the check times out with no verdict.
