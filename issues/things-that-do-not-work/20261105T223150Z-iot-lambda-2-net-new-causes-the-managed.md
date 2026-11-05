Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot__lambda.cfn-deploy-lambda-execution-role-trusts-lambda-service-positive iot__lambda.lambda-execution-role-trusts-lambda-service-positive iot__lambda.lambda-iot-create-thing-missing-permission-negative iot__lambda.lambda-iot-create-thing-positive iot__lambda.lambda-iot-describe-endpoint-missing-permission-negative iot__lambda.lambda-iot-describe-endpoint-positive iot__lambda.lambda-iot-list-things-positive iot__lambda.lambda-iot-update-thing-attributes-positive iot__lambda.lambda-sdk-iot-create-thing-missing-permission-negative iot__lambda.lambda-sdk-iot-create-thing-positive iot__lambda.lambda-sdk-iot-describe-endpoint-positive iot__lambda.lambda-sdk-iot-list-things-positive iot__lambda.cfn-deploy-iot-thing-custom-resource-lambda-positive
Issue-Key: iot__lambda.cfn-deploy-lambda-execution-role-trusts-lambda-service-positive|run#16|AssertionError|assert "FunctionError" not in invoke_resp, (
Issue-Key: iot__lambda.lambda-execution-role-trusts-lambda-service-positive|run#8|AssertionError|assert resp.get("FunctionError") is None, f"invocation errored: {payload}"
Issue-Key: iot__lambda.lambda-iot-create-thing-missing-permission-negative|_invoke_json#5|AssertionError|raise AssertionError(f"lambda invocation errored: {body}")
Issue-Key: iot__lambda.lambda-iot-create-thing-positive|_invoke_json#6|AssertionError|raise AssertionError(f"lambda invocation errored: {body}")
Issue-Key: iot__lambda.lambda-iot-describe-endpoint-missing-permission-negative|_invoke_json#4|AssertionError|raise AssertionError(f"lambda invocation errored: {body}")
Issue-Key: iot__lambda.lambda-iot-describe-endpoint-positive|_invoke_json#3|AssertionError|raise AssertionError(f"lambda invocation errored: {body}")
Issue-Key: iot__lambda.lambda-iot-list-things-positive|_invoke_json#7|AssertionError|raise AssertionError(f"lambda invocation errored: {body}")
Issue-Key: iot__lambda.lambda-iot-update-thing-attributes-positive|_invoke_json#7|AssertionError|raise AssertionError(f"lambda invocation errored: {body}")
Issue-Key: iot__lambda.lambda-sdk-iot-create-thing-missing-permission-negative|_invoke#6|AssertionError|raise AssertionError(f"lambda invocation failed ({resp['FunctionError']}): {body!r}")
Issue-Key: iot__lambda.lambda-sdk-iot-create-thing-positive|_invoke#4|AssertionError|raise AssertionError(f"lambda invocation failed ({resp['FunctionError']}): {body!r}")
Issue-Key: iot__lambda.lambda-sdk-iot-describe-endpoint-positive|_invoke#3|AssertionError|raise AssertionError(f"lambda invocation failed ({resp['FunctionError']}): {body!r}")
Issue-Key: iot__lambda.lambda-sdk-iot-list-things-positive|_invoke#7|AssertionError|raise AssertionError(f"lambda invocation failed ({resp['FunctionError']}): {body!r}")
Issue-Key: iot__lambda.cfn-deploy-iot-thing-custom-resource-lambda-positive|_deploy_cfn#3|AssertionError|raise AssertionError(f"Stack {name} ended in {stack['StackStatus']}")
Issue-Commit: 2730cd672dd608c80462ed6cac2962eb48b48789

# iot__lambda: 2 net-new causes — the managed python Lambda runtime ships no boto3 so every IoT handler dies Runtime.ImportModuleError, and a stack whose IoT Thing is created by a Lambda-backed custom resource rolls back

> _Note: this issue was identified against the code base at commit `2730cd672dd608c80462ed6cac2962eb48b48789`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Two distinct root causes, both grounded in iot__lambda checks. Neither cause is an IoT-service gap: the first is in the Lambda runtime image and the second in the CloudFormation custom-resource path; the IoT calls under test are never reached.

