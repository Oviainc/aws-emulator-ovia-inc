Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__events__lambda.cfn-lambda-dynamodb-table-permissions-deployment dynamodb__events__lambda.cfn-lambda-role-missing-eventbridge-permission-negative dynamodb__events__lambda.cli-lambda-role-policy-missing-dynamodb-negative-workflow dynamodb__events__lambda.lambda-describe-rule-missing-permission-negative dynamodb__events__lambda.lambda-dynamodb-batchwriteitem-positive dynamodb__events__lambda.lambda-dynamodb-deleteitem-missing-permission-negative dynamodb__events__lambda.lambda-dynamodb-query-missing-permission-negative dynamodb__events__lambda.lambda-dynamodb-scan-with-filter-positive dynamodb__events__lambda.lambda-dynamodb-updateitem-conditional-check-positive dynamodb__events__lambda.lambda-execution-role-trusts-lambda-service-positive dynamodb__events__lambda.lambda-putevents-custom-bus-positive dynamodb__events__lambda.lambda-putevents-default-bus-positive dynamodb__events__lambda.lambda-putevents-missing-permission-negative dynamodb__events__lambda.lambda-putevents-triggers-downstream-rule-positive dynamodb__events__lambda.lambda-putevents-wrong-event-bus-arn-negative dynamodb__events__lambda.lambda-role-wrong-trust-principal-negative
Issue-Key: dynamodb__events__lambda.cfn-lambda-dynamodb-table-permissions-deployment|_invoke#8|AssertionError|assert "FunctionError" not in resp, f"lambda invoke error: {body}"
Issue-Key: dynamodb__events__lambda.cfn-lambda-role-missing-eventbridge-permission-negative|run#10|AssertionError|assert "FunctionError" not in resp, f"unexpected lambda runtime error: {body}"
Issue-Key: dynamodb__events__lambda.cli-lambda-role-policy-missing-dynamodb-negative-workflow|_invoke_with_retry#16|Timeout|time.sleep(interval_s)
Issue-Key: dynamodb__events__lambda.lambda-describe-rule-missing-permission-negative|_invoke_lambda#13|AssertionError|raise AssertionError(f"lambda invocation raised an unhandled error: {body}")
Issue-Key: dynamodb__events__lambda.lambda-dynamodb-batchwriteitem-positive|_invoke#6|AssertionError|raise AssertionError(f"lambda invocation error: {body}")
Issue-Key: dynamodb__events__lambda.lambda-dynamodb-deleteitem-missing-permission-negative|_invoke#8|AssertionError|raise AssertionError(f"lambda invocation error: {body}")
Issue-Key: dynamodb__events__lambda.lambda-dynamodb-query-missing-permission-negative|_invoke#8|AssertionError|raise AssertionError(f"lambda invocation error: {body}")
Issue-Key: dynamodb__events__lambda.lambda-dynamodb-scan-with-filter-positive|_invoke#10|AssertionError|raise AssertionError(f"lambda invocation error: {body}")
Issue-Key: dynamodb__events__lambda.lambda-dynamodb-updateitem-conditional-check-positive|_invoke#7|AssertionError|raise AssertionError(f"lambda invocation error: {body}")
Issue-Key: dynamodb__events__lambda.lambda-execution-role-trusts-lambda-service-positive|run#13|AssertionError|assert "FunctionError" not in invoke_resp, f"invocation errored: {invoke_resp!r}"
Issue-Key: dynamodb__events__lambda.lambda-putevents-custom-bus-positive|_invoke_lambda#14|AssertionError|raise AssertionError(f"lambda invocation raised an unhandled error: {body}")
Issue-Key: dynamodb__events__lambda.lambda-putevents-default-bus-positive|_invoke_lambda#13|AssertionError|raise AssertionError(f"lambda invocation raised an unhandled error: {body}")
Issue-Key: dynamodb__events__lambda.lambda-putevents-missing-permission-negative|_invoke_lambda#15|AssertionError|raise AssertionError(f"lambda invocation raised an unhandled error: {body}")
Issue-Key: dynamodb__events__lambda.lambda-putevents-triggers-downstream-rule-positive|_invoke_lambda#22|AssertionError|raise AssertionError(f"lambda invocation raised an unhandled error: {body}")
Issue-Key: dynamodb__events__lambda.lambda-putevents-wrong-event-bus-arn-negative|_invoke_lambda#15|AssertionError|raise AssertionError(f"lambda invocation raised an unhandled error: {body}")
Issue-Key: dynamodb__events__lambda.lambda-role-wrong-trust-principal-negative|run#23|AssertionError|assert "FunctionError" not in invoke_resp, invoke_resp
Issue-Commit: 6da489cbc4c2969d809761cd42ffd786edefa992

