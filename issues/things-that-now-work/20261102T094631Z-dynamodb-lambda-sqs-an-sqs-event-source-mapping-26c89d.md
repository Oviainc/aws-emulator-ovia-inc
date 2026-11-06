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

## Resolution

The cause's premise — "an enabled SQS event source mapping never invokes its function ... no
batch is ever delivered" — does not hold any more: an enabled mapping on a queue polls it,
invokes the mapped function with the `aws:sqs` record batch, reads the function's partial
batch report and deletes or leaves each message accordingly, and a message no attempt
processes is dead-lettered by the queue's own `RedrivePolicy` and reprocessed once a move
task returns it to the source queue. Both legs are driven end to end, over the router and
through a deployment package that RUNS, by
`services/service-com-amazonaws-lambda-awsgirapiservice/tests/all/sqs_event_source_handler_run.rs`.

What the recordings did expose in that scope, and what is FIXED here, is the deletion set of a
partial batch report: the poller intersected the report's complement with a message-group rule
of its own, so a message the report left out was held back by a reported failure EARLIER in its
own `MessageGroupId` — which for a single-group batch whose first message was reported left the
whole batch on the queue. Lambda deletes exactly the messages the report does not name,
whatever group each carries (`state/sqs_poller.rs`, `processed_in_group_order` removed).

Still OUTSTANDING for the two replayed checks, and filed on its own: their handlers reach
DynamoDB through the AWS SDK and their assertions poll DynamoDB for the item the handler
writes, and this emulator's python invoke runtime registers no AWS SDK on a handler's import
path, so such a handler ends at module import whatever the poller delivers. That capability is
`20261105T094343Z-events-lambda-sqs-the-lambda-python-runtime-provides.md` (and
`20261105T081644Z-events-lambda-sqs-stepfunctions-the-lambda-python-runtime.md`), which is the
invoke runtime rather than the event-source-mapping poller this issue names.

