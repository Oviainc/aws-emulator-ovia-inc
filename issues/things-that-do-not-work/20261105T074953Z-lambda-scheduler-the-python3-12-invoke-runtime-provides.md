Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__scheduler.lambda-invokes-scheduler-create-schedule-positive lambda__scheduler.lambda-invokes-scheduler-get-schedule-positive lambda__scheduler.lambda-invokes-scheduler-delete-schedule-positive lambda__scheduler.lambda-scheduler-update-schedule-group-positive lambda__scheduler.lambda-scheduler-create-schedule-missing-permission-negative lambda__scheduler.lambda-scheduler-create-schedule-wrong-group-negative lambda__scheduler.lambda-scheduler-list-schedules-missing-permission-negative
Issue-Key: lambda__scheduler.lambda-invokes-scheduler-create-schedule-positive|_invoke_lambda#7|AssertionError|raise AssertionError(f"lambda invocation crashed: {body}")
Issue-Key: lambda__scheduler.lambda-invokes-scheduler-get-schedule-positive|_invoke_lambda#9|AssertionError|raise AssertionError(f"lambda invocation crashed: {body}")
Issue-Key: lambda__scheduler.lambda-invokes-scheduler-delete-schedule-positive|_invoke_lambda#9|AssertionError|raise AssertionError(f"lambda invocation crashed: {body}")
Issue-Key: lambda__scheduler.lambda-scheduler-update-schedule-group-positive|_invoke_lambda#7|AssertionError|raise AssertionError(f"lambda invocation crashed: {body}")
Issue-Key: lambda__scheduler.lambda-scheduler-create-schedule-missing-permission-negative|_invoke_lambda#7|AssertionError|raise AssertionError(f"lambda invocation crashed: {body}")
Issue-Key: lambda__scheduler.lambda-scheduler-create-schedule-wrong-group-negative|_invoke_lambda#8|AssertionError|raise AssertionError(f"lambda invocation crashed: {body}")
Issue-Key: lambda__scheduler.lambda-scheduler-list-schedules-missing-permission-negative|_invoke_lambda#8|AssertionError|raise AssertionError(f"lambda invocation crashed: {body}")
Issue-Commit: 8bf126a687f579c23502fa67f92e47f7c7638042

# lambda__scheduler: the python3.12 Invoke runtime provides no AWS SDK, so every handler that imports boto3 dies with Runtime.ImportModuleError

> _Note: this issue was identified against the code base at commit `8bf126a687f579c23502fa67f92e47f7c7638042`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
One net-new root cause, grounded in all 7 failing lambda__scheduler checks.

Cause 1 of 1: the emulator's managed python3.12 Lambda runtime does not provide the AWS SDK for Python, so a handler whose first statement is `import boto3` never reaches its entrypoint.

AWS: AWS Lambda's managed Python runtimes (python3.12 among them) ship the AWS SDK for Python — boto3 and botocore — preinstalled on the runtime's import path, so a deployment package that does NOT bundle boto3 can still `import boto3` and build a client. Smithy: Invoke, POST /2015-03-31/functions/{FunctionName}/invocations, output InvocationResponse{StatusCode (httpResponseCode), FunctionError (X-Amz-Function-Error), LogResult, Payload (httpPayload), ExecutedVersion}; an unhandled init failure is NOT a modeled error, it is reported in-band as 200 + FunctionError. Recorded behavior: the check's handler (`index.handler`, Runtime="python3.12", created by CreateFunction with a ZipFile containing only index.py) does `import boto3`, builds `boto3.client("scheduler")`, calls the requested scheduler operation and returns `{"ok": true, "result": ...}`. On AWS, Invoke answers 200 with NO X-Amz-Function-Error header and that JSON document as Payload.

Emulator: Invoke answers 200 but sets FunctionError and returns the runtime's init-failure document as the Payload: `{"errorMessage": "Unable to import module 'index': No module named 'boto3'", "errorType": "Runtime.ImportModuleError", "requestId": "<per-invocation uuid>", "stackTrace": []}`. In that message the fixed text is `Unable to import module '<module>': No module named '<module-name>'`; `index` is the module half of the function's configured Handler and `boto3` is the module the handler asked for — both come from the request/function state, and `requestId` is a per-run value. The errorType `Runtime.ImportModuleError` is the Python runtime's init-error class. So the package IS executed (the in-process runtime landed — see the resolved 20261029T233239Z-lambda-synchronous-invoke-data-plane-answers-501-notimplemented.md, which this does not reopen: the call is no longer refused, it returns 200), but the runtime's environment is bare: no boto3, no botocore.

