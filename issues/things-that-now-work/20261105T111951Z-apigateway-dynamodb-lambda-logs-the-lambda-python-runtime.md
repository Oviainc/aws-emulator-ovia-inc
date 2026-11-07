Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda__logs.cli-dynamodb-put-item-then-lambda-scan apigateway__dynamodb__lambda__logs.cli-lambda-invoke-missing-permission-negative apigateway__dynamodb__lambda__logs.cli-lambda-update-env-vars-dynamodb-table-name apigateway__dynamodb__lambda__logs.deploy-cfn-missing-dynamodb-table-arn-env-negative apigateway__dynamodb__lambda__logs.lambda-apigateway-create-deployment-positive apigateway__dynamodb__lambda__logs.lambda-apigateway-cross-account-negative apigateway__dynamodb__lambda__logs.lambda-apigateway-get-rest-apis-positive apigateway__dynamodb__lambda__logs.lambda-apigateway-missing-permission-negative apigateway__dynamodb__lambda__logs.lambda-apigateway-update-integration-positive apigateway__dynamodb__lambda__logs.lambda-apigateway-wrong-api-id-negative apigateway__dynamodb__lambda__logs.lambda-apigateway-wrong-rest-api-id-negative apigateway__dynamodb__lambda__logs.lambda-concurrent-invocations-distinct-log-streams apigateway__dynamodb__lambda__logs.lambda-create-apigateway-resource-via-sdk-positive apigateway__dynamodb__lambda__logs.lambda-describe-rest-apis-via-sdk-positive apigateway__dynamodb__lambda__logs.lambda-get-deployment-details-positive apigateway__dynamodb__lambda__logs.lambda-logs-missing-putlogevents-permission-negative apigateway__dynamodb__lambda__logs.lambda-put-log-events-via-sdk-positive apigateway__dynamodb__lambda__logs.lambda-update-apigateway-stage-variables-positive apigateway__dynamodb__lambda__logs.cli-lambda-tail-logs-after-invoke apigateway__dynamodb__lambda__logs.cli-lambda-update-function-code-then-invoke-logs-check apigateway__dynamodb__lambda__logs.apigw-http-api-lambda-missing-permission-negative apigateway__dynamodb__lambda__logs.iam-role-trusts-lambda-service-positive apigateway__dynamodb__lambda__logs.lambda-custom-logger-writes-to-cloudwatch apigateway__dynamodb__lambda__logs.lambda-error-stack-trace-in-logs apigateway__dynamodb__lambda__logs.lambda-invocation-creates-log-group apigateway__dynamodb__lambda__logs.lambda-invocation-writes-log-stream-events apigateway__dynamodb__lambda__logs.lambda-invoke-creates-log-group apigateway__dynamodb__lambda__logs.lambda-log-group-explicit-arn-mismatch-negative apigateway__dynamodb__lambda__logs.lambda-log-retention-configuration apigateway__dynamodb__lambda__logs.lambda-log-retention-set-via-put-retention-policy apigateway__dynamodb__lambda__logs.lambda-log-stream-contains-invocation-logs apigateway__dynamodb__lambda__logs.lambda-logs-correlation-request-id apigateway__dynamodb__lambda__logs.lambda-logs-error-stack-trace-format apigateway__dynamodb__lambda__logs.lambda-missing-logs-permission-negative apigateway__dynamodb__lambda__logs.lambda-missing-logs-permission-no-log-group-negative
Issue-Key: apigateway__dynamodb__lambda__logs.cli-dynamodb-put-item-then-lambda-scan|run#20|KeyError|matches = [it for it in invoke_result["items"] if it.get("id", {}).get("S") == item_id]
Issue-Key: apigateway__dynamodb__lambda__logs.cli-lambda-invoke-missing-permission-negative|run#25|AssertionError|assert proc.returncode != 0, "CLI invoke with no lambda permissions unexpectedly succeeded"
Issue-Key: apigateway__dynamodb__lambda__logs.cli-lambda-update-env-vars-dynamodb-table-name|run#17|AssertionError|assert "Item" in resp_a, f"expected row-a in {table_a} (TABLE_NAME target) after invoke, got {resp_a}"
Issue-Key: apigateway__dynamodb__lambda__logs.deploy-cfn-missing-dynamodb-table-arn-env-negative|run#10|KeyError|assert payload["ok"] is True, f"positive control failed: {payload}"
Issue-Key: apigateway__dynamodb__lambda__logs.lambda-apigateway-create-deployment-positive|_invoke#5|AssertionError|raise AssertionError(f"lambda invoke error: {body}")
Issue-Key: apigateway__dynamodb__lambda__logs.lambda-apigateway-cross-account-negative|_invoke#10|AssertionError|raise AssertionError(f"lambda invoke error: {body}")
Issue-Key: apigateway__dynamodb__lambda__logs.lambda-apigateway-get-rest-apis-positive|_invoke#8|AssertionError|raise AssertionError(f"lambda invoke error: {body}")
Issue-Key: apigateway__dynamodb__lambda__logs.lambda-apigateway-missing-permission-negative|_invoke#11|AssertionError|raise AssertionError(f"lambda invoke raised unhandled error: {body}")
Issue-Key: apigateway__dynamodb__lambda__logs.lambda-apigateway-update-integration-positive|_invoke#13|AssertionError|raise AssertionError(f"lambda invoke error: {body}")
Issue-Key: apigateway__dynamodb__lambda__logs.lambda-apigateway-wrong-api-id-negative|_invoke#8|AssertionError|raise AssertionError(f"lambda invoke error: {body}")
Issue-Key: apigateway__dynamodb__lambda__logs.lambda-apigateway-wrong-rest-api-id-negative|_invoke#18|AssertionError|raise AssertionError(f"lambda invoke raised unhandled error: {body}")
Issue-Key: apigateway__dynamodb__lambda__logs.lambda-concurrent-invocations-distinct-log-streams|_poll_until#11|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s; last observed: {result!r}")
Issue-Key: apigateway__dynamodb__lambda__logs.lambda-create-apigateway-resource-via-sdk-positive|_invoke#11|AssertionError|raise AssertionError(f"lambda invoke raised unhandled error: {body}")
Issue-Key: apigateway__dynamodb__lambda__logs.lambda-describe-rest-apis-via-sdk-positive|_invoke#9|AssertionError|raise AssertionError(f"lambda invoke raised unhandled error: {body}")
Issue-Key: apigateway__dynamodb__lambda__logs.lambda-get-deployment-details-positive|_invoke#17|AssertionError|raise AssertionError(f"lambda invoke raised unhandled error: {body}")
Issue-Key: apigateway__dynamodb__lambda__logs.lambda-logs-missing-putlogevents-permission-negative|run#17|AssertionError|assert payload2 == {"ok": True}, f"expected positive control to report ok=True, got {payload2}"
Issue-Key: apigateway__dynamodb__lambda__logs.lambda-put-log-events-via-sdk-positive|run#15|AssertionError|assert "FunctionError" not in resp, f"unexpected FunctionError on invoke: {resp.get('FunctionError')}"
Issue-Key: apigateway__dynamodb__lambda__logs.lambda-update-apigateway-stage-variables-positive|_invoke#20|AssertionError|raise AssertionError(f"lambda invoke raised unhandled error: {body}")
Issue-Key: apigateway__dynamodb__lambda__logs.cli-lambda-tail-logs-after-invoke|_wait_for_log_marker#11|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__dynamodb__lambda__logs.cli-lambda-update-function-code-then-invoke-logs-check|_wait_for_log_marker#10|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__dynamodb__lambda__logs.apigw-http-api-lambda-missing-permission-negative|_poll_for_log_latency#21|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__dynamodb__lambda__logs.iam-role-trusts-lambda-service-positive|_poll_log_marker#24|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__dynamodb__lambda__logs.lambda-custom-logger-writes-to-cloudwatch|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__dynamodb__lambda__logs.lambda-error-stack-trace-in-logs|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__dynamodb__lambda__logs.lambda-invocation-creates-log-group|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__dynamodb__lambda__logs.lambda-invocation-writes-log-stream-events|_poll_until#8|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__dynamodb__lambda__logs.lambda-invoke-creates-log-group|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__dynamodb__lambda__logs.lambda-log-group-explicit-arn-mismatch-negative|_poll_until#17|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__dynamodb__lambda__logs.lambda-log-retention-configuration|_poll_until#8|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__dynamodb__lambda__logs.lambda-log-retention-set-via-put-retention-policy|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__dynamodb__lambda__logs.lambda-log-stream-contains-invocation-logs|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__dynamodb__lambda__logs.lambda-logs-correlation-request-id|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__dynamodb__lambda__logs.lambda-logs-error-stack-trace-format|_poll_until#16|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__dynamodb__lambda__logs.lambda-missing-logs-permission-negative|_poll_until#17|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__dynamodb__lambda__logs.lambda-missing-logs-permission-no-log-group-negative|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Commit: a0ca0e6d405fa8f821a988e0d013d6063af395f8

