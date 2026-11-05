Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__ssm.concurrent-lambda-invocations-update-ssm-parameter-versioning events__lambda__ssm.eventbridge-schedule-lambda-writes-ssm-parameter events__lambda__ssm.lambda-events-putevents-wrong-bus-arn-denied events__lambda__ssm.lambda-resolves-ssm-parameter-arn events__lambda__ssm.lambda-ssm-get-nonexistent-parameter-error-propagates events__lambda__ssm.lambda-ssm-get-parameters-partial-batch events__lambda__ssm.lambda-ssm-path-prefix-condition-restriction-denied events__lambda__ssm.lambda-ssm-wrong-resource-arn-denied events__lambda__ssm.lambda-without-events-permission-denied events__lambda__ssm.lambda-without-ssm-permission-denied
Issue-Key: events__lambda__ssm.concurrent-lambda-invocations-update-ssm-parameter-versioning|_classify_concurrent_write#10|AssertionError|assert error_type == "TooManyUpdates", (
Issue-Key: events__lambda__ssm.eventbridge-schedule-lambda-writes-ssm-parameter|_payload#7|AssertionError|assert not result.get("error"), (
Issue-Key: events__lambda__ssm.lambda-events-putevents-wrong-bus-arn-denied|_payload#16|AssertionError|assert not result.get("error"), (
Issue-Key: events__lambda__ssm.lambda-resolves-ssm-parameter-arn|_payload#8|AssertionError|assert not result.get("error"), (
Issue-Key: events__lambda__ssm.lambda-ssm-get-nonexistent-parameter-error-propagates|_payload#8|AssertionError|assert not result.get("error"), (
Issue-Key: events__lambda__ssm.lambda-ssm-get-parameters-partial-batch|_payload#9|AssertionError|assert not result.get("error"), (
Issue-Key: events__lambda__ssm.lambda-ssm-path-prefix-condition-restriction-denied|_payload#12|AssertionError|assert not result.get("error"), (
Issue-Key: events__lambda__ssm.lambda-ssm-wrong-resource-arn-denied|_payload#11|AssertionError|assert not result.get("error"), (
Issue-Key: events__lambda__ssm.lambda-without-events-permission-denied|_payload#3|AssertionError|assert not result.get("error"), (
Issue-Key: events__lambda__ssm.lambda-without-ssm-permission-denied|_payload#8|AssertionError|assert not result.get("error"), (
Issue-Commit: bd1d039abc40ef8a9373a2b76b9b7f1b159abc82

# events__lambda__ssm: the Lambda python runtime has no boto3, so every handler that imports the AWS SDK faults Runtime.ImportModuleError and no Lambda-driven integration can run

> _Note: this issue was identified against the code base at commit `bd1d039abc40ef8a9373a2b76b9b7f1b159abc82`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1: a python3.x function whose handler does `import boto3` faults on import instead of running.

AWS: the managed `python3.x` runtimes ship the AWS SDK for Python in the execution environment — boto3 and botocore are importable from a handler with no layer, no vendored dependency and no packaging step, and the environment also carries the credentials and region the function's execution role was given. So `Invoke` (RequestResponse) answers 200 with the handler's own payload, and when the handler's AWS call is denied the handler observes an `AccessDeniedException` from that call (which is what these checks then assert on). Recorded behavior: every one of these checks creates a function from an inline zip whose only non-stdlib import is boto3 and expects a payload back.

Emulator: `Invoke` answers 200 with `X-Amz-Function-Error: Unhandled` and the error document `{"errorMessage": "Unable to import module 'lambda_function': No module named 'boto3'", "errorType": "Runtime.ImportModuleError", "requestId": <per-run id>, "stackTrace": []}`. The shape and wording are the ones AWS uses for a genuinely missing module — `Unable to import module '<handler module>': No module named '<module>'` with errorType `Runtime.ImportModuleError`, the module names taken from the request's handler and the failing import — so the response is well-formed; the defect is that boto3 IS absent from a runtime where AWS provides it.

Why: the whole reason a function exists in most scenarios is to call another AWS service. Every such handler faults, so a client sees a Lambda that can only run pure-stdlib code: EventBridge→Lambda→SSM pipelines, IAM denial propagation into a handler, partial-batch `GetParameters` results, concurrent `PutParameter` TooManyUpdates contention and cross-bus PutEvents denials are all unobservable, and a check that asserts a DENIAL cannot distinguish it from this import fault.

Fix: the Lambda execution environment the in-process interpreter builds — `services/service-com-amazonaws-lambda-awsgirapiservice/src/state/execution/python` and the runtime bootstrap beside it — must expose boto3/botocore (or an equivalent module presenting that API) to the handler, wired to this emulator's endpoints and to the credentials/region of the function's execution role, for every runtime whose managed AWS counterpart bundles the SDK.

Traces: the ten failing checks in trace_ids; each fails on the first invocation of a handler importing boto3. The same root cause fails kms__lambda__ssm.lambda-reads-securestring-parameter-encrypted-with-customer-key, filed separately in this sweep because it is a different service heading.

## Evidence
Representative (eventbridge-schedule-lambda-writes-ssm-parameter): `AssertionError: Lambda invocation failed with Unhandled: {'errorMessage': "Unable to import module 'lambda_function': No module named 'boto3'", 'errorType': 'Runtime.ImportModuleError', 'requestId': <id>, 'stackTrace': []}`.
Same payload at concurrent-lambda-invocations-update-ssm-parameter-versioning (`status_code: 200, error: 'Unhandled'`, where the check expected an SSM `TooManyUpdates` guard), lambda-events-putevents-wrong-bus-arn-denied, lambda-resolves-ssm-parameter-arn, lambda-ssm-get-nonexistent-parameter-error-propagates, lambda-ssm-get-parameters-partial-batch, lambda-ssm-path-prefix-condition-restriction-denied, lambda-ssm-wrong-resource-arn-denied, lambda-without-events-permission-denied and lambda-without-ssm-permission-denied.
Expected in each: HTTP 200 with no `FunctionError` and the handler's JSON payload.

## Steps to Reproduce
Replay the behavioral suite, group events__lambda__ssm. Create a python3.x function from an inline zip whose `lambda_function.py` starts with `import boto3`, give it an execution role, and call `lambda:Invoke` with InvocationType=RequestResponse: the response is 200 with `FunctionError: Unhandled` and `Runtime.ImportModuleError` / `No module named 'boto3'` instead of the handler's payload.
