Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot__lambda.lambda-iot-data-publish-missing-permission-negative iot__lambda.lambda-iot-get-thing-shadow-missing-permission-negative iot__lambda.lambda-iot-get-thing-shadow-positive iot__lambda.lambda-iot-update-thing-shadow-positive iot__lambda.lambda-iot-update-thing-shadow-version-conflict-negative iot__lambda.lambda-sdk-iot-publish-missing-permission-negative iot__lambda.lambda-sdk-iot-publish-positive iot__lambda.lambda-sdk-iot-update-shadow-missing-permission-negative iot__lambda.lambda-sdk-iot-update-thing-shadow-positive
Issue-Key: iot__lambda.lambda-iot-data-publish-missing-permission-negative|_invoke_json#10|AssertionError|raise AssertionError(f"lambda invocation errored: {body}")
Issue-Key: iot__lambda.lambda-iot-get-thing-shadow-missing-permission-negative|_invoke_json#13|AssertionError|raise AssertionError(f"lambda invocation errored: {body}")
Issue-Key: iot__lambda.lambda-iot-get-thing-shadow-positive|_invoke_json#13|AssertionError|raise AssertionError(f"lambda invocation errored: {body}")
Issue-Key: iot__lambda.lambda-iot-update-thing-shadow-positive|_invoke_json#12|AssertionError|raise AssertionError(f"lambda invocation errored: {body}")
Issue-Key: iot__lambda.lambda-iot-update-thing-shadow-version-conflict-negative|_invoke_json#14|AssertionError|raise AssertionError(f"lambda invocation errored: {body}")
Issue-Key: iot__lambda.lambda-sdk-iot-publish-missing-permission-negative|_invoke#13|AssertionError|raise AssertionError(f"lambda invocation failed ({resp['FunctionError']}): {body!r}")
Issue-Key: iot__lambda.lambda-sdk-iot-publish-positive|_invoke_until_delivered#11|Timeout|time.sleep(4.0)
Issue-Key: iot__lambda.lambda-sdk-iot-update-shadow-missing-permission-negative|_invoke_until_stable#11|AssertionError|raise AssertionError(f"lambda invocation failed ({function_error}): {last_body!r}")
Issue-Key: iot__lambda.lambda-sdk-iot-update-thing-shadow-positive|_invoke_until_delivered#10|Timeout|time.sleep(4.0)
Issue-Commit: dc856e880220c47012eb41a5b29cc1ddaf120576

# iot__lambda: the python Lambda runtime provides no AWS SDK (boto3), so every handler that calls IoT dies with Runtime.ImportModuleError and all 9 Lambda->IoT interop checks are unobservable

> _Note: this issue was identified against the code base at commit `dc856e880220c47012eb41a5b29cc1ddaf120576`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
One net-new root cause, grounded in all 9 net-new failing iot__lambda checks (the other 21 iot__lambda failures in this run are marked OWNED/ALREADY FILED and are not part of this issue).

Cause 1 of 1: the emulator's managed python Lambda runtime does not provide the AWS SDK for Python on the interpreter's import path, so a handler whose module-level code says `import boto3` never reaches its entrypoint, and no Lambda-side call to iot / iot-data ever happens.

AWS: AWS Lambda's managed Python runtimes ship the AWS SDK for Python (boto3 and botocore) preinstalled in the execution environment, so a deployment package that bundles NO SDK can still `import boto3` and build a client. Smithy (lambda): Invoke, POST /2015-03-31/functions/{FunctionName}/invocations, http code 200, output InvocationResponse{StatusCode (@httpResponseCode), FunctionError (header X-Amz-Function-Error), LogResult (X-Amz-Log-Result), Payload (@httpPayload), ExecutedVersion (X-Amz-Executed-Version)}; an init/import failure is NOT a modeled error of Invoke (its modeled errors are InvalidRequestContentException, InvalidRuntimeException, ResourceNotFoundException, ... ), it is reported in band as HTTP 200 + FunctionError + an error document Payload. For these checks the handler is `index.handler` / `lambda_function.lambda_handler` / `recorder.handler` on a python3.x runtime, created by CreateFunction with a ZipFile containing only that one .py file; it does `boto3.client('iot').describe_endpoint(endpointType='iot:Data-ATS')` and then an iot-data Publish / UpdateThingShadow / GetThingShadow against that endpoint, returning its own JSON document (e.g. {"ok": true} or {"ok": false, "error_code": "..."}). On AWS, Invoke answers 200 with NO X-Amz-Function-Error and that document as Payload.

