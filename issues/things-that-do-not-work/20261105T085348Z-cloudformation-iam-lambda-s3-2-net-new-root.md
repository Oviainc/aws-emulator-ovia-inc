Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__lambda__s3.cfn-update-iam-role-policy-reflected-in-lambda-execution-positive cloudformation__iam__lambda__s3.cli-iam-role-create-attach-lambda-execution-positive cloudformation__iam__lambda__s3.cli-iam-role-deleted-breaks-lambda-invoke-negative cloudformation__iam__lambda__s3.cli-lambda-update-function-code-from-s3-positive cloudformation__iam__lambda__s3.iam-explicit-deny-overrides-allow-s3-negative cloudformation__iam__lambda__s3.iam-role-policy-update-reflected-lambda-s3-access-positive cloudformation__iam__lambda__s3.iam-role-trust-policy-allows-lambda-assume-positive cloudformation__iam__lambda__s3.lambda-role-permission-boundary-restricts-s3-negative cloudformation__iam__lambda__s3.lambda-role-s3-resource-arn-mismatch-negative cloudformation__iam__lambda__s3.lambda-role-wildcard-resource-s3-access-positive cloudformation__iam__lambda__s3.lambda-sdk-cfn-create-stack-missing-passrole-negative cloudformation__iam__lambda__s3.lambda-sdk-cfn-create-stack-with-iam-capability-positive cloudformation__iam__lambda__s3.lambda-sdk-cfn-delete-stack-missing-permission-negative cloudformation__iam__lambda__s3.lambda-sdk-cfn-delete-stack-positive cloudformation__iam__lambda__s3.lambda-sdk-cfn-update-stack-missing-capability-negative
Issue-Key: cloudformation__iam__lambda__s3.cfn-update-iam-role-policy-reflected-in-lambda-execution-positive|_invoke_lambda#12|AssertionError|assert "FunctionError" not in resp, resp.get("FunctionError")
Issue-Key: cloudformation__iam__lambda__s3.cli-iam-role-create-attach-lambda-execution-positive|run#22|AssertionError|assert invoke_result == {"wrote": key}, f"unexpected invoke result: {invoke_result}"
Issue-Key: cloudformation__iam__lambda__s3.cli-iam-role-deleted-breaks-lambda-invoke-negative|_invoke#19|AssertionError|assert result == {"wrote": payload["key"]}, f"unexpected invoke result: {result}"
Issue-Key: cloudformation__iam__lambda__s3.cli-lambda-update-function-code-from-s3-positive|run#23|KeyError|assert updated["CodeSha256"] == expected_sha256, (
Issue-Key: cloudformation__iam__lambda__s3.iam-explicit-deny-overrides-allow-s3-negative|run#11|KeyError|assert denied["ok"] is False, denied
Issue-Key: cloudformation__iam__lambda__s3.iam-role-policy-update-reflected-lambda-s3-access-positive|run#9|KeyError|assert baseline["ok"] is False, baseline
Issue-Key: cloudformation__iam__lambda__s3.iam-role-trust-policy-allows-lambda-assume-positive|_invoke_until#9|Timeout|time.sleep(interval_s)
Issue-Key: cloudformation__iam__lambda__s3.lambda-role-permission-boundary-restricts-s3-negative|_invoke_until#10|AssertionError|raise AssertionError(
Issue-Key: cloudformation__iam__lambda__s3.lambda-role-s3-resource-arn-mismatch-negative|run#11|KeyError|assert denied["ok"] is False, denied
Issue-Key: cloudformation__iam__lambda__s3.lambda-role-wildcard-resource-s3-access-positive|_invoke_until#8|Timeout|time.sleep(interval_s)
Issue-Key: cloudformation__iam__lambda__s3.lambda-sdk-cfn-create-stack-missing-passrole-negative|_invoke#11|AssertionError|assert "FunctionError" not in resp, f"lambda invocation errored: {resp.get('FunctionError')}"
Issue-Key: cloudformation__iam__lambda__s3.lambda-sdk-cfn-create-stack-with-iam-capability-positive|_invoke#12|AssertionError|assert "FunctionError" not in resp, f"lambda invocation errored: {resp.get('FunctionError')}"
Issue-Key: cloudformation__iam__lambda__s3.lambda-sdk-cfn-delete-stack-missing-permission-negative|_invoke#10|AssertionError|assert "FunctionError" not in resp, f"lambda invocation errored: {resp.get('FunctionError')}"
Issue-Key: cloudformation__iam__lambda__s3.lambda-sdk-cfn-delete-stack-positive|_invoke#10|AssertionError|assert "FunctionError" not in resp, f"lambda invocation errored: {resp.get('FunctionError')}"
Issue-Key: cloudformation__iam__lambda__s3.lambda-sdk-cfn-update-stack-missing-capability-negative|_invoke#16|AssertionError|assert "FunctionError" not in resp, f"lambda invocation errored: {resp.get('FunctionError')}"
Issue-Commit: 8f45cbde1fc786a42a670207326215689e25a831