# dynamodb__events__lambda: the Lambda python runtime provides no AWS SDK, so every boto3-importing handler faults Runtime.ImportModuleError and 16 in-function DynamoDB/EventBridge checks fail

> _Note: this issue was identified against the code base at commit `6da489cbc4c2969d809761cd42ffd786edefa992`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1: the python3.x Lambda invoke runtime has no boto3/botocore on the handler's import path, so every function these checks deploy — each of which calls DynamoDB (PutItem/BatchWriteItem/Query/Scan/UpdateItem/DeleteItem) or EventBridge (PutEvents/DescribeRule) through the AWS SDK — fails at module import and its body never executes. Failing checks: dynamodb__events__lambda.cfn-lambda-dynamodb-table-permissions-deployment, dynamodb__events__lambda.cfn-lambda-role-missing-eventbridge-permission-negative, dynamodb__events__lambda.cli-lambda-role-policy-missing-dynamodb-negative-workflow, dynamodb__events__lambda.lambda-describe-rule-missing-permission-negative, dynamodb__events__lambda.lambda-dynamodb-batchwriteitem-positive, dynamodb__events__lambda.lambda-dynamodb-deleteitem-missing-permission-negative, dynamodb__events__lambda.lambda-dynamodb-query-missing-permission-negative, dynamodb__events__lambda.lambda-dynamodb-scan-with-filter-positive, dynamodb__events__lambda.lambda-dynamodb-updateitem-conditional-check-positive, dynamodb__events__lambda.lambda-execution-role-trusts-lambda-service-positive, dynamodb__events__lambda.lambda-putevents-custom-bus-positive, dynamodb__events__lambda.lambda-putevents-default-bus-positive, dynamodb__events__lambda.lambda-putevents-missing-permission-negative, dynamodb__events__lambda.lambda-putevents-triggers-downstream-rule-positive, dynamodb__events__lambda.lambda-putevents-wrong-event-bus-arn-negative, dynamodb__events__lambda.lambda-role-wrong-trust-principal-negative.

AWS: AWS Lambda's managed python3.x runtimes ship the AWS SDK for Python (boto3/botocore) pre-installed on the function's sys.path, so `import boto3` at module scope succeeds and the handler can call DynamoDB and EventBridge with its execution-role credentials. Smithy (lambda, Invoke): FunctionError is bound to the response header `X-Amz-Function-Error`, StatusCode to the HTTP response code; a successful synchronous Invoke answers HTTP 200 with NO `X-Amz-Function-Error` header and Payload equal to the handler's return value. `FunctionError: Unhandled` with an {errorMessage,errorType,requestId,stackTrace} payload is reserved for a fault in customer code — and for the negative checks here the handler is expected to run and report the service's own AccessDeniedException back in its result.