Emulator: Invoke answers 200 but sets FunctionError (value `Unhandled`) and returns the python runtime's init-failure document as the Payload: {"errorMessage": "Unable to import module '<handler module>': No module named 'boto3'", "errorType": "Runtime.ImportModuleError", "requestId": "<per-invocation uuid>", "stackTrace": []}. Message anatomy: the fixed text is `Unable to import module '<module>': No module named '<module-name>'`; `<module>` is the module half of the function's configured Handler (observed as `index`, `lambda_function` and `recorder` across these checks, i.e. taken from function state) and `<module-name>` is the module the handler asked for (`boto3`); `requestId` is a per-run value and no part of the requirement. errorType `Runtime.ImportModuleError` is the Python runtime's init-error class. So the package IS executed (the call is not refused; the invoke path lands and returns 200), but the runtime environment is bare: no boto3, no botocore.

Two of the nine checks (iot__lambda.lambda-sdk-iot-publish-positive, iot__lambda.lambda-sdk-iot-update-thing-shadow-positive) surface the same cause as `Check timed out after 60.0s` rather than an assertion, because their `_invoke_until_delivered` helper re-invokes the function in a retry loop and every attempt returns the same import failure until the harness budget is exhausted; their handler sources (`import boto3` + boto3.client('iot')/client('iot-data')) are identical in kind to the seven that report the document above.

Why: calling another AWS service from inside a function is the primary Lambda pattern, and on the managed Python runtimes the SDK is runtime-provided — real deployment packages never vendor it. Against this emulator every such function fails at module import before one line of handler code runs, so a client sees FunctionError / Runtime.ImportModuleError where AWS returns the handler's result. Concretely this makes ALL Lambda->IoT interop unobservable: the positive shadow/publish paths (iot-data Publish, UpdateThingShadow, GetThingShadow issued from inside a function), the shadow version-conflict path, and the permission-boundary negatives that depend on the function surfacing IoT's refusal code for a missing iot:Publish / iot:GetThingShadow / iot:UpdateThingShadow grant. None of them reaches its assertion, so neither the data-plane behaviour nor the IAM-denial behaviour they exist to prove is tested.

This same bare-runtime cause is already filed for other service pairings (e.g. 20261105T074953Z-lambda-scheduler-the-python3-12-invoke-runtime-provides.md, 20261105T094343Z-events-lambda-sqs-the-lambda-python-runtime-provides.md, 20261105T081139Z-lambda-secretsmanager-sqs-the-in-process-lambda-python.md). None of them carries these iot__lambda checks, and the replay report did not mark these checks as owned, so they are filed here; a fix to the shared runtime should close all of them.

Fix: the Lambda Invoke execution path's runtime environment — the lambda crate's Invoke handler / execution component that builds the sandbox for a `python3.*` function must start an interpreter on which boto3 and botocore are importable as runtime-provided packages, in addition to whatever the deployment archive contains. Hypothesis (not stated by the model or the recording, but implied by what these checks then assert): the runtime-provided SDK must also be pointed at the emulator's own endpoints and given the function's execution-role credentials via the standard runtime environment variables, otherwise the import will succeed and the in-function iot / iot-data calls will be directed off-box instead of at the emulator.

Traces (all 9 share the identical import failure): iot__lambda.lambda-iot-data-publish-missing-permission-negative, iot__lambda.lambda-iot-get-thing-shadow-missing-permission-negative, iot__lambda.lambda-iot-get-thing-shadow-positive, iot__lambda.lambda-iot-update-thing-shadow-positive, iot__lambda.lambda-iot-update-thing-shadow-version-conflict-negative, iot__lambda.lambda-sdk-iot-publish-missing-permission-negative, iot__lambda.lambda-sdk-iot-publish-positive (timeout form), iot__lambda.lambda-sdk-iot-update-shadow-missing-permission-negative, iot__lambda.lambda-sdk-iot-update-thing-shadow-positive (timeout form).

