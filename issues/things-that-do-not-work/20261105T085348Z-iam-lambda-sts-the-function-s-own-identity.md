Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__lambda__sts.function-identity-explicit-deny-overrides-its-allow iam__lambda__sts.function-runs-as-an-assumed-role-session-named-for-the-function
Issue-Key: iam__lambda__sts.function-identity-explicit-deny-overrides-its-allow|run#10|AssertionError|assert err is None and out == {"arn": role["arn"]}, f"the allowed GetRole gave {err!r} {out!r}"
Issue-Key: iam__lambda__sts.function-runs-as-an-assumed-role-session-named-for-the-function|run#5|AssertionError|assert err is None, f"the function failed: {err!r} {out!r}"
Issue-Commit: 8f45cbde1fc786a42a670207326215689e25a831

# iam__lambda__sts: the function's own identity is unobservable — the Lambda python runtime has no AWS SDK, so no handler can call STS or IAM

> _Note: this issue was identified against the code base at commit `8f45cbde1fc786a42a670207326215689e25a831`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
One root cause.

Cause 1 of 1: `lambda:Invoke` executes the deployment package in a python runtime with no AWS SDK on the import path, so a handler that calls `sts:GetCallerIdentity` or `iam:GetRole` to establish what identity it runs as dies at module import.

AWS: the managed python3.x Lambda runtimes bundle boto3/botocore (AWS publishes the bundled version per runtime), so `import boto3` succeeds and the handler runs as an assumed-role session of its execution role — `sts:GetCallerIdentity` from inside the function reports `arn:aws:sts::<account>:assumed-role/<execution-role-name>/<function-name>`, i.e. a session named for the function, and `iam:GetRole` succeeds or is refused exactly as the execution role's identity policy dictates. Invoke answers 200 with the handler's own payload and no function error. Smithy (lambda Invoke): `StatusCode` is `@httpResponseCode`, `FunctionError` is bound to header `X-Amz-Function-Error` and is set only when the handler itself faulted, `Payload` is the `@httpPayload` blob; AWS models no error for a missing SDK because there is none.

Emulator: Invoke answers 200 with `X-Amz-Function-Error: Unhandled` and the payload `{"errorMessage":"Unable to import module '<handler-module>': No module named 'boto3'","errorType":"Runtime.ImportModuleError","requestId":"<per-invocation id>","stackTrace":[]}`. Fixed text: `Unable to import module '<module>': No module named 'boto3'`, `errorType` `Runtime.ImportModuleError`; the module name comes from the function's `Handler` and the request id is per-invocation, so neither belongs to the requirement.

Why: a client cannot determine, or rely on, the identity a function runs as. Neither the session-ARN shape (`assumed-role/<role>/<function>`) nor the precedence of an explicit deny over an allow in the execution role's own policy is observable, because both the allowed call and the denied call return the same runtime fault — so an execution role that grants too much is indistinguishable from one that is correctly scoped.

Fix: the Invoke execution path's python runtime environment — boto3/botocore must be importable inside the in-process runtime, and the credentials it picks up must be a real assumed-role session of the function's execution role named for the function, so calls made from inside the handler are authorized as that session.

Traces: the two failing checks in trace_ids.

## Evidence
Expected (AWS): `invoke` returns 200 with no `FunctionError`; the payload is the handler's result, e.g. `{"arn": "<role arn>"}` from the allowed `iam:GetRole`, and a caller identity of the form `arn:aws:sts::<account>:assumed-role/<role-name>/<function-name>`.

Actual (emulator), identically in both checks:
- iam__lambda__sts.function-identity-explicit-deny-overrides-its-allow — `AssertionError: the allowed GetRole gave 'Unhandled' {'errorMessage': "Unable to import module 'index': No module named 'boto3'", 'errorType': 'Runtime.ImportModuleError', 'requestId': '<per-run id>', 'stackTrace': []}`
- iam__lambda__sts.function-runs-as-an-assumed-role-session-named-for-the-function — `AssertionError: the function failed: 'Unhandled' {'errorMessage': "Unable to import module 'index': No module named 'boto3'", 'errorType': 'Runtime.ImportModuleError', 'requestId': '<per-run id>', 'stackTrace': []}`

## Steps to Reproduce
1. Create an execution role with an inline policy allowing `iam:GetRole` on itself (and, for the second variant, an explicit `Deny` statement for the same action).
2. Create a python3.12 function whose handler does `import boto3` and calls `sts:GetCallerIdentity` and `iam:GetRole`.
3. `lambda:Invoke` the function.
4. Observe the 200 response sets `X-Amz-Function-Error: Unhandled` with a `Runtime.ImportModuleError` / `No module named 'boto3'` payload, so neither the session ARN nor the allow/deny verdict is ever returned.
