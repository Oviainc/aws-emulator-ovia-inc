Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sqs.function-sdk-send-to-unpermitted-queue-denied-as-function-error
Issue-Key: lambda__sqs.function-sdk-send-to-unpermitted-queue-denied-as-function-error|run#9|AssertionError|assert err is None, f"the permitted SendMessage failed the invocation: {err!r} {out!r}"
Issue-Commit: 30d5092aa3ebb101aaddada2a4fffe08f2e86509

# lambda__sqs: a handler's permitted SendMessage never runs — the Python Lambda runtime provides no AWS SDK, so the invocation faults Runtime.ImportModuleError instead of returning the SDK's verdict

> _Note: this issue was identified against the code base at commit `30d5092aa3ebb101aaddada2a4fffe08f2e86509`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
This issue carries 1 net-new root cause. It must be closed before submitting.

Cause 1 of 1: the emulator's Python Lambda runtime makes no AWS SDK importable, so a handler that does `import boto3` never runs. The check's design — a handler that sends to a PERMITTED queue (must succeed) and then to an UNPERMITTED queue (must surface as a function error carrying the denial) — cannot be graded at all, because both legs fault identically before the handler executes.

AWS: Lambda's managed Python runtimes (python3.9 … python3.13) ship boto3 and botocore in the runtime image, so a package containing only the handler module may `import boto3` and call SQS with the credentials Lambda injects for the execution role. The permitted SendMessage answers HTTP 200 with NO X-Amz-Function-Error header and the handler's result (the MessageId/MD5 it returns). The unpermitted SendMessage raises the SDK's ClientError inside the handler — `AccessDenied` with message `User: arn:aws:sts::<account>:assumed-role/<role>/<session> is not authorized to perform: sqs:SendMessage on resource: <queue-arn> because no identity-based policy allows the sqs:SendMessage action` — and, if the handler lets it propagate, the invocation answers 200 with X-Amz-Function-Error: Unhandled and a payload whose errorType is the SDK exception class (`ClientError`) and whose errorMessage carries that AccessDenied text. The fixed text is the `is not authorized to perform: <action> on resource: <arn>` template; the account, role, session, action and queue ARN are values from the request and the emulator's state. Smithy: Invoke → InvocationResponse.StatusCode (httpResponseCode), FunctionError (X-Amz-Function-Error), Payload (httpPayload); Invoke models no "SDK missing" error, because on AWS it never happens.

Emulator: the import fails before the handler runs. Even the PERMITTED leg answers HTTP 200 with X-Amz-Function-Error: Unhandled and payload `{"errorMessage": "Unable to import module 'index': No module named 'boto3'", "errorType": "Runtime.ImportModuleError", "requestId": "<per-invocation id>", "stackTrace": []}`. Fixed text: `Unable to import module '` … `': No module named 'boto3'` and errorType `Runtime.ImportModuleError`; the variable parts are the handler module from the function's Handler and the per-invocation requestId.

Why: a client relying on AWS's behavior cannot tell an authorization denial apart from a broken deployment package: the emulator returns the same Runtime.ImportModuleError for the permitted and the unpermitted queue, so sqs:SendMessage enforcement observed through a function is unobservable, and no handler that talks to SQS runs at all.

Fix: the Lambda Invoke execution path's Python runtime environment — services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/invoke.rs and the runtime/sandbox it delegates to when it imports and calls the handler — must make boto3/botocore importable in the handler interpreter and point their default endpoint and credentials at the emulator, so the in-handler SendMessage is served and authorized like any other request and the denial surfaces as the SDK's AccessDenied.

Traces: lambda__sqs.function-sdk-send-to-unpermitted-queue-denied-as-function-error. This cause is already filed for OTHER services (20261105T081727Z-lambda-sns-sqs-the-emulator-s-python-lambda.md, 20261105T074953Z-lambda-scheduler-the-python3-12-invoke-runtime-provides.md); neither carries this service's check or its replay key. The open issue 20261105T022225Z-lambda-sqs-an-enabled-sqs-event-source-mapping.md covers this service's event-source-mapping delivery, not the runtime's missing SDK.

## Evidence
Expected (AWS): the permitted leg answers HTTP 200 with no X-Amz-Function-Error and the handler's SendMessage result; the unpermitted leg surfaces as a function error whose payload carries `AccessDenied` / `... is not authorized to perform: sqs:SendMessage on resource: <queue-arn> ...`.
Actual (emulator): lambda__sqs.function-sdk-send-to-unpermitted-queue-denied-as-function-error — AssertionError: the permitted SendMessage failed the invocation: 'Unhandled' {'errorMessage': "Unable to import module 'index': No module named 'boto3'", 'errorType': 'Runtime.ImportModuleError', 'requestId': '<id>', 'stackTrace': []}. The permitted control never succeeds, so the deny leg proves nothing.

## Steps to Reproduce
Create two queues; CreateFunction with Runtime=python3.12, Handler=index.handler and a single-module package whose handler does `import boto3` and calls boto3.client('sqs').send_message(QueueUrl=<url>, MessageBody=...) for the queue named in the event; grant the execution role sqs:SendMessage on the FIRST queue only. Invoke once per queue with InvocationType=RequestResponse.
AWS: queue 1 → HTTP 200, no X-Amz-Function-Error, the handler's MessageId; queue 2 → the SDK's AccessDenied surfaced from inside the handler (`... is not authorized to perform: sqs:SendMessage on resource: <queue-arn> ...`).
Emulator: BOTH answer HTTP 200 with X-Amz-Function-Error: Unhandled and payload {"errorMessage": "Unable to import module 'index': No module named 'boto3'", "errorType": "Runtime.ImportModuleError", ...}. Replay lambda__sqs.function-sdk-send-to-unpermitted-queue-denied-as-function-error.
