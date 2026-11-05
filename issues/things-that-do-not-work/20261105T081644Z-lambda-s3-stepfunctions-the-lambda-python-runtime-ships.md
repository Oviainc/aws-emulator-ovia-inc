Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__s3__stepfunctions.lambda-starts-sfn-execution-positive lambda__s3__stepfunctions.lambda-writes-to-s3-via-sdk-positive lambda__s3__stepfunctions.lambda-describe-sfn-execution-positive lambda__s3__stepfunctions.lambda-describe-sfn-missing-permission-negative lambda__s3__stepfunctions.lambda-list-sfn-executions-wrong-arn-negative lambda__s3__stepfunctions.lambda-reads-s3-object-missing-permission-negative lambda__s3__stepfunctions.lambda-s3-getobject-nonexistent-key-error-propagation lambda__s3__stepfunctions.lambda-s3-putobject-missing-permission-negative lambda__s3__stepfunctions.lambda-starts-sfn-missing-permission-negative lambda__s3__stepfunctions.cfn-deploy-missing-sfn-permission-negative lambda__s3__stepfunctions.cfn-deploy-sfn-lambda-s3-stack-positive lambda__s3__stepfunctions.sfn-express-workflow-lambda-sync-positive lambda__s3__stepfunctions.sfn-pipeline-lambda-writes-s3 lambda__s3__stepfunctions.sfn-task-lambda-missing-invoke-permission-negative lambda__s3__stepfunctions.sfn-catch-lambda-error-and-write-s3
Issue-Key: lambda__s3__stepfunctions.lambda-starts-sfn-execution-positive|run#16|AssertionError|assert exec_arn, f"Missing executionArn: {payload}"
Issue-Key: lambda__s3__stepfunctions.lambda-writes-to-s3-via-sdk-positive|run#9|AssertionError|assert payload.get("etag"), f"Missing ETag: {payload}"
Issue-Key: lambda__s3__stepfunctions.lambda-describe-sfn-execution-positive|run#20|AssertionError|assert payload.get("status") in ("RUNNING", "SUCCEEDED"), (
Issue-Key: lambda__s3__stepfunctions.lambda-describe-sfn-missing-permission-negative|_poll_until#21|AssertionError|raise AssertionError(
Issue-Key: lambda__s3__stepfunctions.lambda-list-sfn-executions-wrong-arn-negative|run#19|AssertionError|assert control.get("ok") is True, (
Issue-Key: lambda__s3__stepfunctions.lambda-reads-s3-object-missing-permission-negative|run#10|AssertionError|assert payload.get("code") == "AccessDenied", f"Expected AccessDenied but got: {payload}"
Issue-Key: lambda__s3__stepfunctions.lambda-s3-getobject-nonexistent-key-error-propagation|run#12|AssertionError|assert resp["payload"].get("body") == present_body.decode(), (
Issue-Key: lambda__s3__stepfunctions.lambda-s3-putobject-missing-permission-negative|_poll_until#12|AssertionError|raise AssertionError(
Issue-Key: lambda__s3__stepfunctions.lambda-starts-sfn-missing-permission-negative|_poll_until#17|Timeout|last = probe()
Issue-Key: lambda__s3__stepfunctions.cfn-deploy-missing-sfn-permission-negative|run#13|AssertionError|assert "accessdenied" in payload_str or "not authorized" in payload_str, (
Issue-Key: lambda__s3__stepfunctions.cfn-deploy-sfn-lambda-s3-stack-positive|run#15|AssertionError|assert payload.get("status") == "ok", f"Lambda did not return ok: {payload}"
Issue-Key: lambda__s3__stepfunctions.sfn-express-workflow-lambda-sync-positive|run#24|KeyError|assert output["squared"] == 49, f"Expected squared==49, got: {output}"
Issue-Key: lambda__s3__stepfunctions.sfn-pipeline-lambda-writes-s3|run#24|KeyError|s3_key = output["s3Result"]["s3Key"]
Issue-Key: lambda__s3__stepfunctions.sfn-task-lambda-missing-invoke-permission-negative|run#25|AssertionError|assert _marker_exists(positive_token), (
Issue-Key: lambda__s3__stepfunctions.sfn-catch-lambda-error-and-write-s3|_poll_until#28|AssertionError|raise AssertionError(
Issue-Commit: ce1e7fa07219b3a065b602a90626dda6f8aa6c34

# lambda__s3__stepfunctions: the Lambda Python runtime ships no AWS SDK, and a Lambda Task whose function raises still SUCCEEDS so Catch never fires (2 causes)

> _Note: this issue was identified against the code base at commit `ce1e7fa07219b3a065b602a90626dda6f8aa6c34`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Two net-new root causes. The fixed issue 20261029T233239Z-lambda-synchronous-invoke-data-plane-answers-501-notimplemented.md is distinct: Invoke now answers 200 and does execute the package, and neither cause below is a refusal of Invoke.

