Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__dynamodb__iam__lambda__s3.sam-policies-shorthand-expansion-deny-probed
Issue-Key: cloudformation__dynamodb__iam__lambda__s3.sam-policies-shorthand-expansion-deny-probed|_probe#47|AssertionError|assert not response.get("FunctionError"), f"probe {op} crashed: {body[:400]}"
Issue-Commit: 8f45cbde1fc786a42a670207326215689e25a831

# cloudformation__dynamodb__iam__lambda__s3: the SAM policies-shorthand probe function cannot call any AWS service — the Lambda python runtime has no boto3

> _Note: this issue was identified against the code base at commit `8f45cbde1fc786a42a670207326215689e25a831`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
One root cause.

Cause 1 of 1: the probe function a SAM `Policies:` shorthand check uses to exercise each expanded permission cannot import the AWS SDK, so no probe can be run and the expansion cannot be verified either way.

AWS: the managed python3.x Lambda runtimes bundle boto3/botocore, so a probe handler that does `import boto3` and then calls `s3.get_object` / `dynamodb.get_item` under the function's execution role runs and returns either the result or the service's own refusal. Smithy (lambda Invoke): `FunctionError` is the `X-Amz-Function-Error` header and is set only when the handler itself faulted; `Payload` is the `@httpPayload` blob. AWS has no error for a missing SDK in the runtime.

Emulator: the invocation answers 200 with `FunctionError` set and the payload `{"errorMessage":"Unable to import module '<handler-module>': No module named 'boto3'","errorType":"Runtime.ImportModuleError","requestId":"<per-invocation id>","stackTrace":[]}`. Fixed text: `Unable to import module '<module>': No module named 'boto3'`, `errorType` `Runtime.ImportModuleError`. The module name comes from the function's `Handler`; `requestId` is per-invocation and is not part of the requirement.

Why: a client that uses SAM `Policies:` shorthand gets no signal about which actions the shorthand actually expanded to. Both the allowed and the denied halves of the probe are indistinguishable — every probe reports the same runtime fault — so an over-broad or missing expansion passes unnoticed.

Fix: the Invoke execution path's python runtime environment — boto3/botocore must be importable inside the in-process runtime that executes the deployment package.

Traces: cloudformation__dynamodb__iam__lambda__s3.sam-policies-shorthand-expansion-deny-probed.

## Evidence
Expected (AWS): the probe invocation returns 200 with no `FunctionError`, and its payload reports the per-action outcome (allowed, or the service's `AccessDenied`).

Actual (emulator): `AssertionError: probe s3.get_object crashed: {"errorMessage":"Unable to import module 'index': No module named 'boto3'","errorType":"Runtime.ImportModuleError","requestId":"95e6513d-05f6-462a-bccd-e90a796d6f93","stackTrace":[]}` (the request id is per-run and not part of the requirement).

## Steps to Reproduce
1. Deploy a SAM (AUTO_EXPAND) stack whose function declares `Policies:` shorthand entries, with a handler module that does `import boto3`.
2. Invoke the function with a payload naming an operation to probe (e.g. `s3.get_object`).
3. Observe the 200 response sets `X-Amz-Function-Error` and the payload is `Runtime.ImportModuleError` / `No module named 'boto3'` instead of the probe's verdict.
