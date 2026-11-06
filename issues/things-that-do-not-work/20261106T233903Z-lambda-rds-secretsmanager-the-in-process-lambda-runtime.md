Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__rds__secretsmanager.lambda-rds-data-api-execute-statement-positive lambda__rds__secretsmanager.lambda-rds-data-api-executestatement-positive lambda__rds__secretsmanager.lambda-rds-data-api-missing-permission-negative lambda__rds__secretsmanager.lambda-rds-wrong-db-cluster-arn-negative
Issue-Key: lambda__rds__secretsmanager.lambda-rds-data-api-execute-statement-positive|_invoke#18|AssertionError|raise AssertionError(
Issue-Key: lambda__rds__secretsmanager.lambda-rds-data-api-executestatement-positive|_invoke_lambda#4|AssertionError|raise AssertionError(f"lambda invocation errored: {body}")
Issue-Key: lambda__rds__secretsmanager.lambda-rds-data-api-missing-permission-negative|_invoke_lambda#5|AssertionError|raise AssertionError(f"lambda invocation errored: {body}")
Issue-Key: lambda__rds__secretsmanager.lambda-rds-wrong-db-cluster-arn-negative|_invoke_lambda#7|AssertionError|raise AssertionError(f"lambda invocation errored: {body}")
Issue-Commit: 45af2140f358870cdf193287cbc1a2501641d010

# lambda__rds__secretsmanager: the in-process Lambda runtime's bundled AWS SDK knows only sqs/events/eventbridge/dynamodb, so boto3.client("rds-data") in a handler raises UnknownServiceError

> _Note: this issue was identified against the code base at commit `45af2140f358870cdf193287cbc1a2501641d010`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
One net-new cause, in the runtime environment the Invoke path gives a deployment package.

Cause 1 of 1: a python3.x handler that builds an `rds-data` client cannot construct it, so the function never reaches the Data API call under test.
AWS: recorded behavior only for the runtime's bundled dependency set (Smithy, lambda awsgirapiservice: Invoke POST /2015-03-31/functions/{FunctionName}/invocations, StatusCode via httpResponseCode, FunctionError via the X-Amz-Function-Error header, Payload via httpPayload). AWS's managed Python runtimes ship a complete boto3/botocore whose service catalogue is the full set of AWS services, so `boto3.client("rds-data")` constructs and `execute_statement(...)` is signed and sent; Invoke answers 200 with NO X-Amz-Function-Error header and the handler's document as Payload. On the negative paths the handler gets back a botocore ClientError whose Error.Code is the service's own refusal (rds-data AccessDeniedException @httpError 403, or the resource refusal for a cluster ARN the caller may not use) and returns it as data.
Emulator: Invoke answers 200 with `x-amz-function-error: Unhandled` and the payload `{"errorMessage":"Unknown service: 'rds-data'. Valid service names are: sqs, events, eventbridge, dynamodb","errorType":"UnknownServiceError","requestId":"<per-invocation id>","stackTrace":[...]}`. Fixed text: `Unknown service: '` … `'. Valid service names are: ` followed by the runtime shim's catalogue; the variable part is the service name the handler asked for (here `rds-data`). The stack trace shows the failure comes from the emulator's own in-runtime client shim (`client.py` `_raise_refusal` -> `UnknownServiceError`), not from the handler. requestId is per-invocation and is not a requirement.
Why: no function that talks to a service outside that four-name catalogue can run. The two positive checks never obtain a Data API result from inside a function, and the two negative checks cannot tell the AccessDeniedException / wrong-cluster refusal they exist to assert from the emulator's client-construction failure — the handler's own error channel is occupied by a runtime fault. A client that develops a Lambda against this emulator sees any non-SQS/EventBridge/DynamoDB integration fail at import/construct time, which AWS never does.
Fix: the Invoke execution path's in-process runtime in services/service-com-amazonaws-lambda-awsgirapiservice — the bundled SDK shim must expose every service this emulator serves (at minimum, construct a client for any service whose endpoint the emulator answers, wired to the emulator endpoint with the execution role's credentials), instead of a hard-coded four-service catalogue.
Traces: lambda-rds-data-api-execute-statement-positive, lambda-rds-data-api-executestatement-positive, lambda-rds-data-api-missing-permission-negative, lambda-rds-wrong-db-cluster-arn-negative.

Related open issues, neither of which covers this cause: 20261105T081139Z-lambda-rds-secretsmanager-2-net-new-root-causes-e2a51e.md (the runtime shipped no boto3 at all, and no _socket) and 20261104T044754Z-lambda-rds-secretsmanager-7-net-new-root-causes.md / 20261029T195009Z-lambda-rds-secretsmanager-5-net-new-root-causes.md (CFN/RDS causes). A shim that now answers for four services is a different defect from one that answered for none.

## Evidence
All four checks fail inside the invoked handler, at the client construction, with the same document:

AssertionError: lambda invocation failed (Unhandled): {'errorMessage': "Unknown service: 'rds-data'. Valid service names are: sqs, events, eventbridge, dynamodb", 'errorType': 'UnknownServiceError', 'requestId': '<id>', 'stackTrace': ['  File "index.py", line 19, in _run\n    client.execute_statement(...)', '  File "client.py", line 141, in _call\n    self._raise_refusal(refused, answered, operation_name)', '  File "client.py", line 168, in _raise_refusal\n    raise UnknownServiceError(service_name=self._service_name, known_service_names=detail)']}

Expected: Invoke -> 200, no X-Amz-Function-Error, Payload = the handler's reply ({"records": [...]} on the positive paths; {"error": {"code": ..., "message": ...}} carrying the rds-data refusal on the negative paths).
Actual: Invoke -> 200 with x-amz-function-error: Unhandled and the UnknownServiceError document above. The handler code is the same in all four checks: boto3.client("rds-data", region_name=event["region"]) followed by execute_statement(resourceArn=..., secretArn=..., database=..., sql=...).

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite group lambda__rds__secretsmanager.
2. By hand: create a python3.12 function whose index.py does `import boto3; boto3.client("rds-data", region_name=<region>).execute_statement(resourceArn=<cluster arn>, secretArn=<secret arn>, database=<db>, sql="SELECT 1")`.
3. lambda Invoke it.
4. Observe 200 + `x-amz-function-error: Unhandled` + errorType UnknownServiceError naming only sqs, events, eventbridge, dynamodb, instead of the handler's reply.
