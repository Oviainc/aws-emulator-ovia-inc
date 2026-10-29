Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__sqs.conc-limits-and-fanout-sqs-esm-handler-error-redelivers-whole-batch dynamodb__lambda__sqs.conc-limits-and-fanout-sqs-esm-invalid-item-identifier-rejected-whole-batch-redelivered dynamodb__lambda__sqs.conc-limits-and-fanout-sqs-esm-partial-batch-redelivers-exactly-reported-items dynamodb__lambda__sqs.revoked-downstream-permission-is-denied-with-the-exact-authz-message-and-the-message-lands-in-the-dlq
Issue-Key: dynamodb__lambda__sqs.conc-limits-and-fanout-sqs-esm-handler-error-redelivers-whole-batch|_poll_tally#5|TimeoutError|raise TimeoutError(f'{what}: the run budget ended first; not graded')
Issue-Key: dynamodb__lambda__sqs.conc-limits-and-fanout-sqs-esm-invalid-item-identifier-rejected-whole-batch-redelivered|_poll_tally#5|TimeoutError|raise TimeoutError(f'{what}: the run budget ended first; not graded')
Issue-Key: dynamodb__lambda__sqs.conc-limits-and-fanout-sqs-esm-partial-batch-redelivers-exactly-reported-items|_poll_tally#5|TimeoutError|raise TimeoutError(f'{what}: the run budget ended first; not graded')
Issue-Key: dynamodb__lambda__sqs.revoked-downstream-permission-is-denied-with-the-exact-authz-message-and-the-message-lands-in-the-dlq|_poll_item#40|TimeoutError|raise TimeoutError(f"item {pk!r} appearing in {table!r}: the run budget ended first; not graded")
Issue-Commit: c17c6e54c038586d9d6328bb68b9d1b3d6bcf360

# dynamodb__lambda__sqs: an SQS event source mapping never delivers any message to the function, so no batch is ever processed, redelivered or dead-lettered (1 cause)

> _Note: this issue was identified against the code base at commit `c17c6e54c038586d9d6328bb68b9d1b3d6bcf360`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Cause 1 of 1: an ENABLED SQS event source mapping never receives from the queue and never invokes the mapped function, so not a single message is delivered even once.

