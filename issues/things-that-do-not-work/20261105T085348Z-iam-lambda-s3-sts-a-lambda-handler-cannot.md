Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__lambda__s3__sts.assume-role-denied-when-trust-policy-omits-lambda-role iam__lambda__s3__sts.lambda-assumed-role-credential-reads-restricted-bucket
Issue-Key: iam__lambda__s3__sts.assume-role-denied-when-trust-policy-omits-lambda-role|_invoke_payload#12|AssertionError|assert "error" not in result, f"handler faulted instead of returning a result: {result!r}"
Issue-Key: iam__lambda__s3__sts.lambda-assumed-role-credential-reads-restricted-bucket|_invoke_payload#10|AssertionError|assert "error" not in result, f"handler faulted instead of returning a result: {result!r}"
Issue-Commit: 8f45cbde1fc786a42a670207326215689e25a831

# iam__lambda__s3__sts: a Lambda handler cannot call STS or S3 — the in-process python runtime has no AWS SDK, so the assume-role scenarios never run

> _Note: this issue was identified against the code base at commit `8f45cbde1fc786a42a670207326215689e25a831`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
One root cause.

Cause 1 of 1: `lambda:Invoke` executes the deployment package in a python runtime with no AWS SDK on the import path, so a handler that calls `sts:AssumeRole` and then uses the vended credentials against S3 dies at module import.

AWS: the managed python3.x Lambda runtimes bundle boto3/botocore (AWS publishes the bundled version per runtime), so `import boto3` succeeds and the handler can call `sts:AssumeRole` with the function's own execution-role identity and then read S3 with the returned session credentials. Invoke answers 200 with the handler's own payload — including, on the negative path, the handler's capture of STS's own `AccessDenied` refusal — and no function error. Smithy (lambda Invoke): `StatusCode` is `@httpResponseCode`, `FunctionError` is bound to header `X-Amz-Function-Error` and is set only when the handler itself faulted, `Payload` is the `@httpPayload` blob; no modeled error covers a missing SDK because AWS has none.

Emulator: Invoke answers 200 with `X-Amz-Function-Error: Unhandled` and the payload `{"errorMessage":"Unable to import module '<handler-module>': No module named 'boto3'","errorType":"Runtime.ImportModuleError","requestId":"<per-invocation id>","stackTrace":[]}`. Fixed text: `Unable to import module '<module>': No module named 'boto3'` and `errorType` `Runtime.ImportModuleError`; the module name is taken from the function's `Handler` and the request id is per-invocation, so neither is part of the requirement.

Why: the role-chaining semantics these checks exist to prove are asserted from inside the function. Whether a trust policy that omits the Lambda execution role refuses `AssumeRole`, and whether credentials vended by an assumed role can read a bucket the function's own role cannot, are both invisible — the allowed and the refused paths produce the identical runtime fault, so a missing trust-policy check or an over-broad session passes silently.

Fix: the Invoke execution path's python runtime environment — boto3/botocore must be importable inside the in-process runtime that executes the deployment package, and the credential/endpoint environment it exposes must resolve to the emulator under the function's execution-role identity (so `sts:AssumeRole` from inside the function is attributed to that role).

Traces: the two failing checks in trace_ids.

## Evidence
Expected (AWS): `invoke` returns 200 with no `FunctionError` and the handler's result — either the assumed-role read's outcome or the handler's captured `AccessDenied` from `sts:AssumeRole`.

Actual (emulator), identically in both checks:
`AssertionError: handler faulted instead of returning a result: {'status_code': 200, 'error': 'Unhandled', 'payload': {'errorMessage': "Unable to import module 'lambda_function': No module named 'boto3'", 'errorType': 'Runtime.ImportModuleError', 'requestId': '<per-run id>', 'stackTrace': []}}`
- iam__lambda__s3__sts.assume-role-denied-when-trust-policy-omits-lambda-role
- iam__lambda__s3__sts.lambda-assumed-role-credential-reads-restricted-bucket

## Steps to Reproduce
1. Create a Lambda execution role and a second role whose trust policy either does or does not name it.
2. Create a python3.12 function whose handler does `import boto3`, calls `sts:AssumeRole` on the second role, and uses the returned credentials for `s3:GetObject` on a restricted bucket.
3. `lambda:Invoke` the function.
4. Observe the 200 response sets `X-Amz-Function-Error: Unhandled` and the payload is `Runtime.ImportModuleError` / `No module named 'boto3'`, for both the allowed and the denied trust-policy variants.