# cloudformation__iam__lambda__s3: 2 net-new root causes — the Lambda python runtime has no AWS SDK, and UpdateFunctionCode's response omits CodeSha256

> _Note: this issue was identified against the code base at commit `8f45cbde1fc786a42a670207326215689e25a831`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Two root causes.

Cause 1 of 2: `lambda:Invoke` now runs the deployment package, but the python runtime it runs it in has no AWS SDK on the import path, so every handler that calls an AWS service dies at module import.
AWS: the managed python3.x Lambda runtimes bundle boto3/botocore (AWS publishes the bundled SDK version per runtime), so `import boto3` succeeds and the handler can call S3, IAM, CloudFormation and STS under the function's execution role. Invoke then answers 200 with the handler's own payload and no function error. Smithy (lambda Invoke): `StatusCode` is `@httpResponseCode`, `FunctionError` is the `X-Amz-Function-Error` header and is set only when the handler itself faulted, `Payload` is the `@httpPayload` blob; AWS models no error for a missing SDK because on AWS there is none.
Emulator: Invoke answers 200 with `X-Amz-Function-Error: Unhandled` and the payload `{"errorMessage":"Unable to import module '<handler-module>': No module named 'boto3'","errorType":"Runtime.ImportModuleError","requestId":"<per-invocation id>","stackTrace":[]}`. Fixed text: `Unable to import module '<module>': No module named 'boto3'` and `errorType` `Runtime.ImportModuleError`; the module name is taken from the function's `Handler` and `requestId` is per-invocation (not part of the requirement). Checks that poll for the handler's result instead report the same payload as the last observed response, or exhaust their budget.
Why: none of the authorization semantics these checks exist to prove is observable. An execution role's allow, its explicit deny, its permissions boundary, a resource-ARN mismatch, a wildcard resource, a deleted role, `iam:PassRole` on CreateStack, and the IAM-capability gate on CreateStack/UpdateStack/DeleteStack are all asserted from INSIDE the function; with no SDK the positive and negative halves are indistinguishable, so a missing or over-broad authorization check passes silently.
Fix: the Invoke execution path's python runtime environment — boto3/botocore must be importable inside the in-process runtime that executes the deployment package, and the credential/endpoint environment it exposes must resolve to the emulator under the function's execution role.
Related: the open issue 20261104T160000Z-cloudformation-iam-lambda-s3-six-causes-awaiting-the-lambda-provisioner-and-an-in-process-runtime.md describes an EARLIER state of this path (its Cause 3: `Invoke` answered 501 NotImplemented). Invoke now returns 200 and runs the package, so these checks no longer carry that issue's replay keys and the suite reports them as unowned; this entry is the current, net-new failure mode.
Traces: cfn-update-iam-role-policy-reflected-in-lambda-execution-positive, cli-iam-role-create-attach-lambda-execution-positive, cli-iam-role-deleted-breaks-lambda-invoke-negative, iam-explicit-deny-overrides-allow-s3-negative, iam-role-policy-update-reflected-lambda-s3-access-positive, iam-role-trust-policy-allows-lambda-assume-positive, lambda-role-permission-boundary-restricts-s3-negative, lambda-role-s3-resource-arn-mismatch-negative, lambda-role-wildcard-resource-s3-access-positive, lambda-sdk-cfn-create-stack-missing-passrole-negative, lambda-sdk-cfn-create-stack-with-iam-capability-positive, lambda-sdk-cfn-delete-stack-missing-permission-negative, lambda-sdk-cfn-delete-stack-positive, lambda-sdk-cfn-update-stack-missing-capability-negative.

