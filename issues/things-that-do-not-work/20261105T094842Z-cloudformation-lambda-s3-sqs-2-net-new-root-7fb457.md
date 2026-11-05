Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__lambda__s3__sqs.cfn-deploy-broken-lambda-role-negative cloudformation__lambda__s3__sqs.cfn-deploy-lambda-sqs-stack-positive cloudformation__lambda__s3__sqs.lambda-env-var-queue-url-resolution-positive cloudformation__lambda__s3__sqs.lambda-sdk-cfn-describe-stacks-missing-permission-negative cloudformation__lambda__s3__sqs.lambda-sdk-cfn-describe-stacks-positive cloudformation__lambda__s3__sqs.lambda-sdk-s3-getobject-positive cloudformation__lambda__s3__sqs.lambda-sdk-s3-missing-putobject-permission-negative cloudformation__lambda__s3__sqs.lambda-sdk-s3-putobject-positive cloudformation__lambda__s3__sqs.lambda-sdk-send-sqs-message-positive cloudformation__lambda__s3__sqs.lambda-sdk-send-sqs-missing-permission-negative cloudformation__lambda__s3__sqs.s3-bucket-policy-blocks-lambda-getobject-negative cloudformation__lambda__s3__sqs.sqs-queue-policy-blocks-lambda-send-negative cloudformation__lambda__s3__sqs.cfn-stack-update-lambda-env-positive
Issue-Key: cloudformation__lambda__s3__sqs.cfn-deploy-broken-lambda-role-negative|run#8|AssertionError|"AccessDenied" in payload_str
Issue-Key: cloudformation__lambda__s3__sqs.cfn-deploy-lambda-sqs-stack-positive|run#14|AssertionError|assert "error" not in invoke_result, (
Issue-Key: cloudformation__lambda__s3__sqs.lambda-env-var-queue-url-resolution-positive|_wait_for_messages#6|AssertionError|raise AssertionError(
Issue-Key: cloudformation__lambda__s3__sqs.lambda-sdk-cfn-describe-stacks-missing-permission-negative|run#14|AssertionError|assert granted.get("outcome") == "allowed", f"Positive control failed: {granted}"
Issue-Key: cloudformation__lambda__s3__sqs.lambda-sdk-cfn-describe-stacks-positive|run#9|AssertionError|assert "cfn-sdk-result" in payload_str, (
Issue-Key: cloudformation__lambda__s3__sqs.lambda-sdk-s3-getobject-positive|run#9|AssertionError|assert body_val == '{"answer": 42}', f"Unexpected body: {body_val}"
Issue-Key: cloudformation__lambda__s3__sqs.lambda-sdk-s3-missing-putobject-permission-negative|run#10|AssertionError|assert isinstance(payload, dict) and "get" in payload and "put" in payload, (
Issue-Key: cloudformation__lambda__s3__sqs|GetObject|NoSuchKey
Issue-Key: cloudformation__lambda__s3__sqs.lambda-sdk-send-sqs-message-positive|_wait_for_messages#6|AssertionError|raise AssertionError(
Issue-Key: cloudformation__lambda__s3__sqs.lambda-sdk-send-sqs-missing-permission-negative|_poll_until#7|AssertionError|raise AssertionError(
Issue-Key: cloudformation__lambda__s3__sqs.s3-bucket-policy-blocks-lambda-getobject-negative|run#11|AssertionError|assert before["payload"].get("body") == "secret", (
Issue-Key: cloudformation__lambda__s3__sqs.sqs-queue-policy-blocks-lambda-send-negative|run#11|AssertionError|isinstance(baseline_payload, dict)
Issue-Key: cloudformation__lambda__s3__sqs.cfn-stack-update-lambda-env-positive|run#13|WaiterError|waiter.wait(StackName=stack_name, WaiterConfig={"Delay": 2, "MaxAttempts": 150})
Issue-Commit: 30d5092aa3ebb101aaddada2a4fffe08f2e86509

