Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__s3__stepfunctions.lambda-starts-sfn-missing-permission-negative
Issue-Key: lambda__s3__stepfunctions.lambda-starts-sfn-missing-permission-negative|_poll_until#17|Timeout|raise AssertionError(
Issue-Commit: c1082b91874a267e16e5e882b92de2a31503a88d

# lambda__s3__stepfunctions: the under-permissioned StartExecution-from-Lambda negative never converges and is killed at its 60s deadline

> _Note: this issue was identified against the code base at commit `c1082b91874a267e16e5e882b92de2a31503a88d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1: the negative pair that proves a Lambda handler without `states:StartExecution` cannot start a state machine makes no observable progress against the emulator and is killed at its deadline. Failing check: lambda__s3__stepfunctions.lambda-starts-sfn-missing-permission-negative.

AWS: Smithy (lambda, Invoke): HTTP POST /2015-03-31/functions/{FunctionName}/invocations, success code 200, with StatusCode bound to httpResponseCode, FunctionError to the response header `X-Amz-Function-Error` and Payload to the HTTP payload. A handler that calls StepFunctions StartExecution with an execution role lacking `states:StartExecution` runs, catches the AccessDeniedException the SDK raises, and returns its own report of that refusal; the granted twin starts an execution. Both halves therefore answer promptly, and the emulator must produce one of them to compare. Recorded behavior only for the timing: no response was produced, so there is no status or error code to quote.

Emulator: the check produces nothing to assert on and ends with `Error: Check timed out after 60.0s`. Hypothesis: the same cause as the already-filed checks of this group — the emulator's python Lambda invoke runtime ships no AWS SDK, so the handler dies at `import boto3` with `Runtime.ImportModuleError`, never calls StartExecution, and the check then polls for an execution (or for the refusal) that never appears until its deadline. The timeout line carries no further diagnosis, and this check's deployed handler is the same boto3-importing shape as its siblings.

Why: a client cannot observe the authorization boundary between a Lambda that may start executions and one that may not; instead of a prompt AccessDeniedException report the caller gets a hang, which in a real pipeline reads as a stuck invocation rather than a permissions problem.

Fix: the Lambda invoke runtime/sandbox backing the python3.x runtimes must expose the AWS SDK on the handler's import path so the handler runs and the StepFunctions refusal is reported (services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/invoke.rs and the python runtime bundle it loads); the StartExecution authorization path in services/service-com-amazonaws-sfn-awsstepfunctions must then refuse the ungranted session.

The open issue 20261105T081644Z-lambda-s3-stepfunctions-the-lambda-python-runtime-ships.md owns the other failing checks of this service heading but does not carry this check's replay key, so this check is otherwise unowned.

Traces: see trace_ids.

## Evidence
Expected (AWS): synchronous Invoke -> HTTP 200 with no `X-Amz-Function-Error` header and a Payload in which the handler reports the StartExecution refusal (AccessDeniedException), while the granted twin reports a started execution ARN.

Actual (emulator): `Error: Check timed out after 60.0s` for lambda__s3__stepfunctions.lambda-starts-sfn-missing-permission-negative — no response, status, error code or message was produced. Sibling checks of the same group under the same handler shape return HTTP 200 with `x-amz-function-error: Unhandled` and payload `{"errorMessage":"Unable to import module '<module>': No module named 'boto3'","errorType":"Runtime.ImportModuleError",...}`.

## Steps to Reproduce
1. Create a state machine and two Lambda functions (runtime python3.x) whose handler modules do `import boto3` and call stepfunctions.start_execution — one with `states:StartExecution` on its role, one without.
2. Invoke both synchronously and wait for the handlers' reports.
3. Observe that neither returns a usable report (the import fails) and the check hangs to its 60s deadline instead of reporting the AccessDeniedException.
4. Re-replay lambda__s3__stepfunctions.lambda-starts-sfn-missing-permission-negative to confirm.
