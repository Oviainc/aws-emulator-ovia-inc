Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sesv2.cli-lambda-sesv2-describe-email-identity lambda__sesv2.cli-lambda-sesv2-describe-function-config
Issue-Key: lambda__sesv2.cli-lambda-sesv2-describe-email-identity|_run_shell#11|AssertionError|raise AssertionError(
Issue-Key: lambda__sesv2.cli-lambda-sesv2-describe-function-config|_run_shell#8|AssertionError|raise AssertionError(
Issue-Commit: 3ae1c5f9ad044bf0dd64ac25eb19f35c30d1d713

# lambda__sesv2: lambda Invoke refuses every real invocation with an unmodeled NotImplemented error, so the SES v2 interop checks cannot run

> _Note: this issue was identified against the code base at commit `3ae1c5f9ad044bf0dd64ac25eb19f35c30d1d713`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
One root cause. Neither failing check is marked OWNED or ALREADY FILED in the replay report; the open issue 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md describes the same defect but does not carry these two checks' replay keys, so they are filed here and that issue is named for reference.

AWS: Smithy lambda Invoke (POST /2015-03-31/functions/{FunctionName}/invocations) runs the function's deployment package and answers HTTP 200 with the handler's response in the Payload blob, the X-Amz-Executed-Version header, and X-Amz-Function-Error only when the handler itself failed. Its modeled errors are the ones enumerated on the operation (InvalidRequestContentException, ResourceNotFoundException, RequestTooLargeException, ServiceException, TooManyRequestsException, the EC2/ENI/EFS/KMS 502-class errors, ...); `NotImplemented` is not among them and no modeled error reports that the service cannot run a package it accepted at CreateFunction.
Emulator: every real invocation of a python3.12 package is refused with an error code `NotImplemented` and the message `Invoke resolved <function ARN> at version $LATEST, but running its python3.12 deployment package is not implemented in this emulator: there is no in-process runtime for it, and no response payload is returned that the function did not produce. InvocationType=DryRun validates an invocation without running the function.` — emulator-authored prose, under an error code the Invoke model does not declare. The `aws lambda invoke` CLI exits 254.
Why: a client cannot execute any Lambda. Because the refusal is an unmodeled code, SDK error handling cannot classify it either: it is neither a retryable throttle nor a resource-not-found, so callers see an opaque failure. Here it blocks the two SES v2 interop paths entirely — the function that would call sesv2 GetEmailIdentity and the function that would report its own configuration never run, so nothing about the SES v2 surface they exercise is observable.
Fix: the Lambda Invoke handler and the function-runtime execution path (it must run the accepted deployment package and return its payload); until it does, CreateFunction should not accept a package the service cannot run.
Traces: lambda__sesv2.cli-lambda-sesv2-describe-email-identity, lambda__sesv2.cli-lambda-sesv2-describe-function-config.

## Evidence
Expected: `aws lambda invoke --function-name <fn> --payload <json> out.json` exits 0 and writes the handler's response to out.json (HTTP 200, no X-Amz-Function-Error).
Actual (replay): `AssertionError: Command failed (rc=254): cmd: ['aws', '--output', 'json', '--no-cli-pager', 'lambda', 'invoke', '--function-name', '<fn>', ...]` with stderr `aws: [ERROR]: An error occurred (NotImplemented) when calling the Invoke operation: Invoke resolved arn:aws:lambda:<region>:<account>:function:<fn> at version $LATEST, but running its python3.12 deployment package is not implemented in this emulator: there is no in-process runtime for it, and no response payload is returned that the function did not produce. InvocationType=DryRun validates an invocation without running the function.` The function names and account id are per-run values; the unmodeled `NotImplemented` code and the emulator-authored message are the divergence.

## Steps to Reproduce
1. CreateFunction with a python3.12 runtime and a zip whose handler calls sesv2 (GetEmailIdentity) or returns its own configuration; wait until the function is Active.
2. Run `aws lambda invoke --function-name <fn> --cli-binary-format raw-in-base64-out --payload '{"identity": "<domain>"}' out.json`.
3. The CLI exits 254 and the service answers an error whose code is NotImplemented instead of running the handler and returning 200 with its payload.