# cloudformation__lambda__s3__sqs: 2 net-new root causes — the Python Lambda runtime provides no AWS SDK (boto3), and a stack update that changes a Lambda Environment variable ends UPDATE_ROLLBACK_COMPLETE

> _Note: this issue was identified against the code base at commit `30d5092aa3ebb101aaddada2a4fffe08f2e86509`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
This issue carries 2 net-new root causes. BOTH must be closed before submitting.

=== Cause 1 of 2: the emulator's Python Lambda runtime makes no AWS SDK importable, so every handler that does `import boto3` dies with Runtime.ImportModuleError and no in-function SDK call (CloudFormation, S3 or SQS) is ever exercised ===

AWS: Lambda's managed Python runtimes (python3.9 … python3.13) ship the AWS SDK for Python — boto3 and botocore — inside the runtime image's site-packages, so a deployment package that contains only the handler module may `import boto3`, build a client from the invocation's injected credentials (AWS_ACCESS_KEY_ID / AWS_SECRET_ACCESS_KEY / AWS_SESSION_TOKEN / AWS_REGION) and call other services under the function's execution role. A successful RequestResponse Invoke then answers HTTP 200 with NO X-Amz-Function-Error header and the handler's own return value as the payload. Smithy: Invoke → InvocationResponse.StatusCode (httpResponseCode), FunctionError (X-Amz-Function-Error), Payload (httpPayload); Invoke's modeled errors do not include any "SDK missing" case, because on AWS the SDK is never missing. Recorded behavior for these checks: a handler calling cloudformation:DescribeStacks returns its `cfn-sdk-result`; a handler calling s3:GetObject returns the object body `{"answer": 42}`; a handler calling sqs:SendMessage makes the message appear in the queue; and a handler whose role lacks the action returns an AccessDenied/AccessDeniedException error from the SDK call inside the function (HTTP 200, no FunctionError) rather than faulting.

Emulator: the import itself fails. Invoke answers HTTP 200 with X-Amz-Function-Error: Unhandled, ExecutedVersion $LATEST and payload `{"errorMessage": "Unable to import module '<handler-module>': No module named 'boto3'", "errorType": "Runtime.ImportModuleError", "requestId": "<per-invocation id>", "stackTrace": []}`. Fixed text: `Unable to import module '` … `': No module named 'boto3'` and the errorType `Runtime.ImportModuleError`; the variable parts are the handler module taken from the function's Handler (observed as `index` and `lambda_function`) and the per-invocation requestId — do not treat either recorded value as part of the requirement. Downstream of the fault the checks see the knock-on shapes: `AssertionError: Expected 1 messages, got 0 within 30s` (nothing was sent), `NoSuchKey … The specified key does not exist` on the GetObject of an object the handler was supposed to put, and `Unexpected body: None`.