AWS: CreateEventSourceMapping with an SQS queue ARN starts a poller that calls ReceiveMessage on the queue and invokes the mapped function with the batch; on a successful invocation the batch's messages are deleted. On a handler exception the WHOLE batch becomes visible again after the queue's VisibilityTimeout and is redelivered; with FunctionResponseTypes=["ReportBatchItemFailures"] a response listing batchItemFailures redelivers exactly the reported messageIds, and a response containing an itemIdentifier that is not in the batch is rejected so the whole batch is redelivered; after maxReceiveCount failed receives the message is moved to the RedrivePolicy's dead-letter queue, whose first receive reports ApproximateReceiveCount carried over from the source queue. Downstream refusals raised inside the handler surface as that service's own error (here DynamoDB's AccessDeniedException, HTTP 400, "User: <principal> is not authorized to perform: dynamodb:PutItem on resource: <table arn> because no identity-based policy allows the dynamodb:PutItem action" — fixed text with the principal and table ARN taken from the request/state).

Emulator: the poller is inert. All four checks fail at their very first delivery calibration — the baseline where every message must be delivered at least once — and never reach the behaviour under test: three end with TimeoutError "not every message was delivered even once: the run budget ended first; not graded", and the authz/DLQ check ends with TimeoutError "item 'A' appearing in '<table>': the run budget ended first; not graded" because the handler that would write the baseline item is never run. CreateEventSourceMapping itself returns and reports the mapping Enabled; nothing is ever received from the queue and the function is never invoked.

Why: SQS-triggered Lambda is the standard queue-worker pattern. A client gets a mapping that reports healthy while the queue drains to nothing and no work happens — messages are neither processed nor redelivered nor dead-lettered, ApproximateReceiveCount never advances, ReportBatchItemFailures partial-batch semantics are unobservable, and a failing worker produces no DLQ copy to diagnose. Any retry/poison-message design built on these guarantees is untestable and, in practice, silently broken.

Fix: the event-source-mapping poller/dispatch component for the SQS source type in the lambda crate — the loop that must ReceiveMessage from the mapped queue, invoke the function, Delete on success, leave the batch to its VisibilityTimeout on failure, honour FunctionResponseTypes=ReportBatchItemFailures (including rejecting an unknown itemIdentifier), and let the queue's RedrivePolicy/maxReceiveCount move the message to the DLQ. Open issues describe the same inert poller reached from other service combinations (20261027T180000Z-lambda-an-sqs-event-source-mapping-never-consumes-or-delivers.md, 20261026T094219Z-dynamodb-lambda-sqs-an-sqs-event-source-mapping.md, 20261028T230054Z-dynamodb-lambda-sqs-the-sqs-poison-message-dlq.md); none carries these four checks' replay keys.

Traces: dynamodb__lambda__sqs.conc-limits-and-fanout-sqs-esm-handler-error-redelivers-whole-batch, dynamodb__lambda__sqs.conc-limits-and-fanout-sqs-esm-invalid-item-identifier-rejected-whole-batch-redelivered, dynamodb__lambda__sqs.conc-limits-and-fanout-sqs-esm-partial-batch-redelivers-exactly-reported-items, dynamodb__lambda__sqs.revoked-downstream-permission-is-denied-with-the-exact-authz-message-and-the-message-lands-in-the-dlq

## Evidence
Expected (AWS): after SendMessage of N messages to a queue with an ENABLED SQS event source mapping, every message is delivered to the function at least once (the tally table records each index), and then: a thrown handler error redelivers the whole batch; a ReportBatchItemFailures response redelivers exactly the reported ids; a bogus itemIdentifier redelivers the whole batch; a handler whose DynamoDB PutItem is denied gets AccessDeniedException (400) with the message "User: <principal> is not authorized to perform: dynamodb:PutItem on resource: <table arn> because no identity-based policy allows the dynamodb:PutItem action" and, after maxReceiveCount receives, the message appears in the DLQ with ApproximateReceiveCount carried over.

Actual (emulator): no message is delivered even once.
  conc-limits-and-fanout-sqs-esm-handler-error-redelivers-whole-batch -> TimeoutError: not every message was delivered even once: the run budget ended first; not graded
  conc-limits-and-fanout-sqs-esm-invalid-item-identifier-rejected-whole-batch-redelivered -> TimeoutError: not every message was delivered even once: the run budget ended first; not graded
  conc-limits-and-fanout-sqs-esm-partial-batch-redelivers-exactly-reported-items -> TimeoutError: not every message was delivered even once: the run budget ended first; not graded
  revoked-downstream-permission-...-lands-in-the-dlq -> TimeoutError: item 'A' appearing in 'sim2real-g1-tbl-3edd55b0073f': the run budget ended first; not graded
(the table name is a per-run value; the requirement is that the handler's baseline write happens at all.)

## Steps to Reproduce
1. CreateQueue with VisibilityTimeout >= the function Timeout; for the DLQ leg also create a DLQ and set the source queue's RedrivePolicy with maxReceiveCount.
2. CreateTable for the tally/results table.
3. CreateFunction for a handler that records each received messageId/index into the tally table and, per mode, throws / returns batchItemFailures / returns a bogus itemIdentifier / writes to DynamoDB.
4. CreateEventSourceMapping(EventSourceArn=<queue arn>, FunctionName=<fn>, BatchSize=<n>, FunctionResponseTypes=["ReportBatchItemFailures"] where applicable); wait for State=Enabled.
5. SendMessage N messages and poll the tally table.
Observed: the tally table stays empty — not one delivery. Expected: every message delivered at least once, then the mode-specific redelivery/DLQ behaviour.