Emulator: every such Invoke answers HTTP 200 with `x-amz-function-error: Unhandled` (header observed verbatim as `x-amz-function-error: Unhandled`, `x-amz-executed-version: $LATEST`) and the payload is the runtime's own import failure, not the handler's result: errorType `Runtime.ImportModuleError`, errorMessage `Unable to import module '<module>': No module named 'boto3'`. The fixed text is `Unable to import module '` + `': No module named 'boto3'`; `<module>` is the handler module from the function's Handler setting (observed as `index`) and requestId is the per-invocation id. Because the body never runs, no item is ever written or read, no PutEvents is ever made, and the IAM decision the negative checks assert on is never reached; dynamodb__events__lambda.cli-lambda-role-policy-missing-dynamodb-negative-workflow instead polls for an effect that never appears and ends with `Check timed out after 60.0s` (Hypothesis: same cause — it deploys and invokes the same shape of boto3-importing handler via the CLI, and the timeout line carries no further diagnosis).

Why: a client that packages an ordinary python handler using boto3 gets a function that cannot run at all, so the whole Lambda->DynamoDB and Lambda->EventBridge interop surface is unobservable: positive paths write nothing, and negative (permission-boundary) paths cannot be distinguished from a broken runtime because the emulator reports the fault as the customer's code faulting rather than as a missing SDK.

Fix: the Lambda invoke runtime/sandbox that backs the python3.x runtimes must expose the AWS SDK on the handler's import path (services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/invoke.rs and the python runtime bundle it loads).

Related open issues describe the same runtime gap under other service groupings (20261105T081139Z-dynamodb-lambda-s3-secretsmanager-the-in-process-lambda.md, 20261105T081225Z-events-lambda-ssm-the-lambda-python-runtime-has.md); the open dynamodb__events__lambda issues (e.g. 20261029T180026Z-dynamodb-events-lambda-eventbridge-rules-never-invoke-their-lambda-targets-ddb.md) do not carry the replay keys of these sixteen checks.

Traces: see trace_ids.

## Evidence
Expected (AWS): Invoke -> HTTP 200, no `X-Amz-Function-Error` header, Payload = the handler's own JSON result (the written/read items, the PutEvents EventId, or — for the negative checks — the AccessDeniedException the SDK call raised).

Actual (emulator): HTTP 200 with `x-amz-function-error: Unhandled`, `FunctionError: 'Unhandled'`, Payload = {"errorMessage":"Unable to import module 'index': No module named 'boto3'","errorType":"Runtime.ImportModuleError","requestId":"<per-invocation id>","stackTrace":[]}. Representative assertions:
- cfn-lambda-dynamodb-table-permissions-deployment line 32: `AssertionError: lambda invoke error: {'errorMessage': "Unable to import module 'index': No module named 'boto3'", 'errorType': 'Runtime.ImportModuleError', ...}`
- lambda-dynamodb-batchwriteitem-positive line 147 / lambda-dynamodb-scan-with-filter-positive line 145 / lambda-dynamodb-updateitem-conditional-check-positive line 194 / lambda-dynamodb-deleteitem-missing-permission-negative line 201: `AssertionError: lambda invocation error: {... Runtime.ImportModuleError ...}`
- lambda-putevents-custom-bus-positive line 172 / lambda-putevents-default-bus-positive line 171 / lambda-putevents-missing-permission-negative line 187 / lambda-putevents-triggers-downstream-rule-positive line 203 / lambda-putevents-wrong-event-bus-arn-negative line 194 / lambda-describe-rule-missing-permission-negative line 194: `AssertionError: lambda invocation raised an unhandled error: {... Runtime.ImportModuleError ...}`
- lambda-execution-role-trusts-lambda-service-positive line 61 / lambda-role-wrong-trust-principal-negative line 72 / cfn-lambda-role-missing-eventbridge-permission-negative line 43: FunctionError 'Unhandled' present on a 200 response
- cli-lambda-role-policy-missing-dynamodb-negative-workflow: `Check timed out after 60.0s`.

## Steps to Reproduce
1. CreateFunction (runtime python3.x) from a zip whose index.py does `import boto3` and calls DynamoDB or EventBridge.
2. Invoke it synchronously.
3. Observe HTTP 200 with `x-amz-function-error: Unhandled` and a Runtime.ImportModuleError payload naming boto3; the table is unchanged and no event is published.
4. Re-replay the sixteen checks in trace_ids to confirm.
