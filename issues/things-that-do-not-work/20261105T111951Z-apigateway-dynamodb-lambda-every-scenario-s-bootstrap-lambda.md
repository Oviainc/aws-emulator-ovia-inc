Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda.apigw-authorizer-deny-integration-not-invoked apigateway__dynamodb__lambda.apigw-lambda-authorizer-missing-permission-negative apigateway__dynamodb__lambda.apigw-lambda-ddb-pipeline-missing-ddb-permission-negative apigateway__dynamodb__lambda.apigw-lambda-ddb-update-pipeline-positive apigateway__dynamodb__lambda.apigw-lambda-env-table-arn-resolution apigateway__dynamodb__lambda.apigw-lambda-missing-invoke-permission-negative apigateway__dynamodb__lambda.apigw-lambda-payload-format-preservation apigateway__dynamodb__lambda.apigw-lambda-proxy-positive apigateway__dynamodb__lambda.apigw-lambda-response-passthrough-format apigateway__dynamodb__lambda.apigw-wrong-lambda-arn-integration-negative apigateway__dynamodb__lambda.ddb-stream-lambda-batch-items-data-format apigateway__dynamodb__lambda.ddb-stream-lambda-delete-event-format apigateway__dynamodb__lambda.ddb-stream-lambda-esm-missing-stream-permission-negative apigateway__dynamodb__lambda.ddb-stream-lambda-esm-positive apigateway__dynamodb__lambda.lambda-ddb-conditional-write-positive apigateway__dynamodb__lambda.lambda-ddb-missing-putitem-permission-negative apigateway__dynamodb__lambda.lambda-ddb-put-get-positive apigateway__dynamodb__lambda.lambda-ddb-query-gsi-positive apigateway__dynamodb__lambda.lambda-ddb-scan-no-permission-negative apigateway__dynamodb__lambda.lambda-ddb-transact-write-no-permission-negative apigateway__dynamodb__lambda.lambda-describe-apigateway-positive
Issue-Key: apigateway__dynamodb__lambda.apigw-authorizer-deny-integration-not-invoked|run#13|AssertionError|assert not result.get("error"), f"API Gateway bootstrap failed: {result}"
Issue-Key: apigateway__dynamodb__lambda.apigw-lambda-authorizer-missing-permission-negative|run#11|RuntimeError|raise RuntimeError(f"Authorizer API setup failed: {result}")
Issue-Key: apigateway__dynamodb__lambda.apigw-lambda-ddb-pipeline-missing-ddb-permission-negative|_create_table_via_lambda#3|RuntimeError|raise RuntimeError(f"Bootstrap Lambda error: {result}")
Issue-Key: apigateway__dynamodb__lambda.apigw-lambda-ddb-update-pipeline-positive|_create_table_via_lambda#3|RuntimeError|raise RuntimeError(f"Bootstrap Lambda error: {result}")
Issue-Key: apigateway__dynamodb__lambda.apigw-lambda-env-table-arn-resolution|_create_table_via_lambda#3|RuntimeError|raise RuntimeError(f"Bootstrap Lambda error: {result}")
Issue-Key: apigateway__dynamodb__lambda.apigw-lambda-missing-invoke-permission-negative|_deploy_lambda_api#2|RuntimeError|raise RuntimeError(f"API setup Lambda failed: {result}")
Issue-Key: apigateway__dynamodb__lambda.apigw-lambda-payload-format-preservation|_create_table_via_lambda#3|RuntimeError|raise RuntimeError(f"Bootstrap Lambda error: {result}")
Issue-Key: apigateway__dynamodb__lambda.apigw-lambda-proxy-positive|_deploy_lambda_api#2|RuntimeError|raise RuntimeError(f"API setup Lambda failed: {result}")
Issue-Key: apigateway__dynamodb__lambda.apigw-lambda-response-passthrough-format|_deploy_lambda_api#2|RuntimeError|raise RuntimeError(f"API setup Lambda failed: {result}")
Issue-Key: apigateway__dynamodb__lambda.apigw-wrong-lambda-arn-integration-negative|run#11|RuntimeError|raise RuntimeError(f"Ghost API setup failed: {result}")
Issue-Key: apigateway__dynamodb__lambda.ddb-stream-lambda-batch-items-data-format|_create_table_via_lambda#3|RuntimeError|raise RuntimeError(f"Bootstrap Lambda error: {result}")
Issue-Key: apigateway__dynamodb__lambda.ddb-stream-lambda-delete-event-format|_create_table_via_lambda#3|RuntimeError|raise RuntimeError(f"Bootstrap Lambda error: {result}")
Issue-Key: apigateway__dynamodb__lambda.ddb-stream-lambda-esm-missing-stream-permission-negative|_create_table_via_lambda#3|RuntimeError|raise RuntimeError(f"Bootstrap Lambda error: {result}")
Issue-Key: apigateway__dynamodb__lambda.ddb-stream-lambda-esm-positive|_create_table_via_lambda#3|RuntimeError|raise RuntimeError(f"Bootstrap Lambda error: {result}")
Issue-Key: apigateway__dynamodb__lambda.lambda-ddb-conditional-write-positive|_create_table_via_lambda#3|RuntimeError|raise RuntimeError(f"Bootstrap Lambda error: {result}")
Issue-Key: apigateway__dynamodb__lambda.lambda-ddb-missing-putitem-permission-negative|_create_table_via_lambda#3|RuntimeError|raise RuntimeError(f"Bootstrap Lambda error: {result}")
Issue-Key: apigateway__dynamodb__lambda.lambda-ddb-put-get-positive|_create_table_via_lambda#3|RuntimeError|raise RuntimeError(f"Bootstrap Lambda error: {result}")
Issue-Key: apigateway__dynamodb__lambda.lambda-ddb-query-gsi-positive|run#8|KeyError|table_arn = body["arn"]
Issue-Key: apigateway__dynamodb__lambda.lambda-ddb-scan-no-permission-negative|_create_table_via_lambda#3|RuntimeError|raise RuntimeError(f"Bootstrap Lambda error: {result}")
Issue-Key: apigateway__dynamodb__lambda.lambda-ddb-transact-write-no-permission-negative|_create_table_via_lambda#3|RuntimeError|raise RuntimeError(f"Bootstrap Lambda error: {result}")
Issue-Key: apigateway__dynamodb__lambda.lambda-describe-apigateway-positive|run#6|RuntimeError|raise RuntimeError(f"Create API failed: {result}")
Issue-Commit: a0ca0e6d405fa8f821a988e0d013d6063af395f8

