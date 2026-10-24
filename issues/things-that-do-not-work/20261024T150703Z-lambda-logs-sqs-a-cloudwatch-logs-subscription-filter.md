Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__logs__sqs.subscription-filter-delivers-log-events-to-lambda
Issue-Key: lambda__logs__sqs.subscription-filter-delivers-log-events-to-lambda|_receive_messages#16|Timeout|resp = sqs.receive_message(
Issue-Commit: 483d650f1d2fec6203edc8ec91bc65f69df3bd2d

# lambda__logs__sqs: a CloudWatch Logs subscription filter with a Lambda destination delivers nothing (1 cause)

> _Note: this issue was identified against the code base at commit `483d650f1d2fec6203edc8ec91bc65f69df3bd2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutSubscriptionFilter

## Description
One root cause. Cause count: 1.

=== Cause 1 of 1: a subscription filter whose destinationArn is a Lambda function never invokes it ===
AWS: PutSubscriptionFilter with a Lambda function as destinationArn (plus the lambda:InvokeFunction permission for logs.<region>.amazonaws.com) causes CloudWatch Logs to invoke that function asynchronously for each batch of matching events, passing the gzip+base64 'awslogs' envelope (owner, logGroup, logStream, subscriptionFilters, logEvents). The function then observably forwards what it received — here, onto an SQS queue the check reads.
Emulator: nothing is ever delivered. The check polls the queue (returning as soon as one message arrives, so a correct emulator would end it in one iteration) and instead runs to the suite's 60 s cap with an empty queue. The emulator has no in-process Lambda runtime — lambda Invoke answers NotImplemented for a real deployment package (open issue 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md records that for direct Invoke) — and, separately, the logs subscription-filter fan-out to a Lambda destination produces no invocation attempt at all, so even an emulator that could run the function would not be handed the envelope.
Why: Lambda-destination subscription filters are one of the two main CWL fan-out targets. A client gets a filter that DescribeSubscriptionFilters reports as installed and a function that is never called, with no error anywhere to indicate the delivery path is inert.
Fix: the logs subscription-filter delivery path for Lambda destinations (it must build the awslogs envelope and invoke the function), and the Lambda invocation runtime it depends on.
Traces: lambda__logs__sqs.subscription-filter-delivers-log-events-to-lambda.

## Evidence
lambda__logs__sqs.subscription-filter-delivers-log-events-to-lambda: 'Check timed out after 60.0s'. The check creates the function, grants logs.<region>.amazonaws.com invoke permission, installs the subscription filter, writes matching events with PutLogEvents and then polls SQS (_receive_messages / _poll_until, which return on the first message) for what the function forwarded — no message ever arrived. Expected: at least one message carrying the decoded awslogs envelope for the matching events.

## Steps to Reproduce
Create a Lambda function that forwards its event to an SQS queue; lambda AddPermission(Principal='logs.<region>.amazonaws.com', Action='lambda:InvokeFunction'); logs CreateLogGroup + CreateLogStream; logs PutSubscriptionFilter(logGroupName, filterName, filterPattern, destinationArn=<function ARN>); logs PutLogEvents with matching lines; sqs ReceiveMessage — the queue stays empty.
