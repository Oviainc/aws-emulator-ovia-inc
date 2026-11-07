Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__sns__sqs.lambda-sdk-publish-to-sns
Issue-Key: events__lambda__sns__sqs.lambda-sdk-publish-to-sns|_poll_until#11|AssertionError|raise AssertionError(
Issue-Commit: 959ef406fada22402fcbc8404625fdf48db790f8

# events__lambda__sns__sqs: the Lambda runtime's bundled SDK has no 'sns' client, so a handler's sns:Publish can never succeed

> _Note: this issue was identified against the code base at commit `959ef406fada22402fcbc8404625fdf48db790f8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
Cause 1 of 1: boto3.client('sns') cannot be constructed inside the emulator's Lambda runtime.

AWS: the Lambda Python runtime ships an AWS SDK that can build a client for any AWS service, so a handler doing boto3.client('sns').publish(TopicArn=..., Message=...) succeeds once the execution role grants sns:Publish (and is refused AuthorizationError/AccessDenied by SNS itself while it does not), and the published message reaches the topic's SQS subscription.

Emulator: the handler raises before any call is made — errorType "UnknownServiceError", errorMessage "Unknown service: 'sns'. Valid service names are: sqs, events, eventbridge, dynamodb, logs, cloudwatchlogs, apigateway, secretsmanager" (fixed text; the valid-name list is the emulator's own registry). The invocation returns StatusCode 200 with error 'Unhandled', so the grant-propagation leg never turns ok and nothing is published.

Why: every in-function SNS fan-out stops working, and worse, a permission test cannot distinguish "denied" from "no such client": the handler fails identically with and without the IAM grant, so a client validating its least-privilege policy gets a false negative.

Fix: the Lambda runtime's bundled SDK client registry — it must expose a client for sns (and the rest of the emulated services) and route the call through the normal authorization path so an ungranted call is refused by SNS with its own error code. Related open issue 20261105T081727Z-events-lambda-sns-sqs-a-lambda-handler-cannot.md covers the sibling negative check; the suite reports this check as un-owned.

Traces: events__lambda__sns__sqs.lambda-sdk-publish-to-sns.

## Evidence
Expected: after the role grant propagates, the handler's sns:Publish returns a MessageId and the subscribed queue receives the message.
Actual: AssertionError: Lambda's sns:Publish succeeding once its role grant propagates not met within 60.0s. Last result: {'status_code': 200, 'error': 'Unhandled', 'payload': {'errorMessage': "Unknown service: 'sns'. Valid service names are: sqs, events, eventbridge, dynamodb, logs, cloudwatchlogs, apigateway, secretsmanager", 'errorType': 'UnknownServiceError', ...}}

## Steps to Reproduce
1. CreateTopic and subscribe an SQS queue to it.
2. CreateFunction (python runtime) whose handler calls boto3.client('sns').publish(TopicArn=<topic>, Message="from-lambda-sdk"), with an execution role granting sns:Publish on the topic.
3. Invoke the function and read the payload; poll the queue.
Observed: UnknownServiceError for 'sns' at client construction; nothing published.
