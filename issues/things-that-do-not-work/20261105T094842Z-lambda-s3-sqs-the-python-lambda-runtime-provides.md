Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__s3__sqs.cli-invoke-lambda-manual-test-event-positive lambda__s3__sqs.lambda-sdk-s3-copyobject-positive lambda__s3__sqs.lambda-sdk-s3-delete-object-positive lambda__s3__sqs.lambda-sdk-s3-generate-presigned-url-positive lambda__s3__sqs.lambda-sdk-s3-list-objects-v2-pagination-positive lambda__s3__sqs.lambda-sdk-s3-missing-deleteobject-permission-negative lambda__s3__sqs.lambda-sdk-s3-presigned-url-missing-permission-negative lambda__s3__sqs.lambda-sdk-sqs-missing-receivemessage-permission-negative lambda__s3__sqs.lambda-sdk-sqs-receive-and-delete-message-positive lambda__s3__sqs.lambda-sdk-sqs-send-message-batch-positive
Issue-Key: lambda__s3__sqs.cli-invoke-lambda-manual-test-event-positive|run#11|AssertionError|assert invoke_meta.get("FunctionError") is None, (
Issue-Key: lambda__s3__sqs.lambda-sdk-s3-copyobject-positive|_invoke_lambda#10|AssertionError|raise AssertionError(f"lambda invocation errored: {body}")
Issue-Key: lambda__s3__sqs.lambda-sdk-s3-delete-object-positive|_invoke_lambda#8|AssertionError|raise AssertionError(f"lambda invocation errored: {body}")
Issue-Key: lambda__s3__sqs.lambda-sdk-s3-generate-presigned-url-positive|_invoke_lambda#9|AssertionError|raise AssertionError(f"lambda invocation errored: {body}")
Issue-Key: lambda__s3__sqs.lambda-sdk-s3-list-objects-v2-pagination-positive|_invoke_lambda#8|AssertionError|raise AssertionError(f"lambda invocation errored: {body}")
Issue-Key: lambda__s3__sqs.lambda-sdk-s3-missing-deleteobject-permission-negative|_invoke_lambda#10|AssertionError|raise AssertionError(f"lambda invocation errored: {body}")
Issue-Key: lambda__s3__sqs.lambda-sdk-s3-presigned-url-missing-permission-negative|_invoke_lambda#9|AssertionError|raise AssertionError(f"lambda invocation errored: {body}")
Issue-Key: lambda__s3__sqs.lambda-sdk-sqs-missing-receivemessage-permission-negative|_invoke_lambda#9|AssertionError|raise AssertionError(f"lambda invocation errored: {body}")
Issue-Key: lambda__s3__sqs.lambda-sdk-sqs-receive-and-delete-message-positive|_invoke_lambda#9|AssertionError|raise AssertionError(f"lambda invocation errored: {body}")
Issue-Key: lambda__s3__sqs.lambda-sdk-sqs-send-message-batch-positive|_invoke_lambda#9|AssertionError|raise AssertionError(f"lambda invocation errored: {body}")
Issue-Commit: 30d5092aa3ebb101aaddada2a4fffe08f2e86509

# lambda__s3__sqs: the Python Lambda runtime provides no AWS SDK, so no handler can call S3 or SQS — every invocation faults Runtime.ImportModuleError

> _Note: this issue was identified against the code base at commit `30d5092aa3ebb101aaddada2a4fffe08f2e86509`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
This issue carries 1 net-new root cause. It must be closed before submitting.

Cause 1 of 1: the emulator's Python Lambda runtime makes no AWS SDK importable, so a handler that does `import boto3` never runs; every in-function s3:CopyObject / s3:DeleteObject / s3:ListObjectsV2 / presigned-URL generation and sqs:SendMessageBatch / sqs:ReceiveMessage / sqs:DeleteMessage call — and the allow/deny grading built on those calls — is unreachable.

AWS: Lambda's managed Python runtimes (python3.9 … python3.13) ship boto3 and botocore in the runtime image, so a deployment package containing only the handler module may `import boto3`, build a client from the credentials Lambda injects for the execution role, and call other services; `boto3.client('s3').generate_presigned_url(...)` is likewise available because botocore is present. A successful RequestResponse Invoke answers HTTP 200 with NO X-Amz-Function-Error header and the handler's return value as the payload. When the execution role lacks the action, the call inside the handler raises the service's own authorization error (S3 `AccessDenied` / `User: arn:aws:sts::<account>:assumed-role/<role>/<session> is not authorized to perform: <action> on resource: <arn>`) which the handler returns as data — the invocation itself still succeeds. Smithy: Invoke → InvocationResponse.StatusCode (httpResponseCode), FunctionError (X-Amz-Function-Error), Payload (httpPayload); Invoke models no "SDK missing" error, because on AWS it never occurs. Recorded behavior for these checks: the handler's copy/delete/list/presign result and the SQS send-batch/receive-and-delete result come back as the payload, the copied object exists and the deleted one is gone, the paginated list walks every page, the presigned URL resolves, and the denied twin reports the service's AccessDenied.

