Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__dynamodb__lambda.cfn-sam-transform-function-table-end-to-end
Issue-Key: cloudformation__dynamodb__lambda.cfn-sam-transform-function-table-end-to-end|_invoke_json#20|AssertionError|assert not response.get("FunctionError"), f"{function_name} failed: {body}"
Issue-Commit: cc6137e930728edc173d6e704b5a7c1716e7fffd

# cloudformation__dynamodb__lambda: the SAM-transformed writer function cannot import boto3, so the Lambda python runtime ships no AWS SDK

> _Note: this issue was identified against the code base at commit `cc6137e930728edc173d6e704b5a7c1716e7fffd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
One net-new behavioral root cause of service `cloudformation__dynamodb__lambda`, grounded in one failing behavioral check.

Cause 1 of 1: the python runtime that executes a deployed function provides no AWS SDK, so a handler that does `import boto3` dies at module load and the function can never touch another AWS service.
AWS: recorded behavior only — the managed `python3.x` Lambda runtimes bundle boto3/botocore, so `import boto3` at module scope succeeds and the handler runs under the role the SAM transform synthesized, writing the item that DynamoDB GetItem then returns. Smithy (lambda Invoke): `FunctionError` is the `X-Amz-Function-Error` header and is set only when the handler itself faulted; `Payload` is the `@httpPayload` blob. AWS models no error for a missing SDK in the runtime.
Emulator: Invoke answers 200 with `FunctionError` set and payload `{"errorMessage":"Unable to import module '<handler module>': No module named 'boto3'","errorType":"Runtime.ImportModuleError","requestId":"<per-invocation id>","stackTrace":[]}`. The fixed text is `Unable to import module '<module>': No module named 'boto3'` with `errorType` `Runtime.ImportModuleError`; the module name comes from the function's Handler and the requestId is per-invocation, so neither is part of the requirement.
Why: the end-to-end guarantee of the SAM transform — that AWS::Serverless::Function + SimpleTable produces a function whose generated role really can write the table — is unobservable. Every such handler returns the same runtime fault, so a missing or wrong generated role is indistinguishable from a correct one, and the common Lambda idiom (call another AWS service) cannot run at all.
Fix: the python runtime environment of the Lambda invoke path — boto3/botocore must be importable inside the in-process runtime that executes the deployment package. The same class is already recorded under other suite headings, e.g. the open issues 20261105T094343Z-cloudformation-dynamodb-events-lambda-the-lambda-python-runtime.md and 20261105T112044Z-dynamodb-lambda-s3-the-lambda-python-runtime-provides.md; this check is listed under `cloudformation__dynamodb__lambda` and is not covered by them.
Traces: cloudformation__dynamodb__lambda.cfn-sam-transform-function-table-end-to-end.

## Evidence
cloudformation__dynamodb__lambda.cfn-sam-transform-function-table-end-to-end [mismatch]:
  AssertionError: sim2real-xsvc-sam-writer-<id> failed: {"errorMessage":"Unable to import module 'index': No module named 'boto3'","errorType":"Runtime.ImportModuleError","requestId":"<per-run id>","stackTrace":[]}
  Expected (AWS): Invoke returns 200 with no FunctionError and the handler's own JSON result, after which DynamoDB GetItem returns the item the handler wrote. The function name and request id are per-run and are not part of the requirement.

Smithy consulted: lambda Invoke — FunctionError is the X-Amz-Function-Error header, set only on a handler fault; Payload is the @httpPayload blob. No modeled error covers a missing SDK in the runtime.

## Steps to Reproduce
1. Deploy a CAPABILITY_AUTO_EXPAND SAM stack with an AWS::Serverless::Function (python runtime, handler module doing `import boto3`) and an AWS::Serverless::SimpleTable keyed on pk.
2. Invoke the function with a payload naming the item to write.
3. Observe the 200 response sets X-Amz-Function-Error and the payload is Runtime.ImportModuleError / `No module named 'boto3'` instead of the handler's result, so the table write never happens.
4. Re-replay: cloudformation__dynamodb__lambda.cfn-sam-transform-function-table-end-to-end.