# apigateway__dynamodb__lambda: every scenario's bootstrap Lambda faults at init because the python runtime has no AWS SDK

> _Note: this issue was identified against the code base at commit `a0ca0e6d405fa8f821a988e0d013d6063af395f8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1 — the managed python runtime ships no boto3/botocore, so the bootstrap/worker function of every scenario in this group faults before its handler runs.
AWS: the python3.x managed runtimes bundle boto3 and botocore; a handler may `import boto3` and call apigateway:*, dynamodb:* and lambda:* under its execution role, and Invoke returns the handler's payload with no FunctionError.
Emulator: Invoke returns HTTP 200 with FunctionError='Unhandled' and payload {"errorMessage":"Unable to import module 'lambda_function': No module named 'boto3'","errorType":"Runtime.ImportModuleError","stackTrace":[]} — the fixed text is "Unable to import module '<module>': No module named 'boto3'", with the configured handler's module name substituted. Checks that then read a field of the expected result raise KeyError instead.
Why: the API Gateway proxy integrations, the DynamoDB Streams event-source mappings and all the IAM-boundary controls in this group are unobservable: a client gets an init failure where AWS returns the handler's result, so a REST API built this way answers 502 rather than the integration's response.
Fix: the python runtime's bundled site-packages / module search path in the lambda runtime component.
Traces: all 21 check ids in trace_ids.

## Evidence
RuntimeError: Bootstrap Lambda error / API setup Lambda failed / Ghost API setup failed: {'status_code': 200, 'error': 'Unhandled', 'payload': {'errorMessage': "Unable to import module 'lambda_function': No module named 'boto3'", 'errorType': 'Runtime.ImportModuleError', 'stackTrace': []}}; AssertionError: API Gateway bootstrap failed: same payload. Expected: StatusCode 200 with no FunctionError and the handler's JSON result.

## Steps to Reproduce
CreateFunction with Runtime=python3.12 and Handler=lambda_function.handler whose module does `import boto3`, then Invoke it. The emulator answers 200 with FunctionError=Unhandled and a Runtime.ImportModuleError naming boto3 instead of running the handler.
