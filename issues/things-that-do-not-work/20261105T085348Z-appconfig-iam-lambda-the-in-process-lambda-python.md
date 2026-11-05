Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appconfig__iam__lambda.execution-role-without-appconfig-actions-denied-in-handler appconfig__iam__lambda.handler-with-logical-id-identifiers-falls-back-to-default appconfig__iam__lambda.in-place-identifier-fix-reaches-appconfig-only-once-all-three-are-real
Issue-Key: appconfig__iam__lambda.execution-role-without-appconfig-actions-denied-in-handler|_invoke_until#25|AssertionError|assert "FunctionError" not in resp, (
Issue-Key: appconfig__iam__lambda.handler-with-logical-id-identifiers-falls-back-to-default|_invoke_until#20|AssertionError|assert "FunctionError" not in resp, (
Issue-Key: appconfig__iam__lambda.in-place-identifier-fix-reaches-appconfig-only-once-all-three-are-real|_invoke#16|AssertionError|assert resp["StatusCode"] == 200 and "FunctionError" not in resp, (
Issue-Commit: 8f45cbde1fc786a42a670207326215689e25a831

# appconfig__iam__lambda: the in-process Lambda python runtime ships no AWS SDK, so every AppConfig handler faults Runtime.ImportModuleError instead of returning its result

> _Note: this issue was identified against the code base at commit `8f45cbde1fc786a42a670207326215689e25a831`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
One root cause.

Cause 1 of 1: lambda Invoke runs the handler, but the python runtime it runs it in has no AWS SDK on the import path, so any handler that calls AppConfig (or any other AWS service) dies at module import.

AWS: the managed python3.x Lambda runtimes bundle boto3/botocore on the function's import path (AWS publishes the bundled SDK version per runtime), so `import boto3` in a handler succeeds. Invoke then answers HTTP 200 with the handler's own payload and NO function error. Smithy (lambda Invoke): output member `StatusCode` is `@httpResponseCode` (200 for `RequestResponse`), `FunctionError` is bound to header `X-Amz-Function-Error` and is present only when the handler itself faulted, `Payload` is the `@httpPayload` blob. No modeled error covers a missing SDK, because on AWS there is none.

Emulator: every invocation of a handler that imports boto3 answers 200 with `X-Amz-Function-Error: Unhandled` and the payload `{"errorMessage":"Unable to import module '<handler-module>': No module named 'boto3'","errorType":"Runtime.ImportModuleError","requestId":"<per-invocation id>","stackTrace":[]}`. In that payload the fixed text is `Unable to import module '<module>': No module named 'boto3'` and `errorType` `Runtime.ImportModuleError`; the module name is taken from the function's `Handler` property and `requestId` is the per-invocation id (never part of the requirement). Two of the three checks here see it as `FunctionError 'Unhandled'` from handlers that are written to catch their own AppConfig failure and return a fallback — the import fault happens before any handler code runs, so the catch cannot fire.

Why: a client cannot run any realistic Lambda function against this emulator. Every scenario that depends on what a function DOES — here reading an AppConfig deployed configuration, falling back to a default when the identifiers are logical ids, and the identity-denied path — is unobservable, and the failure is misreported as the function's own unhandled exception rather than a missing runtime capability, so an operator debugging it is sent to their handler code.

Fix: the Invoke execution path's python runtime environment — the SDK (boto3/botocore) must be importable inside the in-process runtime that executes the deployment package, and the credentials/endpoint environment the SDK picks up must point back at the emulator.

Traces: the three failing checks listed in trace_ids. Note: the same runtime gap is already filed under other service tokens (for example 20261105T074953Z-lambda-scheduler-the-python3-12-invoke-runtime-provides.md and 20261105T081139Z-dynamodb-lambda-s3-secretsmanager-the-in-process-lambda.md); neither carries these checks' replay keys, so this is the appconfig__iam__lambda instance.

## Evidence
Expected (AWS): `invoke` returns 200, no `FunctionError`, payload = the handler's JSON result (e.g. `{"source": "appconfig", ...}`).

Actual (emulator):
- appconfig__iam__lambda.execution-role-without-appconfig-actions-denied-in-handler — `AssertionError: the handler catches its own AppConfig failure, so no invocation may report FunctionError; got 'Unhandled'`
- appconfig__iam__lambda.handler-with-logical-id-identifiers-falls-back-to-default — `AssertionError: the handler catches its own AppConfig failure, so no invocation may report FunctionError; got 'Unhandled'`
- appconfig__iam__lambda.in-place-identifier-fix-reaches-appconfig-only-once-all-three-are-real — `AssertionError: the invoke answered 200 with FunctionError 'Unhandled'`

The payload behind that `Unhandled` is, verbatim from the sibling clusters that print it: `{"errorMessage":"Unable to import module 'lambda_function': No module named 'boto3'","errorType":"Runtime.ImportModuleError","requestId":"<id>","stackTrace":[]}`.

## Steps to Reproduce
1. Create an IAM role Lambda may assume and a python3.12 function whose handler begins `import boto3`.
2. Create an AppConfig application/environment/configuration-profile and deploy a configuration version.
3. Call `lambda:Invoke` (`InvocationType=RequestResponse`) on the function.
4. Observe the 200 response carries `X-Amz-Function-Error: Unhandled` and a `Runtime.ImportModuleError` payload naming `No module named 'boto3'`, instead of the handler's result.
