Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__cloudwatch__lambda__logs__sqs__ssm.cfn-change-set-queue-worker-update-workflow
Issue-Key: cloudformation__cloudwatch__lambda__logs__sqs__ssm.cfn-change-set-queue-worker-update-workflow|_poll_until#19|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Commit: 5bc50a64f180637d452285d750c7aafc588926d7

# An SQS-triggered Lambda worker deployed by CloudFormation never consumes the queue — no invocation and no log line for a sent message

> _Note: this issue was identified against the code base at commit `5bc50a64f180637d452285d750c7aafc588926d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Cause 1 of 1: the Lambda event-source mapping created for an SQS queue never polls the queue, so a message sent to the queue produces no invocation and the worker writes no CloudWatch Logs record.

AWS: an ENABLED AWS::Lambda::EventSourceMapping whose EventSourceArn is an SQS queue is polled by the Lambda service; each received message is delivered to the function as an SQS event record, the function runs, and its stdout/stderr lands in the function's /aws/lambda/<function-name> log group within seconds. Smithy (lambda): CreateEventSourceMapping input EventSourceArn/FunctionName/Enabled; output State (the mapping becomes `Enabled`); there is no modeled state in which an enabled mapping legitimately never polls. Recorded behavior only for the polling latency; the recording shows the message delivered and logged well inside the 300 s budget.

Emulator: after the stack reaches CREATE_COMPLETE and a message is sent, FilterLogEvents over the worker's log group returns `[]` for the full 300 s wait — the worker log line for the sent message never appears, so the mapping is not consuming the queue (no invocation at all).

Why: a client that wires SQS to Lambda — the standard queue-worker pattern, and the subject of change-set/update workflows — observes a stack that deploys cleanly but processes nothing. Messages pile up in the queue, alarms built on the worker's metrics never fire, and an update workflow cannot be verified because the "before" behaviour was never there.

Fix: the Lambda event-source-mapping poller (the data-plane component that drives ReceiveMessage against the mapped SQS queue and invokes the function), and the invocation's log emission into the function's log group.

Traces: check cloudformation__cloudwatch__lambda__logs__sqs__ssm.cfn-change-set-queue-worker-update-workflow.

Related but not covering: open issues 20261107T184737Z-cloudformation-lambda-sqs-ssm-a-sam-sqs-event-s-generated-eventsourcemapping-never.md and 20261106T153504Z-cloudformation-lambda-sqs-ssm-a-sam-sqs-event.md cover SAM-generated mappings; this check's mapping is declared directly in a CloudFormation template and is grounded in a different failing check.

## Evidence
Expected: within seconds of SendMessage, the function is invoked once per message and FilterLogEvents on the worker's log group contains a line carrying the message marker.

Actual: `AssertionError: worker log line for message <marker> not met within 300.0s. Last result: []` — the log group yields no events at all, so no invocation happened. The stack itself reported CREATE_COMPLETE and the mapping was created.

## Steps to Reproduce
1. CreateStack a template with an SQS queue, a Lambda function and an AWS::Lambda::EventSourceMapping (Enabled, EventSourceArn = the queue) whose handler logs the message body; wait for CREATE_COMPLETE.
2. sqs SendMessage a uniquely-marked body to the queue.
3. Poll logs FilterLogEvents on /aws/lambda/<worker function> for the marker — it stays empty, and the queue is never drained.