Emulator: the import fails before the handler runs. Invoke answers HTTP 200 with X-Amz-Function-Error: Unhandled, ExecutedVersion $LATEST and payload `{"errorMessage": "Unable to import module '<handler-module>': No module named 'boto3'", "errorType": "Runtime.ImportModuleError", "requestId": "<per-invocation id>", "stackTrace": []}`. Fixed text: `Unable to import module '` … `': No module named 'boto3'` and errorType `Runtime.ImportModuleError`; the variable parts are the handler module from the function's Handler (observed as `index` and `lambda_function`) and the per-invocation requestId — neither recorded value is part of the requirement.

Why: a client relying on AWS's behavior loses every in-function AWS call, which is the main reason Lambdas exist. S3 and SQS work done from a function produces nothing, and the emulator returns the IDENTICAL Runtime.ImportModuleError whether the execution role was granted or denied the action, so IAM enforcement observed through a function cannot be graded and a denial is indistinguishable from a broken deployment package.

Fix: the Lambda Invoke execution path's Python runtime environment — services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/invoke.rs and the runtime/sandbox it delegates to when it imports and calls the handler — must make boto3/botocore importable in the handler interpreter and point their default endpoint and credentials at the emulator, so in-handler client calls are served and authorized like any other request.

Traces: the 10 failing check ids in trace_ids. This cause is already filed for OTHER services (20261105T081139Z-dynamodb-lambda-s3-secretsmanager-the-in-process-lambda.md, 20261105T081727Z-lambda-sns-sqs-the-emulator-s-python-lambda.md); neither carries this service's checks or their replay keys.

## Evidence
Expected (AWS): Invoke of a handler that imports boto3 answers HTTP 200 with no X-Amz-Function-Error and the handler's own result; the S3/SQS effect is visible (object copied/deleted, every page listed, presigned URL usable, batch delivered, message received and deleted), and the denied twin returns the service's AccessDenied.
Actual (emulator): HTTP 200 + X-Amz-Function-Error: Unhandled with payload {"errorMessage": "Unable to import module 'lambda_function': No module named 'boto3'", "errorType": "Runtime.ImportModuleError", "requestId": "<id>", "stackTrace": []} (also with module 'index'). As reported by the checks:
  .lambda-sdk-s3-copyobject-positive — AssertionError: lambda invocation errored: {... No module named 'boto3' ...}
  .lambda-sdk-s3-delete-object-positive — same
  .lambda-sdk-s3-generate-presigned-url-positive — same
  .lambda-sdk-s3-list-objects-v2-pagination-positive — same
  .lambda-sdk-s3-missing-deleteobject-permission-negative — same
  .lambda-sdk-s3-presigned-url-missing-permission-negative — same
  .lambda-sdk-sqs-missing-receivemessage-permission-negative — same
  .lambda-sdk-sqs-receive-and-delete-message-positive — same
  .lambda-sdk-sqs-send-message-batch-positive — same
  .cli-invoke-lambda-manual-test-event-positive — AssertionError: unexpected FunctionError: meta={'StatusCode': 200, 'FunctionError': 'Unhandled', 'ExecutedVersion': '$LATEST'} payload={'errorMessage': "Unable to import module 'index': No module named 'boto3'", 'errorType': 'Runtime.ImportModuleError', ...}

## Steps to Reproduce
CreateFunction with Runtime=python3.12, Handler=lambda_function.handler and a single-module package whose handler does `import boto3` and performs one call — e.g. boto3.client('s3').copy_object(...) or boto3.client('sqs').send_message_batch(...) — granting the execution role that action; Invoke with InvocationType=RequestResponse.
AWS: HTTP 200, no X-Amz-Function-Error, the handler's result, and the effect visible in S3/SQS.
Emulator: HTTP 200 with X-Amz-Function-Error: Unhandled and payload {"errorMessage": "Unable to import module 'lambda_function': No module named 'boto3'", "errorType": "Runtime.ImportModuleError", ...}; no effect.
Repeat with the action removed from the role: AWS returns the service's AccessDenied from inside the handler, the emulator returns the identical ImportModuleError. Replay the 10 check ids in trace_ids.
