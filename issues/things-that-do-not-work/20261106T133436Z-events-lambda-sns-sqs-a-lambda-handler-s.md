Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__sns__sqs.lambda-sdk-publish-to-sns
Issue-Key: events__lambda__sns__sqs.lambda-sdk-publish-to-sns|_invoke_lambda#11|Timeout|resp = ctx.client("lambda").invoke(
Issue-Commit: 2b63d043efdd2fa9855b978e8c81153e1c6b6b87

# events__lambda__sns__sqs: a Lambda handler's sns:Publish never reaches the topic's subscriber, so the SDK-publish fan-out check never reaches a verdict

> _Note: this issue was identified against the code base at commit `2b63d043efdd2fa9855b978e8c81153e1c6b6b87`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
This issue carries 1 net-new root cause. It must be closed before submitting. Related but NOT covering: the open issue 20261105T081727Z-lambda-sns-sqs-the-emulator-s-python-lambda.md owns the equivalent check under the lambda__sns__sqs heading; this heading's check is unowned by the suite, and the symptom has moved on from "no boto3 at all" to the SNS-client gap described below.

Cause 1 of 1: an SNS Publish issued by function code is never carried out, so a topic's SQS subscriber receives nothing and the invocation never settles into a success the check can act on.
AWS: the managed python runtimes bundle a complete boto3/botocore and inject the execution role's credentials, so `boto3.client("sns").publish(TopicArn=..., Message=...)` from a handler whose role allows sns:Publish on that topic succeeds. Smithy: Publish(Message required, TopicArn) -> {MessageId}; its modeled refusals are AuthorizationErrorException (403, awsQuery code `AuthorizationError`), NotFoundException (404, `NotFound`), InvalidParameterException (400, `InvalidParameter`) and the KMS family — nothing in the model makes a permitted publish a no-op. A confirmed sqs-protocol subscription whose queue policy allows sns.amazonaws.com:SendMessage for that SourceArn then receives the notification envelope, whose `Message` member is the published body verbatim. Recorded behavior for this check: one message arrives on the subscribed queue with Message == the string the handler published, and the Invoke itself returns the handler's `{"ok": true}` with no FunctionError.
Emulator: the check never gets there — it exhausts its budget and is killed (`Check timed out after 60.0s`). Its first stage polls Invoke for up to 60 s until the response carries no function error, and only then waits up to 45 s for the queue; the timeout therefore means the handler's publish path never produces a clean invocation (and consequently no message on the queue). Hypothesis: the same guest-SDK gap filed in this sweep under the lambda__sns__sqs heading — the in-runtime SDK resolves no `sns` service client and raises botocore `UnknownServiceError` — applies here, where the client is built at module import (`sns = boto3.client("sns")` at module scope), so every Invoke faults and the poll can never clear.
Why: a client relying on AWS's behavior gets silence instead of a fan-out. A function that publishes to a topic is the backbone of Lambda-driven eventing; here the publish neither succeeds nor is refused with a code the caller can branch on, so subscribers never fire and nothing in the chain is observable. A harness driving this pattern hangs instead of failing fast.
Fix: the Lambda execution engine's guest SDK surface — register the `sns` service in the guest botocore catalogue and dispatch Publish to the emulator's SNS front door under the function's execution-role identity, so the published message enters the topic's normal delivery path to its SQS subscription.
Traces: events__lambda__sns__sqs.lambda-sdk-publish-to-sns.

## Evidence
events__lambda__sns__sqs.lambda-sdk-publish-to-sns
Expected (AWS): Invoke returns the handler's payload {'ok': True} with no FunctionError, and within seconds the queue subscribed to the topic holds exactly one notification whose Message member equals the published body (the check publishes the literal 'from-lambda-sdk').
Actual (emulator): Error: Check timed out after 60.0s — the bounded poll "Lambda's sns:Publish succeeding once its role grant propagates" never sees a clean invocation, so the subsequent 45 s wait for the subscribed queue is never even reached and no message is delivered.

## Steps to Reproduce
Replay events__lambda__sns__sqs.lambda-sdk-publish-to-sns.
Minimal repro: create a topic and a queue, set the queue policy to allow sns.amazonaws.com sqs:SendMessage with ArnEquals aws:SourceArn = the topic, Subscribe the queue ARN with Protocol=sqs; create an IAM role trusted by lambda.amazonaws.com with an inline Allow of sns:Publish on that topic; CreateFunction (Runtime=python3.12) whose module body does `sns = boto3.client("sns")` and whose handler calls sns.publish(TopicArn=<topic>, Message='from-lambda-sdk') and returns {'ok': True}; Invoke (RequestResponse), then ReceiveMessage on the queue. AWS: the invoke succeeds and one notification arrives with Message == 'from-lambda-sdk'. Emulator: the invoke never yields a clean result and the queue stays empty.
