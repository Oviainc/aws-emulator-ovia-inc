Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__sns__sqs.lambda-sdk-publish-to-sns events__lambda__sns__sqs.lambda-sdk-publish-to-sns-missing-permission events__lambda__sns__sqs.lambda-sdk-putevents-to-bus events__lambda__sns__sqs.lambda-sdk-sendmessage-to-sqs
Issue-Key: events__lambda__sns__sqs.lambda-sdk-publish-to-sns|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Key: events__lambda__sns__sqs.lambda-sdk-publish-to-sns-missing-permission|_poll_until#17|AssertionError|raise AssertionError(
Issue-Key: events__lambda__sns__sqs.lambda-sdk-putevents-to-bus|_wait_for_messages#14|Timeout|resp = sqs.receive_message(
Issue-Key: events__lambda__sns__sqs.lambda-sdk-sendmessage-to-sqs|_wait_for_messages#5|AssertionError|raise AssertionError(
Issue-Commit: ce1e7fa07219b3a065b602a90626dda6f8aa6c34

# events__lambda__sns__sqs: a Lambda handler cannot call SNS, SQS or EventBridge — `import boto3` fails with Runtime.ImportModuleError, so no in-function Publish/SendMessage/PutEvents is ever made

> _Note: this issue was identified against the code base at commit `ce1e7fa07219b3a065b602a90626dda6f8aa6c34`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
This issue carries 1 net-new root cause. It must be closed before submitting.

Cause 1 of 1: a python3.x function whose handler imports the AWS SDK and then calls SNS Publish, SQS SendMessage or EventBridge PutEvents never executes that call — the handler fails at import time, so the downstream topic, queue and bus stay empty.

AWS: the managed Python runtimes bundle the AWS SDK for Python (boto3/botocore) in the runtime image, so `import boto3` resolves for a deployment package containing only the handler module, and the runtime injects the execution role's credentials so a client built inside the handler is signed as that role. Smithy: Invoke answers 200 for a RequestResponse invocation whose handler returned normally, with no `x-amz-function-error` header; `FunctionError: Unhandled` is documented only for an error raised while running the function. Recorded behavior: the handler's Publish puts the message on the topic and its subscribed queue receives it; its SendMessage makes the message receivable on the queue; its PutEvents matches the rule on the bus. For the missing-permission member, the in-handler call raises a botocore ClientError with an AccessDenied-class code that the handler catches and reports in its own payload, while the Invoke itself still answers 200 with no FunctionError.

Emulator: the handler module is never imported. Invoke answers 200 with `FunctionError: Unhandled` and the payload `{"errorMessage": "Unable to import module '<module>': No module named 'boto3'", "errorType": "Runtime.ImportModuleError", "requestId": "<uuid>", "stackTrace": []}` — fixed text `Unable to import module '`…`': No module named 'boto3'` plus `Runtime.ImportModuleError`, variable parts the handler module name from the request's `Handler` and the per-invocation requestId. Because the SDK call never happens, the checks that watch the destination instead report nothing arriving: `AssertionError: Expected 1 messages, got 0 within 45s`, `message delivered to queue <url> not met within 45.0s. Last result: []`, or the 60s check budget expiring. The in-process interpreter the run happens in registers an allow-list of guest modules with no socket layer and no boto3/botocore on the import path.

Why: a client relying on AWS's behavior sees Lambda-as-glue produce no effect at all: fan-out from a function to a topic, a queue or an event bus silently does nothing, and the permission-denial path is replaced by an import crash, so a negative test cannot distinguish "IAM refused the call" from "the call was never made".

Fix: the Lambda execution engine's guest environment — services/service-com-amazonaws-lambda-awsgirapiservice/src/state/execution/python.rs (guest module allow-list and import path) and src/state/execution/bootstrap.rs — must expose an AWS SDK surface to the guest that dispatches to the emulator's own SNS/SQS/EventBridge front doors under the function's execution-role identity.

Traces: the 4 failing checks in trace_ids. The same cause under other service headings is filed separately as the lambda__sns__sqs and events__lambda__sns issues from this sweep.

## Evidence
Expected (AWS): Invoke 200 with no FunctionError, and the handler's Publish/SendMessage/PutEvents observable on the topic, queue or bus.
Actual (emulator):
  events__lambda__sns__sqs.lambda-sdk-sendmessage-to-sqs — AssertionError: Expected 1 messages, got 0 within 45s
  events__lambda__sns__sqs.lambda-sdk-publish-to-sns-missing-permission — AssertionError: message delivered to queue https://sqs.<region>.amazonaws.com/<account>/<queue> not met within 45.0s. Last result: []
  events__lambda__sns__sqs.lambda-sdk-publish-to-sns — Check timed out after 60.0s
  events__lambda__sns__sqs.lambda-sdk-putevents-to-bus — Check timed out after 60.0s
The underlying function payload on this emulator, shown by the same handler shape under the lambda__sns__sqs heading, is: {'errorMessage': "Unable to import module 'lambda_function': No module named 'boto3'", 'errorType': 'Runtime.ImportModuleError', 'stackTrace': []} with x-amz-function-error: Unhandled.

## Steps to Reproduce
Replay the checks in trace_ids. Each creates a role, zips a single-module python3.x handler that does `import boto3` and then calls sns.publish / sqs.send_message / events.put_events, creates the function, invokes it, and polls the destination. Minimal repro: CreateFunction (Runtime=python3.12, Handler=lambda_function.handler) with a package whose only module begins `import boto3`, Invoke it, then ReceiveMessage on the destination queue — AWS delivers one message, the emulator delivers none and the Invoke carries FunctionError=Unhandled.