Cause 2 of 2: `lambda:UpdateFunctionCode` answers without `CodeSha256`.
AWS: Smithy — `UpdateFunctionCode`'s output shape is `FunctionConfiguration`, which carries `CodeSha256` (String): the base64-encoded SHA256 digest of the deployment package. AWS always returns it, and it is the value a client compares against the digest of the archive it just uploaded to confirm the new code is the code that landed (the same digest `GetFunction`/`GetFunctionConfiguration` and `PublishVersion` report).
Emulator: the `UpdateFunctionCode` response has no `CodeSha256` member at all — reading it raises `KeyError: 'CodeSha256'`.
Why: a client has no way to verify that an `UpdateFunctionCode` from an S3 object actually replaced the package. Content-addressed deploy tooling (CDK/SAM and any CI that compares digests before publishing a version) cannot confirm the update and cannot detect a no-op or a stale package.
Fix: the `UpdateFunctionCode` handler and the function state behind it — the digest of the stored package must be computed and persisted at create/update time and serialized into every `FunctionConfiguration` response.
Traces: cli-lambda-update-function-code-from-s3-positive.

## Evidence
Cause 1 — expected: 200, no `FunctionError`, payload = the handler's result (e.g. `{"wrote": "<key>"}` or `{"ok": false, ...}`).
Actual:
- `AssertionError: unexpected invoke result: {'errorMessage': "Unable to import module 'index': No module named 'boto3'", 'errorType': 'Runtime.ImportModuleError', 'requestId': '...', 'stackTrace': []}` (cli-iam-role-create-attach-lambda-execution-positive, cli-iam-role-deleted-breaks-lambda-invoke-negative)
- `AssertionError: ... never satisfied the expected condition within 60s; last observed response was {'errorMessage': "Unable to import module 'lambda_function': No module named 'boto3'", 'errorType': 'Runtime.ImportModuleError', ...}` (lambda-role-permission-boundary-restricts-s3-negative)
- `AssertionError: Unhandled` / `AssertionError: lambda invocation errored: Unhandled` (cfn-update-iam-role-policy-reflected-in-lambda-execution-positive, lambda-sdk-cfn-create-stack-missing-passrole-negative, lambda-sdk-cfn-create-stack-with-iam-capability-positive, lambda-sdk-cfn-delete-stack-missing-permission-negative, lambda-sdk-cfn-delete-stack-positive, lambda-sdk-cfn-update-stack-missing-capability-negative)
- `KeyError: 'ok'` — the handler's result dict never arrives (iam-explicit-deny-overrides-allow-s3-negative, iam-role-policy-update-reflected-lambda-s3-access-positive, lambda-role-s3-resource-arn-mismatch-negative)
- `Check timed out after 60.0s` while `_invoke_until(...)` polls for `r.get("ok") is True` from a handler whose first line is `import boto3` (iam-role-trust-policy-allows-lambda-assume-positive, lambda-role-wildcard-resource-s3-access-positive)

Cause 2 — expected: `update_function_code(...)["CodeSha256"]` equals the base64 SHA256 of the uploaded zip.
Actual: `assert updated["CodeSha256"] == expected_sha256` → `KeyError: 'CodeSha256'` (cli-lambda-update-function-code-from-s3-positive).

## Steps to Reproduce
Cause 1: (a) create an execution role and a python3.12 function whose handler imports boto3 and calls `s3:GetObject`/`s3:PutObject`/`iam:GetRole`/`cloudformation:CreateStack`; (b) `lambda:Invoke` it — the 200 response sets `X-Amz-Function-Error: Unhandled` and the payload is `Runtime.ImportModuleError` / `No module named 'boto3'`; (c) repeat with the role's policy allowing then explicitly denying the action — both invocations give the same fault.
Cause 2: upload a zip to S3, call `lambda:UpdateFunctionCode` with `S3Bucket`/`S3Key`, and read `CodeSha256` from the response — the member is absent.