Cause 1 of 2: the Lambda managed Python runtime provides no AWS SDK, so every handler that uses boto3 dies at module import.
AWS: the managed Python runtimes bundle the AWS SDK for Python — `import boto3` succeeds in a function deployed as a bare .zip with no layer, and the SDK resolves the region and the function role's temporary credentials from the runtime environment (AWS_REGION, AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY, AWS_SESSION_TOKEN). Invoke then answers HTTP 200 with no X-Amz-Function-Error header and the handler's returned document as the payload (Smithy: lambda#Invoke output FunctionError -> httpHeader X-Amz-Function-Error, Payload -> httpPayload). In a negative-permission arm the handler instead surfaces the called service's own refusal — s3 `AccessDenied`, states `AccessDeniedException` — because the SDK call actually reaches the service.
Emulator: Invoke answers HTTP 200 with `X-Amz-Function-Error: Unhandled` and the payload `{"errorMessage": "Unable to import module '<module>': No module named 'boto3'", "errorType": "Runtime.ImportModuleError", "requestId": "<per-run id>", "stackTrace": []}`. The fixed text is "Unable to import module '<module>': No module named 'boto3'" with errorType `Runtime.ImportModuleError`; the module name comes from the function's Handler setting (both `index` and `lambda_function` appear) and the requestId is per-invocation. The handler body never runs, so no S3 object is written, no execution is started or described, and no result document (executionArn, ETag, s3Key, squared, ...) is produced.
Why: every Lambda that talks to another AWS service is non-functional, in direct invokes and as a Step Functions Task alike. Positive paths produce no side effect and negative (IAM-denial) paths cannot be told apart from the import failure, so both arms of every permission check collapse to the same answer.
Fix: the Lambda Invoke data-plane runtime/sandbox for the Python runtimes must make the AWS SDK importable on the handler's sys.path and export the region plus the function role's credentials into the handler environment.
Traces: lambda__s3__stepfunctions.lambda-starts-sfn-execution-positive, .lambda-writes-to-s3-via-sdk-positive, .lambda-describe-sfn-execution-positive, .lambda-describe-sfn-missing-permission-negative, .lambda-list-sfn-executions-wrong-arn-negative, .lambda-reads-s3-object-missing-permission-negative, .lambda-s3-getobject-nonexistent-key-error-propagation, .lambda-s3-putobject-missing-permission-negative, .lambda-starts-sfn-missing-permission-negative, .cfn-deploy-missing-sfn-permission-negative, .cfn-deploy-sfn-lambda-s3-stack-positive, .sfn-express-workflow-lambda-sync-positive, .sfn-pipeline-lambda-writes-s3, .sfn-task-lambda-missing-invoke-permission-negative.

Cause 2 of 2: an unhandled Lambda handler error does not fail the Task state, so the state's Catch never fires.
AWS: Lambda Invoke answers 200 with X-Amz-Function-Error: Unhandled when the handler raises, and Step Functions turns that into a task failure: the history records LambdaFunctionFailed then the Catch transition (Smithy sfn#HistoryEventType enumerates LambdaFunctionFailed and TaskFailed), the task error is the errorType from the function's error document (the thrown type, e.g. `Exception`, or `Lambda.Unknown` when the runtime reports none) and the cause is that JSON error document. A Catch on `States.TaskFailed` routes the execution to its handler state, which then runs.
Emulator: the Task is reported successful with the whole Invoke envelope (`"FunctionError":"Unhandled"`, `"Payload":{"errorMessage":...,"errorType":...}`) as its result; the Catch branch is never entered and the error-handling state's side effect (the S3 object it writes) never appears.
Why: error handling in workflows is unreachable. A client that models failure with Catch gets a success verdict and no recovery action, and the diagnostic object/notification its handler would produce is missing.
Fix: the Step Functions ASL interpreter's Task-state Lambda integration must map a non-empty FunctionError on the Invoke response to a task failure before Retry/Catch evaluation.
Traces: lambda__s3__stepfunctions.sfn-catch-lambda-error-and-write-s3.

## Evidence
Cause 1 — expected: the handler's SDK call runs and the invoke returns its document (executionArn / ETag / body / squared / s3Key), or in the negative arms the called service's own AccessDenied. Actual, repeatedly: `AssertionError: Missing executionArn: {'errorMessage': "Unable to import module 'lambda_function': No module named 'boto3'", 'errorType': 'Runtime.ImportModuleError', ...}`; `AssertionError: Missing ETag: {...Runtime.ImportModuleError...}`; `AssertionError: Expected AccessDenied but got: {...Runtime.ImportModuleError...}`; `AssertionError: Expected AccessDeniedException but got: {...Runtime.ImportModuleError...}`; `KeyError: 'squared'`; `KeyError: 's3Key'`; `AssertionError: Positive control execution SUCCEEDED but the Lambda side effect (S3 marker) is missing`; and `Error: Check timed out after 60.0s` where the check polls for a side effect that never arrives.

Cause 2 — expected: the failing Lambda Task is caught and the handler state writes its S3 object. Actual: `AssertionError: condition not met within 30s. Last result: []` — no object was ever written because the task did not fail.

## Steps to Reproduce
Cause 1: CreateFunction with a Python runtime and a one-file .zip whose handler does `import boto3` and calls s3.put_object / s3.get_object / stepfunctions.start_execution / describe_execution; Invoke it and observe 200 with X-Amz-Function-Error: Unhandled and payload errorType Runtime.ImportModuleError, "No module named 'boto3'", with no S3 object or execution created.
Cause 2: Create a state machine whose Task invokes a handler that raises, with Catch: [{ErrorEquals: ["States.TaskFailed"], Next: "WriteError"}]; StartExecution and observe the task reported successful (result = the Invoke envelope with FunctionError "Unhandled"), WriteError never entered and its S3 object absent.