Why: calling another AWS service from a Lambda function is the single most common Lambda pattern, and on the managed Python runtimes the SDK is a runtime-provided dependency that nobody vendors into the archive. Against this emulator every such function fails at module import before a single line of handler code runs, so a client sees FunctionError / Runtime.ImportModuleError where AWS returns the handler's result. All 7 lambda__scheduler scenarios — the three positive paths (CreateSchedule, GetSchedule, DeleteSchedule through a function), the schedule-group update path, and the three negative paths that depend on the function surfacing the scheduler refusal code (AccessDeniedException for the missing scheduler:CreateSchedule / scheduler:ListSchedules grant, and the wrong-group refusal) — can never reach their assertion, so Lambda-to-scheduler interop is wholly unobservable and the IAM-denial behaviour it is meant to prove is untested.

Fix: the Lambda Invoke execution path's runtime environment, in the lambda crate's Invoke handler/execution component that builds the sandbox for a `python3.*` function — the interpreter it starts must have boto3 and botocore importable (runtime-provided, in addition to whatever the deployment package contains), the same way the managed runtime does. Hypothesis (neither the model nor the recording states it, but the checks assert on the effect): the runtime-provided SDK must also be pointed at the emulator's own endpoint and given the function's execution-role credentials through the standard runtime environment variables, or the import will succeed and every client call will then be directed off-box.

Traces (all 7 fail on the identical import error): lambda__scheduler.lambda-invokes-scheduler-create-schedule-positive, lambda__scheduler.lambda-invokes-scheduler-get-schedule-positive, lambda__scheduler.lambda-invokes-scheduler-delete-schedule-positive, lambda__scheduler.lambda-scheduler-update-schedule-group-positive, lambda__scheduler.lambda-scheduler-create-schedule-missing-permission-negative, lambda__scheduler.lambda-scheduler-create-schedule-wrong-group-negative, lambda__scheduler.lambda-scheduler-list-schedules-missing-permission-negative.

## Evidence
Expected (AWS): Invoke -> HTTP 200, no X-Amz-Function-Error header, Payload = the handler's return document, e.g. {"ok": true, "result": {... the scheduler operation's response ...}} for the positive checks, and {"ok": false, "error_code": "AccessDeniedException", ...} for the permission-denied checks.

Actual (emulator): Invoke -> HTTP 200 with X-Amz-Function-Error set and Payload:
  {"errorMessage": "Unable to import module 'index': No module named 'boto3'",
   "errorType": "Runtime.ImportModuleError",
   "requestId": "<uuid, per invocation>",
   "stackTrace": []}
The suite surfaces this as `AssertionError: lambda invocation crashed: {...}` in every one of the 7 lambda__scheduler checks, raised from each check's _invoke_lambda helper at `if resp.get("FunctionError"): raise AssertionError(...)`. Message anatomy: fixed text `Unable to import module '<handler module>': No module named '<module>'`; `index` is taken from the function's Handler (`index.handler`), `boto3` is the module the handler imported; requestId is a per-run value and is not part of the requirement.

## Steps to Reproduce
1. iam:CreateRole a role trusting lambda.amazonaws.com and attach AWSLambdaBasicExecutionRole plus an inline policy granting scheduler:CreateSchedule and iam:PassRole.
2. lambda:CreateFunction with Runtime="python3.12", Handler="index.handler", Timeout=30 and Code.ZipFile containing a single index.py whose body is `import json, boto3` followed by a handler that does `boto3.client("scheduler")` and calls the operation named in the event, returning {"ok": True, "result": ...}. The archive deliberately bundles NO boto3, as on AWS.
3. Wait for the function to report State=Active, then lambda:Invoke it (InvocationType RequestResponse) with payload {"action": "create_schedule", "params": {...}}.
4. Observe the response: AWS returns 200 with no X-Amz-Function-Error and the handler's JSON; the emulator returns 200 with X-Amz-Function-Error set and the Runtime.ImportModuleError document above.
5. Replay the behavioral suite and run the lambda__scheduler group — all 7 checks fail at this same import.
