Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__sqs.sqs-dlq-redrive-reprocesses-after-function-fix dynamodb__lambda__sqs.sqs-fifo-esm-partial-batch-response-deletes-a-later-same-group-success-despite-an-earlier-failure
Issue-Key: dynamodb__lambda__sqs.sqs-dlq-redrive-reprocesses-after-function-fix|_poll_until#33|Timeout|time.sleep(interval_s)
Issue-Key: dynamodb__lambda__sqs.sqs-fifo-esm-partial-batch-response-deletes-a-later-same-group-success-despite-an-earlier-failure|run#47|Timeout|time.sleep(3)
Issue-Commit: 977af42ea79b884a07e35b887b5e587a14d941a9

# dynamodb__lambda__sqs: an SQS event source mapping never delivers, so DLQ redrive after a function fix and FIFO partial-batch deletion are both unobservable

> _Note: this issue was identified against the code base at commit `977af42ea79b884a07e35b887b5e587a14d941a9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Cause 1 of 1: an enabled SQS event source mapping never invokes its function, so neither the redrive-after-fix path nor FIFO partial-batch handling produces anything to grade.

AWS: an enabled event source mapping polls the queue and invokes the function with an SQS record batch. Consequences the recordings capture: messages that exhaust maxReceiveCount land in the redrive policy's DLQ and, after the function is fixed, StartMessageMoveTask (redrive) returns them to the source queue where the mapping processes them to completion; and for a FIFO source with ReportBatchItemFailures, a batchItemFailures response deletes the successfully processed messages — including a later message of the same MessageGroupId whose earlier sibling failed is NOT deleted, while the reported failures return for redelivery. Smithy (lambda CreateEventSourceMapping, members FunctionResponseTypes/BatchSize) models the configuration but not the polling; delivery is asynchronous, so this is recorded behavior only.

Emulator: both checks exhaust the 60 s harness budget ('Error: Check timed out after 60.0s') waiting for the invocations and the resulting queue state; no batch is ever delivered.

Why: a client's queue-driven function appears configured (the mapping exists and reads back Enabled) yet nothing is consumed; DLQ redrive and partial-batch semantics — the two mechanisms clients rely on for at-least-once correctness — cannot be exercised at all.

Fix: the event-source-mapping poller/invoker for SQS sources, including redrive-policy interaction and ReportBatchItemFailures handling for FIFO groups.

Traces: dynamodb__lambda__sqs.sqs-dlq-redrive-reprocesses-after-function-fix, dynamodb__lambda__sqs.sqs-fifo-esm-partial-batch-response-deletes-a-later-same-group-success-despite-an-earlier-failure. Related open issues that do NOT carry these replay keys: 20261029T180026Z-dynamodb-lambda-sqs-sqs-and-dynamodb-stream-event-source-mappings-never.md, 20261029T065950Z-dynamodb-lambda-sqs-an-sqs-event-source-mapping-01e4ea.md.

## Evidence
Expected: messages exceeding maxReceiveCount appear in the DLQ, and after the function is fixed a redrive returns them and they are processed; for the FIFO source the batchItemFailures response leaves the reported failures for redelivery and deletes the successes.
Actual: 'Error: Check timed out after 60.0s' for both checks — no invocation and no queue-state change within the budget.

## Steps to Reproduce
1. CreateQueue (standard with a RedrivePolicy to a DLQ; and separately a FIFO queue), CreateFunction, CreateEventSourceMapping (Enabled, FunctionResponseTypes=['ReportBatchItemFailures'] for the FIFO leg). 2. SendMessage(s), let the failing function exhaust maxReceiveCount, verify the DLQ, fix the function, StartMessageMoveTask, and poll for processing. 3. For FIFO, send ordered messages in one MessageGroupId and have the handler report the first as a batch item failure. Replay both check ids in trace_ids.
