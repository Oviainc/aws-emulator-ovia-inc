Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__sqs.ddb-stream-esm-filter-criteria-insert-only-not-invoked dynamodb__lambda__sqs.ddb-stream-keys-only-view-excludes-new-image dynamodb__lambda__sqs.queue-policy-deny-suppresses-esm-delivery dynamodb__lambda__sqs.sqs-fifo-esm-message-group-order-preserved
Issue-Key: dynamodb__lambda__sqs.ddb-stream-esm-filter-criteria-insert-only-not-invoked|_collect#16|Timeout|resp = sqs.receive_message(
Issue-Key: dynamodb__lambda__sqs.ddb-stream-keys-only-view-excludes-new-image|_collect#16|Timeout|resp = sqs.receive_message(
Issue-Key: dynamodb__lambda__sqs.queue-policy-deny-suppresses-esm-delivery|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Key: dynamodb__lambda__sqs.sqs-fifo-esm-message-group-order-preserved|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Commit: f4e5251e6df88363d8e23ace2ff5bff3be31b423

# SQS and DynamoDB-stream event source mappings never deliver, so filter criteria, stream views, queue-policy deny and FIFO ordering are unobservable

> _Note: this issue was identified against the code base at commit `f4e5251e6df88363d8e23ace2ff5bff3be31b423`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
One net-new root cause in the dynamodb__lambda__sqs group, hit by four checks.

Cause 1 of 1: event source mappings over SQS queues and DynamoDB streams never deliver to the function.
AWS: an enabled mapping polls its source and invokes the function — `FilterCriteria` limits delivery to matching records (e.g. INSERT only), a KEYS_ONLY stream view delivers records without a NewImage, a queue policy that denies the ESM's reads suppresses delivery, and a FIFO queue preserves per-MessageGroupId order across invocations.
Emulator: no delivery occurs at all; every one of these checks polls for the function's effect and ends with `Check timed out after 60.0s`, so neither the positive semantics nor the suppression negative can be observed.
Why: queue- and stream-driven compute is silently inert: messages and stream records are accepted and nothing runs. A client cannot distinguish "filtered out", "denied by the queue policy" and "never polled", and cannot rely on FIFO ordering it has no way to observe. (The open issues 20261029T065950Z-dynamodb-lambda-sqs-an-sqs-event-source-mapping-01e4ea.md and 20261028T230054Z-dynamodb-lambda-sqs-the-sqs-poison-message-dlq.md carry other checks in this group; they do not carry these replay keys.)
Fix: the lambda event-source-mapping poller for SQS and DynamoDB streams — delivery, FilterCriteria evaluation, StreamViewType projection, queue-policy authorization of the poller, and FIFO message-group ordering.
Traces: all four checks below.

## Evidence
All four failures are the same: Error: Check timed out after 60.0s —
- dynamodb__lambda__sqs.ddb-stream-esm-filter-criteria-insert-only-not-invoked
- dynamodb__lambda__sqs.ddb-stream-keys-only-view-excludes-new-image
- dynamodb__lambda__sqs.queue-policy-deny-suppresses-esm-delivery
- dynamodb__lambda__sqs.sqs-fifo-esm-message-group-order-preserved

## Steps to Reproduce
Boot the emulator and replay the four checks listed in trace_ids: each creates an event source mapping (SQS queue or table stream), produces messages/records, and polls DynamoDB for the function's effect, which never arrives.
