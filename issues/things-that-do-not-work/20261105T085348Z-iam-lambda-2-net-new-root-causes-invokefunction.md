Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__lambda.lambda-cannot-put-events-without-permission iam__lambda.xsvc-iam-2-lambda-invoke-without-permission-denied iam__lambda.xsvc-iam-2-lambda-remove-permission-revokes-invoke-denied
Issue-Key: iam__lambda.lambda-cannot-put-events-without-permission|run#15|AssertionError|assert control.get("ok") is True, (
Issue-Key: iam__lambda.xsvc-iam-2-lambda-invoke-without-permission-denied|_settle#13|Timeout|time.sleep(interval_s)
Issue-Key: iam__lambda.xsvc-iam-2-lambda-remove-permission-revokes-invoke-denied|_settle#16|Timeout|time.sleep(interval_s)
Issue-Commit: 8f45cbde1fc786a42a670207326215689e25a831

# iam__lambda: 2 net-new root causes — InvokeFunction is never refused for a principal no policy grants, and the Lambda python runtime has no AWS SDK

> _Note: this issue was identified against the code base at commit `8f45cbde1fc786a42a670207326215689e25a831`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Two root causes.

Cause 1 of 2: `lambda:Invoke` is not authorized against the caller's identity policy or the function's resource policy — a principal that no policy grants can invoke the function, and `RemovePermission` does not take the grant away.
AWS: `Invoke` by an IAM principal with no identity-based policy granting `lambda:InvokeFunction` and no statement in the function's resource policy is refused with HTTP 403 and the error code `AccessDeniedException`, message `User: <caller-arn> is not authorized to perform: lambda:InvokeFunction on resource: <function-arn> because no identity-based policy allows the lambda:InvokeFunction action` — fixed text with the caller ARN, the action and the function ARN taken from the request and the caller's identity. The function does NOT run. Symmetrically, once `AddPermission` has granted the principal and the grant is then withdrawn with `RemovePermission`, the same refusal returns. Smithy: `Invoke`'s modeled error set is about the function and its runtime (`ResourceNotFoundException`, `ResourceConflictException`, `InvalidRequestContentException`, …); the authorization refusal is the service-common IAM refusal applied at the request boundary, so it is recorded behavior plus AWS's documented IAM semantics rather than an operation-level modeled error.
Emulator: no refusal is produced. Both checks poll for the invoke outcome (they require the SAME verdict on three consecutive samples at a 2 s interval, so a correctly refused call converges in roughly six seconds) and neither ever converges on `AccessDeniedException` within the suite's 60 s budget — it keeps answering the successful invocation instead. The handler here is `def handler(event, context): return {"echo": event}`, so nothing else can be failing: the invocation is being executed for an unauthorized principal, and `RemovePermission` leaves it executable.
Why: Lambda's invoke permission is the entire access boundary of a function. A client that revokes a permission believes the function is no longer reachable while every principal can still run it; and a client testing a deny sees it "pass" because the call is never refused. This is a silent authorization bypass on the data plane.
Fix: the lambda Invoke request path's authorization — the caller's identity policy and the function's resource policy must both be evaluated before the function is run, and a principal with neither must be refused 403 `AccessDeniedException` with that message.
Related: 20261029T192356Z-iam-lambda-lambda-resource-policy-authorization-of-invoke.md is open for the grant (AddPermission) side; it does not carry these two checks' replay keys.
Traces: xsvc-iam-2-lambda-invoke-without-permission-denied, xsvc-iam-2-lambda-remove-permission-revokes-invoke-denied.

Cause 2 of 2: the python runtime the invoked package runs in has no AWS SDK on the import path.
AWS: the managed python3.x Lambda runtimes bundle boto3/botocore, so a handler that does `import boto3` and calls `events:PutEvents` under the execution role runs and returns its own result (or the service's own refusal). Smithy (lambda Invoke): `FunctionError` is the `X-Amz-Function-Error` header, set only when the handler itself faulted.
Emulator: Invoke answers 200 with `FunctionError` set and the payload `{"errorMessage":"Unable to import module '<handler-module>': No module named 'boto3'","errorType":"Runtime.ImportModuleError","requestId":"<per-invocation id>","stackTrace":[]}`. Fixed text: `Unable to import module '<module>': No module named 'boto3'` and `errorType` `Runtime.ImportModuleError`; the module name comes from the function's `Handler`, the request id is per-invocation.
Why: the positive control of an in-function authorization test cannot pass, so the negative half proves nothing — a missing `events:PutEvents` enforcement is indistinguishable from a runtime that cannot call EventBridge at all.
Fix: the Invoke execution path's python runtime environment — boto3/botocore must be importable inside the in-process runtime that executes the deployment package.
Traces: lambda-cannot-put-events-without-permission.

## Evidence
Cause 1 — expected (AWS): `invoke` as a principal with neither an identity grant nor a resource-policy statement → 403 `AccessDeniedException`, `User: arn:aws:iam::<account>:user/<name> is not authorized to perform: lambda:InvokeFunction on resource: arn:aws:lambda:<region>:<account>:function:<name> because no identity-based policy allows the lambda:InvokeFunction action`, and the handler does not run.
Actual (emulator): `Error: Check timed out after 60.0s` for both checks. Each uses `_settle(probe, want, streak=3, timeout_s=120, interval_s=2)` over `_outcome(invoke)`, which returns `'OK'` on success and the exact AWS error code otherwise; a real refusal settles in ~6 s, so 60 s without convergence means the verdict stayed the successful invocation. The handler is `def handler(event, context): return {"echo": event}` — no SDK import, so cause 2 is not in play here.

Cause 2 — expected: the granted positive control returns `{"ok": true, ...}`.
Actual: `AssertionError: positive control failed with events:PutEvents granted: {'errorMessage': "Unable to import module 'lambda_function': No module named 'boto3'", 'errorType': 'Runtime.ImportModuleError', 'requestId': '...', 'stackTrace': []}`.

## Steps to Reproduce
Cause 1: (a) create a function with an execution role carrying no permissions; (b) create an IAM user with NO policies and an access key; (c) sign `lambda:Invoke` on the function as that user — it succeeds and the handler runs, instead of 403 `AccessDeniedException`; (d) `AddPermission` for the user, confirm invoke works, `RemovePermission`, and invoke again — it still succeeds.
Cause 2: create a function whose handler does `import boto3` and calls `events:PutEvents`, grant the execution role `events:PutEvents`, and invoke it — the response sets `X-Amz-Function-Error` and the payload is `Runtime.ImportModuleError` / `No module named 'boto3'`.
