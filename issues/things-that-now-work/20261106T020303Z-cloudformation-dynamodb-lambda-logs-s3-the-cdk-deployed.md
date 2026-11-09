Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__dynamodb__lambda__logs__s3.cdk-default-deploy-function-writes-granted-table-and-carries-cdk-metadata
Issue-Key: cloudformation__dynamodb__lambda__logs__s3.cdk-default-deploy-function-writes-granted-table-and-carries-cdk-metadata|_invoke_until_granted#29|AssertionError|assert "FunctionError" not in response, f"{function} raised: {payload!r}"
Issue-Commit: cc6137e930728edc173d6e704b5a7c1716e7fffd

# cloudformation__dynamodb__lambda__logs__s3: the CDK-deployed writer function faults Runtime.ImportModuleError — the Lambda python runtime provides no boto3

> _Note: this issue was identified against the code base at commit `cc6137e930728edc173d6e704b5a7c1716e7fffd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
One net-new behavioral root cause of service `cloudformation__dynamodb__lambda__logs__s3`, grounded in one failing behavioral check.

Cause 1 of 1: the python runtime that executes a deployed function provides no AWS SDK, so a CDK default-props function whose handler does `import boto3` dies at module load and its granted table write never happens.
AWS: recorded behavior only — the managed `python3.x` Lambda runtimes bundle boto3/botocore, so `import boto3` at module scope succeeds and the handler runs under the execution role CDK granted, writing the item. Smithy (lambda Invoke): `FunctionError` is the `X-Amz-Function-Error` header and is set only when the handler itself faulted. AWS models no error for a missing SDK in the runtime; while an IAM grant is still propagating the handler would instead return DynamoDB's own `AccessDeniedException`, which is what the check retries on.
Emulator: Invoke answers 200 with `FunctionError` set and payload `{"errorMessage":"Unable to import module '<handler module>': No module named 'boto3'","errorType":"Runtime.ImportModuleError","requestId":"<per-invocation id>","stackTrace":[]}`. The fixed text is `Unable to import module '<module>': No module named 'boto3'` with `errorType` `Runtime.ImportModuleError`; the module name comes from the function's Handler and the requestId is per-invocation.
Why: a `cdk deploy` of the default function-plus-table-grant shape cannot be verified end to end — the import fault is returned whether or not the grant exists, so a missing grant, a wrong table name and a correct deployment are indistinguishable, and the function writes nothing.
Fix: the python runtime environment of the Lambda invoke path — boto3/botocore must be importable inside the in-process runtime that executes the deployment package. The same class is already recorded under other suite headings (e.g. the open issue 20261105T112044Z-dynamodb-lambda-s3-the-lambda-python-runtime-provides.md); this check is listed under `cloudformation__dynamodb__lambda__logs__s3` and is not covered by it.
Traces: cloudformation__dynamodb__lambda__logs__s3.cdk-default-deploy-function-writes-granted-table-and-carries-cdk-metadata.

## Evidence
cloudformation__dynamodb__lambda__logs__s3.cdk-default-deploy-function-writes-granted-table-and-carries-cdk-metadata [mismatch]:
  AssertionError: sim2real-cdkmeta-writer-<id> raised: {'errorMessage': "Unable to import module 'index': No module named 'boto3'", 'errorType': 'Runtime.ImportModuleError', 'requestId': '<per-run id>', 'stackTrace': []}
  Expected (AWS): Invoke returns 200 with no FunctionError and the handler's result (or, while the grant propagates, DynamoDB's AccessDeniedException), and the item it wrote is readable with GetItem. The function name and request id are per-run and are not part of the requirement.

Smithy consulted: lambda Invoke — FunctionError is the X-Amz-Function-Error header, set only on a handler fault. No modeled error covers a missing SDK in the runtime.

## Steps to Reproduce
1. `cdk deploy` a default-props CDK app with a python Lambda function (handler module doing `import boto3`), a DynamoDB table and `table.grant_write_data(fn)`.
2. Invoke the function with `{"pk": "<key>", "marker": "<marker>"}`.
3. Observe the 200 response sets X-Amz-Function-Error and the payload is Runtime.ImportModuleError / `No module named 'boto3'` instead of the handler's result.
4. Re-replay: cloudformation__dynamodb__lambda__logs__s3.cdk-default-deploy-function-writes-granted-table-and-carries-cdk-metadata.