Cause 1 of 2: the emulator's managed python Lambda runtime provides no AWS SDK, so any handler that does `import boto3` fails at module import and the invocation is reported as a handler fault instead of running.
  AWS: Smithy (service-com-amazonaws-lambda-awsgirapiservice): Invoke returns 200 with X-Amz-Function-Error set only when the function itself faulted. AWS's documented behavior: the managed python runtimes ship the AWS SDK for Python (boto3/botocore) in the runtime's site-packages, so `import boto3` succeeds with no deployment package dependency, a client built inside the handler reaches the service endpoints with the function's execution-role credentials, and the invocation's result is the handler's own return value. A handler that is granted the action gets the service's success response; a handler whose execution role lacks the action gets that service's AccessDeniedException raised inside the handler — which is exactly the positive/negative contrast these checks grade.
  Emulator: every one of these invocations returns a function error whose payload is {'errorMessage': "Unable to import module '<handler-module>': No module named 'boto3'", 'errorType': 'Runtime.ImportModuleError', 'requestId': '<per-invocation id>', 'stackTrace': []} — Runtime.ImportModuleError for the fixed reason "No module named 'boto3'", with the module name taken from the configured Handler. The invocation is reported as FunctionError 'Unhandled' rather than running.
  Why: a client relying on AWS's behavior cannot run any SDK-using handler. Every positive leg (CreateThing, DescribeEndpoint, ListThings, UpdateThingAttributes succeeding from inside a function) is lost, and every negative leg is destroyed in the more dangerous direction: the check cannot tell "the role was denied the IoT action" from "the handler never started", so a missing-permission scenario is unverifiable and a genuinely over-permissive authorization decision would go unnoticed. It also masks every downstream IoT behavior the same scenarios would have graded.
  Fix: the lambda service's managed python runtime image/site-packages — boto3 and botocore must be importable in the execution environment without being bundled in the deployment package, and the in-process runtime must expose the function's execution-role credentials to the SDK's default credential resolution.
  Traces: iot__lambda.cfn-deploy-lambda-execution-role-trusts-lambda-service-positive; iot__lambda.lambda-execution-role-trusts-lambda-service-positive; iot__lambda.lambda-iot-create-thing-missing-permission-negative; iot__lambda.lambda-iot-create-thing-positive; iot__lambda.lambda-iot-describe-endpoint-missing-permission-negative; iot__lambda.lambda-iot-describe-endpoint-positive; iot__lambda.lambda-iot-list-things-positive; iot__lambda.lambda-iot-update-thing-attributes-positive; iot__lambda.lambda-sdk-iot-create-thing-missing-permission-negative; iot__lambda.lambda-sdk-iot-create-thing-positive; iot__lambda.lambda-sdk-iot-describe-endpoint-positive; iot__lambda.lambda-sdk-iot-list-things-positive. (The open issue 20261105T114938Z-iot-lambda-the-python-lambda-runtime-provides-no.md describes the same runtime defect for 9 other checks in this service but carries none of these twelve replay keys.)

Cause 2 of 2: a CloudFormation stack whose IoT Thing is created by a Lambda-backed custom resource does not converge — it ends in ROLLBACK_COMPLETE instead of CREATE_COMPLETE.
  AWS: recorded behavior only (CloudFormation custom-resource provisioning is not a modelled operation of either service). AWS's documented behavior: CreateStack provisions the Lambda function and its role, then invokes the custom resource's ServiceToken with a Create request containing a ResponseURL; the function performs its IoT CreateThing and PUTs a SUCCESS response to that URL; CloudFormation records the physical resource id and the stack reaches CREATE_COMPLETE, with the created thing readable through DescribeThing afterwards.
  Emulator: the stack reaches ROLLBACK_COMPLETE — 'AssertionError: Stack <stack-name> ended in ROLLBACK_COMPLETE' — so the custom resource never signals success and the thing it should have created is never provisioned. (The stack name in the capture is a generated per-run value.)
  Why: a client relying on AWS's behavior cannot provision anything through a Lambda-backed custom resource in this service; a template that deploys cleanly on AWS is rolled back here, and the failure is reported as a stack rollback rather than naming the resource that did not signal, so the real defect is invisible to the operator.
  Fix: the CloudFormation custom-resource provisioner — the Create lifecycle invocation of the ServiceToken, the ResponseURL callback that records SUCCESS/FAILED, and the stack-event detail that must name the failing custom resource and its reason.
  Traces: iot__lambda.cfn-deploy-iot-thing-custom-resource-lambda-positive. (The open issue 20261029T211043Z-iot-lambda-iot-a-cloudformation-custom-resource-that-creates-a.md describes a custom resource that reports SUCCESS while the thing does not exist — the opposite symptom — and does not carry this check's replay key.)

## Evidence
Cause 1 — Expected (AWS): Invoke -> HTTP 200, no X-Amz-Function-Error, payload is the handler's return value (e.g. the CreateThing/DescribeEndpoint/ListThings result), or, for the missing-permission legs, the handler's captured IoT AccessDeniedException.
Actual (emulator), identical across all twelve checks:
  AssertionError: lambda invocation errored: {'errorMessage': "Unable to import module 'lambda_function': No module named 'boto3'", 'errorType': 'Runtime.ImportModuleError', 'requestId': '<id>', 'stackTrace': []}
  AssertionError: lambda invocation failed (Unhandled): {'errorMessage': "Unable to import module 'index': No module named 'boto3'", 'errorType': 'Runtime.ImportModuleError', 'requestId': '<id>', 'stackTrace': []}
  AssertionError: execution role could not assume/run: 'Unhandled'
The module name follows the configured Handler ('lambda_function' or 'index'); the request ids are per-invocation values.

Cause 2 — Expected (AWS): CreateStack -> stack reaches CREATE_COMPLETE, the custom resource's physical id is recorded, and the IoT thing it created is readable.
Actual (emulator):
  File "check_iot__lambda_cfn_deploy_iot_thing_custom_resource_lambda_positive.py", line 242, in _deploy_cfn
  AssertionError: Stack sim2real-xsvc-stk-b1f42ce06450 ended in ROLLBACK_COMPLETE

## Steps to Reproduce
Cause 1: create a python Lambda function whose handler module begins with `import boto3` and which calls an IoT API (e.g. iot.create_thing / iot.describe_endpoint / iot.list_things / iot.update_thing) using the function's execution-role credentials; grant that role the corresponding iot:* action. Invoke the function. Expect HTTP 200 with the IoT result and no X-Amz-Function-Error; observe FunctionError 'Unhandled' with errorType 'Runtime.ImportModuleError' and errorMessage "Unable to import module '<handler-module>': No module named 'boto3'". Repeat with the iot action removed from the role and observe the same import failure instead of the IoT AccessDeniedException.

Cause 2: CreateStack with a template containing an AWS::IAM::Role, an AWS::Lambda::Function whose inline code creates an IoT thing and PUTs a SUCCESS response to the custom resource's ResponseURL, and a Custom:: resource whose ServiceToken is that function's ARN. Wait for the stack to settle. Expect CREATE_COMPLETE and the thing present; observe ROLLBACK_COMPLETE.
