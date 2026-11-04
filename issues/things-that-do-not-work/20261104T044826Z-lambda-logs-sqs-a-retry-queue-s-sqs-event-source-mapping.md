Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__logs__sqs.retry-queue-message-is-processed-through-its-sqs-mapping
Issue-Key: lambda__logs__sqs.retry-queue-message-is-processed-through-its-sqs-mapping|run#10|Timeout|time.sleep(4)
Issue-Commit: b4ead8169e48e6047949699a772a41fd75a6bc4d

# A retry queue's SQS event source mapping never processes its message

> _Note: this issue was identified against the code base at commit `b4ead8169e48e6047949699a772a41fd75a6bc4d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Cause 1 of 1: a message sent to a retry queue that has a Lambda event source mapping must be received, handed to the function and deleted.
AWS: recorded behavior only — the recording sends a message to the retry queue, then reads the handler's log lines in /aws/lambda/<name> to confirm the body was processed and checks the queue drains; on AWS this settles inside the suite's 60s budget.
Emulator: the check is cut off with "Error: Check timed out after 60.0s" — CreateEventSourceMapping is accepted and reports Enabled, but no batch is ever delivered, so the handler never logs and the message is never removed.
Why: a retry/DLQ-reprocessing path silently stalls while the mapping reports healthy; the message stays in the queue and the client has no signal that nothing is consuming it.
Fix: the Lambda event-source-mapping poller for SQS (ReceiveMessage/batch/invoke/DeleteMessage) and the mapping state it reports.
Traces: lambda__logs__sqs.retry-queue-message-is-processed-through-its-sqs-mapping.

## Evidence
Expected (AWS): the retry-queue message is delivered to the handler, logged, and deleted from the queue.
Actual (emulator): 'Error: Check timed out after 60.0s' — the mapping is Enabled but nothing is delivered.

## Steps to Reproduce
1. CreateQueue (retry queue) and a Lambda; CreateEventSourceMapping from the queue ARN to the function.
2. SendMessage to the queue.
3. Poll the function's log group for the processed body and the queue for drain. Observe neither happens within 60s.