## Evidence
Expected (AWS): lambda Invoke -> HTTP 200, no X-Amz-Function-Error header, Payload = the handler's own JSON document, e.g. {"ok": true} for the publish/update-shadow positives, the shadow document for the get-shadow positive, and {"ok": false, "error_code": "AccessDeniedException"}-style documents for the missing-permission negatives and {"error_code": "ConflictException"} for the stale-version negative.

Actual (emulator): lambda Invoke -> HTTP 200 with X-Amz-Function-Error = Unhandled and Payload:
  {"errorMessage": "Unable to import module 'lambda_function': No module named 'boto3'",
   "errorType": "Runtime.ImportModuleError",
   "requestId": "<uuid, per invocation>",
   "stackTrace": []}
(the module name follows the function's Handler: `lambda_function` for the lambda-iot-* checks, `index` for the lambda-sdk-iot-* checks, `recorder` for the recorder function).

As reported by the suite:
- iot__lambda.lambda-iot-data-publish-missing-permission-negative: AssertionError: lambda invocation errored: {'errorMessage': "Unable to import module 'lambda_function': No module named 'boto3'", 'errorType': 'Runtime.ImportModuleError', ...}
- iot__lambda.lambda-iot-get-thing-shadow-missing-permission-negative: same document, raised from _invoke_json.
- iot__lambda.lambda-iot-get-thing-shadow-positive: same document, raised from _invoke_json.
- iot__lambda.lambda-iot-update-thing-shadow-positive: same document, raised from _invoke_json.
- iot__lambda.lambda-iot-update-thing-shadow-version-conflict-negative: same document, raised from _invoke_json.
- iot__lambda.lambda-sdk-iot-publish-missing-permission-negative: AssertionError: lambda invocation failed (Unhandled): {'errorMessage': "Unable to import module 'index': No module named 'boto3'", 'errorType': 'Runtime.ImportModuleError', ...}
- iot__lambda.lambda-sdk-iot-update-shadow-missing-permission-negative: same document, raised from _invoke_until_stable.
- iot__lambda.lambda-sdk-iot-publish-positive and iot__lambda.lambda-sdk-iot-update-thing-shadow-positive: Check timed out after 60.0s inside _invoke_until_delivered, which re-invokes the same boto3-importing handler (_HANDLER_SRC = "import boto3 ... boto3.client('iot').describe_endpoint(endpointType='iot:Data-ATS') ... boto3.client('iot-data', endpoint_url=...)") until the budget runs out.

Message requirement: the error document's fixed text is `Unable to import module '<handler module>': No module named '<module-name>'` with errorType `Runtime.ImportModuleError`; AWS emits no such document at all for these packages because boto3 is importable. requestId and the generated function/thing/topic names in any one run are per-run values, not part of the requirement.

## Steps to Reproduce
1. iot:CreateThing a thing (for the shadow checks) and/or iot:CreateTopicRule a rule over a test topic (for the publish checks).
2. iam:CreateRole a role trusting lambda.amazonaws.com, attach AWSLambdaBasicExecutionRole, and attach an inline policy granting iot:DescribeEndpoint on * plus the operation under test (iot:Publish on the topic ARN, or iot:UpdateThingShadow / iot:GetThingShadow on the thing ARN). For the negative variants omit that second grant.
3. lambda:CreateFunction with a python3.x Runtime, Handler="index.handler" (or "lambda_function.lambda_handler") and Code.ZipFile containing exactly one .py file whose body begins `import boto3` and whose handler does:
     iot = boto3.client('iot', region_name=event['region'])
     ep = iot.describe_endpoint(endpointType='iot:Data-ATS')['endpointAddress']
     data = boto3.client('iot-data', region_name=event['region'], endpoint_url='https://' + ep)
     data.publish(...)  # or data.update_thing_shadow(...) / data.get_thing_shadow(...)
     return {'ok': True}
   The archive deliberately bundles NO boto3, exactly as on AWS.
4. Wait for the function to report State=Active, then lambda:Invoke it (InvocationType=RequestResponse) with the event above.
5. Observe the response: AWS returns 200 with no X-Amz-Function-Error and the handler's JSON; the emulator returns 200 with X-Amz-Function-Error=Unhandled and the Runtime.ImportModuleError document above, so the in-function IoT call never happens and the shadow/topic shows no effect.
6. Replay the behavioral suite and run the iot__lambda group — the 9 checks listed in trace_ids all fail on this one import failure (two of them as a 60s timeout inside their invoke-retry loop).