# apigateway__dynamodb__lambda__logs: the Lambda python runtime has no AWS SDK, and an invocation writes nothing to its CloudWatch log group

> _Note: this issue was identified against the code base at commit `a0ca0e6d405fa8f821a988e0d013d6063af395f8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 2 — the managed python runtime ships no boto3/botocore.
AWS: the python3.x managed runtimes bundle boto3/botocore; a handler may `import boto3` and call apigateway:*, dynamodb:* and logs:PutLogEvents under its execution role, and Invoke returns its payload with no FunctionError.
Emulator: HTTP 200, FunctionError='Unhandled', payload {"errorMessage":"Unable to import module '<module>': No module named 'boto3'","errorType":"Runtime.ImportModuleError"}; checks that then index the expected result raise KeyError (e.g. KeyError: 'items').
Why: no in-function AWS call can be made, so neither the positive flows nor their IAM-denial controls are observable.
Fix: the python runtime's bundled site-packages / module search path in the lambda runtime component.
Traces: the lambda-apigateway-*, lambda-*-via-sdk, cli-* and deploy-cfn-missing-dynamodb-table-arn-env-negative ids in trace_ids.

Cause 2 of 2 — an invocation creates no /aws/lambda/<function-name> log group and writes no log events, so every log-backed assertion polls to its deadline.
AWS: the first invocation of a function creates /aws/lambda/<function-name> (unless LoggingConfig.LogGroup names another), opens a log stream named <YYYY/MM/DD>/[<version>]<hex> and writes START/END/REPORT framing plus the handler's stdout/stderr; FilterLogEvents/GetLogEvents expose those lines within seconds, and when the execution role lacks logs:CreateLogGroup/PutLogEvents no group is created at all.
Emulator: recorded behavior only — nothing appears: no group, no stream, no events, so each check never satisfies its predicate and the harness reports "Check timed out after 60.0s".
Why: a client cannot observe an invocation's output, its duration, its stack trace or its request-id correlation; retention configuration and the missing-permission negatives are equally unobservable.
Fix: the Lambda->CloudWatch Logs delivery path (implicit log-group creation, stream naming, START/END/REPORT emission) in the invoke path.
Traces: the "Check timed out after 60.0s" ids in trace_ids (cli-lambda-tail-logs-after-invoke, lambda-invocation-creates-log-group, lambda-log-stream-contains-invocation-logs and the rest).

## Evidence
Cause 1: AssertionError: lambda invoke error: {'errorMessage': "Unable to import module 'index': No module named 'boto3'", 'errorType': 'Runtime.ImportModuleError'}; KeyError: 'items' where the check read invoke_result['items'].
Cause 2: Error: Check timed out after 60.0s on every check that polls DescribeLogGroups/DescribeLogStreams/FilterLogEvents for /aws/lambda/<fn> after an Invoke. Expected: the group exists, a dated stream exists, and START/END/REPORT plus handler output are readable.

## Steps to Reproduce
1. CreateFunction (python3.12) importing boto3 and calling apigateway/dynamodb/logs, then Invoke -> Runtime.ImportModuleError (cause 1).
2. CreateFunction with a handler that prints, Invoke it, then DescribeLogGroups(logGroupNamePrefix='/aws/lambda/<fn>') and FilterLogEvents -> nothing ever appears (cause 2).