Why: a client relying on AWS's behavior loses every in-function AWS call. Any Lambda that talks to another service — the overwhelmingly common shape — cannot run at all, so both the positive paths (the function's effect on S3/SQS/CloudFormation) and the negative, authorization-grading paths (the function's role being denied an action) are indistinguishable from a broken deployment package: the emulator reports the SAME Runtime.ImportModuleError whether the role was granted or denied, so IAM enforcement cannot be observed through a function.

Fix: the Lambda Invoke execution path's Python runtime environment — services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/invoke.rs and the runtime/sandbox it delegates to when it imports and calls the handler — must put the AWS SDK (boto3/botocore) on the handler interpreter's module search path and point its default endpoint/credentials at the emulator, so `import boto3` resolves and client calls made inside a handler are served and authorized like any other request.

Traces: cloudformation__lambda__s3__sqs.cfn-deploy-broken-lambda-role-negative, .cfn-deploy-lambda-sqs-stack-positive, .lambda-env-var-queue-url-resolution-positive, .lambda-sdk-cfn-describe-stacks-missing-permission-negative, .lambda-sdk-cfn-describe-stacks-positive, .lambda-sdk-s3-getobject-positive, .lambda-sdk-s3-missing-putobject-permission-negative, .lambda-sdk-s3-putobject-positive, .lambda-sdk-send-sqs-message-positive, .lambda-sdk-send-sqs-missing-permission-negative, .s3-bucket-policy-blocks-lambda-getobject-negative, .sqs-queue-policy-blocks-lambda-send-negative. This cause is already filed for OTHER services (20261105T081727Z-lambda-sns-sqs-the-emulator-s-python-lambda.md, 20261105T081727Z-events-lambda-sns-sqs-a-lambda-handler-cannot.md, 20261105T081139Z-dynamodb-lambda-s3-secretsmanager-the-in-process-lambda.md, 20261105T074953Z-lambda-scheduler-the-python3-12-invoke-runtime-provides.md); none of them carries this service's checks or their replay keys.

=== Cause 2 of 2: a CloudFormation stack update that only changes an AWS::Lambda::Function's Environment variable ends in UPDATE_ROLLBACK_COMPLETE instead of UPDATE_COMPLETE ===

AWS: UpdateStack on a template whose only difference is the value of an AWS::Lambda::Function Environment.Variables entry is an in-place (non-replacing) update of that resource: the stack goes UPDATE_IN_PROGRESS → UPDATE_COMPLETE_CLEANUP_IN_PROGRESS → UPDATE_COMPLETE, the function keeps its physical id, and the next Invoke observes the NEW environment value. Smithy: cloudformation UpdateStack returns UpdateStackOutput.StackId and the stack's terminal status is read through DescribeStacks (Stacks[].StackStatus); the StackUpdateComplete waiter treats UPDATE_ROLLBACK_COMPLETE as a terminal FAILURE state. Recorded behavior: the first deploy and invoke succeed and return the v1 environment value, so the update is the only thing that diverges.

Emulator: the update rolls back. DescribeStacks reports StackStatus UPDATE_ROLLBACK_COMPLETE, and the client's waiter fails with `botocore.exceptions.WaiterError: Waiter StackUpdateComplete failed: Waiter encountered a terminal failure state: For expression "Stacks[].StackStatus" we matched expected path: "UPDATE_ROLLBACK_COMPLETE" at least once`. Nothing in the template is invalid, so there is no legitimate reason for the rollback; the stack is left on the v1 environment.

Why: a client cannot change a deployed function's configuration through CloudFormation at all. Every redeploy that only tweaks an environment variable — the most common Lambda update there is — is reported as a failed, rolled-back update, so CI/CD and CDK `deploy` loops see a failure where AWS reports success, and the running function silently keeps the old configuration.

Fix: the CloudFormation stack-update provisioner for AWS::Lambda::Function — the update/modify path of that resource provider and the stack-update orchestration that decides CREATE/UPDATE/REPLACE and triggers rollback on a resource failure — must apply an Environment.Variables change in place and let the stack reach UPDATE_COMPLETE.

Traces: cloudformation__lambda__s3__sqs.cfn-stack-update-lambda-env-positive. The open issue 20261105T081225Z-cloudformation-lambda-ssm-a-stack-update-that-changes.md reports a neighbouring symptom (UPDATE_FAILED with a ResourceConflict "An update is in progress for resource") for a different service's stack update; it does not carry this check or its replay key, and the status observed here is UPDATE_ROLLBACK_COMPLETE.

## Evidence
Cause 1 — Expected (AWS): a RequestResponse Invoke of a handler that imports boto3 answers HTTP 200 with no X-Amz-Function-Error and the handler's result; its SDK call takes effect (message in the queue, object in the bucket, DescribeStacks result echoed) or is refused with AccessDenied/AccessDeniedException inside the handler when the execution role lacks the action.
Actual (emulator): HTTP 200 + X-Amz-Function-Error: Unhandled, payload {"errorMessage": "Unable to import module 'index': No module named 'boto3'", "errorType": "Runtime.ImportModuleError", "requestId": "<id>", "stackTrace": []} (also seen with module 'lambda_function'). Reported by the checks as:
  .cfn-deploy-broken-lambda-role-negative — AssertionError: Expected AccessDenied in response, got: {"errorMessage": "Unable to import module 'index': No module named 'boto3'", "errorType": "Runtime.ImportModuleError", ...}
  .cfn-deploy-lambda-sqs-stack-positive — AssertionError: expected no FunctionError, got 'Unhandled': {... Runtime.ImportModuleError ...}
  .lambda-sdk-cfn-describe-stacks-positive — AssertionError: Expected 'cfn-sdk-result' in Lambda response: {... No module named 'boto3' ...}
  .lambda-sdk-cfn-describe-stacks-missing-permission-negative — AssertionError: Positive control failed: {... No module named 'boto3' ...}
  .lambda-sdk-s3-missing-putobject-permission-negative — AssertionError: Expected a {'get': ..., 'put': ...} payload, got: {... No module named 'boto3' ...}
  .s3-bucket-policy-blocks-lambda-getobject-negative — AssertionError: positive control did not read the object back: {... No module named 'boto3' ...}
  .sqs-queue-policy-blocks-lambda-send-negative — AssertionError: baseline send (before any queue policy) must succeed: {... No module named 'boto3' ...}; the deny leg below would prove nothing
  .lambda-sdk-send-sqs-missing-permission-negative — AssertionError: SDK send succeeds while sqs:SendMessage is granted not met within 30.0s. Last result: {'status_code': 200, 'error': 'Unhandled', 'payload': {... Runtime.ImportModuleError ...}}
  .lambda-sdk-send-sqs-message-positive / .lambda-env-var-queue-url-resolution-positive — AssertionError: Expected 1 messages, got 0 within 30s
  .lambda-sdk-s3-getobject-positive — AssertionError: Unexpected body: None
  .lambda-sdk-s3-putobject-positive — botocore.errorfactory.NoSuchKey: An error occurred (NoSuchKey) when calling the GetObject operation: The specified key does not exist.

Cause 2 — Expected (AWS): after UpdateStack with the v2 template, DescribeStacks reports UPDATE_COMPLETE and the next Invoke returns the v2 environment value.
Actual (emulator): botocore.exceptions.WaiterError: Waiter StackUpdateComplete failed: Waiter encountered a terminal failure state: For expression "Stacks[].StackStatus" we matched expected path: "UPDATE_ROLLBACK_COMPLETE" at least once (check cfn-stack-update-lambda-env-positive; the first deploy and the v1 invoke both succeeded).

## Steps to Reproduce
Cause 1: CreateFunction with Runtime=python3.12, Handler=index.handler and a one-file package whose handler does `import boto3` and performs one call (e.g. boto3.client('sqs').send_message(QueueUrl=..., MessageBody='x')); grant the execution role that action; Invoke with InvocationType=RequestResponse. AWS: HTTP 200, no X-Amz-Function-Error, the message appears in the queue. Emulator: HTTP 200 with X-Amz-Function-Error: Unhandled and the Runtime.ImportModuleError payload, and the queue stays empty. Repeat with the action REMOVED from the role: AWS returns the SDK's AccessDenied inside the handler, the emulator returns the identical ImportModuleError, so the two cases are indistinguishable. Replay the 12 Cause-1 check ids in trace_ids.

Cause 2: CreateStack from a template declaring an AWS::Lambda::Function with Environment.Variables.VERSION=v1 (Capabilities=CAPABILITY_NAMED_IAM), wait for CREATE_COMPLETE, Invoke and observe v1; then UpdateStack with the same template except VERSION=v2 and wait on the StackUpdateComplete waiter. AWS: UPDATE_COMPLETE and the next Invoke returns v2. Emulator: the stack reaches UPDATE_ROLLBACK_COMPLETE and the waiter fails. Replay cloudformation__lambda__s3__sqs.cfn-stack-update-lambda-env-positive.
