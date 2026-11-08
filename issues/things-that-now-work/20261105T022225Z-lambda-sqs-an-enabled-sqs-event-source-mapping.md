Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sqs.batch-item-failures-ignored-without-functionresponsetypes lambda__sqs.esm-filter-criteria-non-matching-not-invoked lambda__sqs.sqs-esm-filters-are-ored-and-numeric-and-case-exact lambda__sqs.sqs-esm-message-attributes-keep-type-and-bytes lambda__sqs.sqs-esm-plain-string-body-filter-drops-non-matching-prefix lambda__sqs.sqs-esm-record-envelope-matches-the-sent-message lambda__sqs.sqs-esm-redelivered-record-keeps-identity-and-first-receive-time lambda__sqs.sqs-fifo-esm-record-carries-group-dedup-and-sequence-number
Issue-Key: lambda__sqs.batch-item-failures-ignored-without-functionresponsetypes|_receive#17|Timeout|got = sqs.receive_message(
Issue-Key: lambda__sqs.esm-filter-criteria-non-matching-not-invoked|_collect#12|Timeout|resp = sqs.receive_message(
Issue-Key: lambda__sqs.sqs-esm-filters-are-ored-and-numeric-and-case-exact|_drain#10|Timeout|resp = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10,
Issue-Key: lambda__sqs.sqs-esm-message-attributes-keep-type-and-bytes|_drain#8|Timeout|resp = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10,
Issue-Key: lambda__sqs.sqs-esm-plain-string-body-filter-drops-non-matching-prefix|_drain#12|Timeout|resp = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10,
Issue-Key: lambda__sqs.sqs-esm-record-envelope-matches-the-sent-message|_drain#11|Timeout|resp = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10,
Issue-Key: lambda__sqs.sqs-esm-redelivered-record-keeps-identity-and-first-receive-time|_drain#10|Timeout|resp = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10,
Issue-Key: lambda__sqs.sqs-fifo-esm-record-carries-group-dedup-and-sequence-number|_drain#11|Timeout|resp = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10,
Issue-Commit: 3ae1c5f9ad044bf0dd64ac25eb19f35c30d1d713

# lambda__sqs: an enabled SQS event-source mapping never consumes the queue or invokes its function, so none of the record-envelope, filter-criteria or FIFO behaviours reach a verdict

> _Note: this issue was identified against the code base at commit `3ae1c5f9ad044bf0dd64ac25eb19f35c30d1d713`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
One root cause behind eight failing checks. None of them is marked OWNED or ALREADY FILED in the replay report.

AWS: recorded behavior only for the poller itself; the control plane is modeled (lambda CreateEventSourceMapping / GetEventSourceMapping, whose State becomes Enabled). An Enabled SQS event-source mapping polls the queue, and for every message it delivers an invocation to the target function with an Records[] envelope whose members are messageId, receiptHandle, body, attributes (ApproximateReceiveCount, SentTimestamp, SenderId, ApproximateFirstReceiveTimestamp — plus MessageGroupId, MessageDeduplicationId and SequenceNumber for a FIFO queue), messageAttributes (each with stringValue/binaryValue and dataType preserved byte-for-byte), md5OfBody, eventSource "aws:sqs", eventSourceARN and awsRegion. Messages the mapping's FilterCriteria do not match are never delivered (and are deleted, not redriven); with FunctionResponseTypes unset a handler that returns batchItemFailures has that value ignored and the whole batch is redelivered; a redelivered record keeps its messageId and ApproximateFirstReceiveTimestamp while ApproximateReceiveCount increases.
Emulator: no delivery happens at all. Each of the eight checks sends its messages and then waits for the invocation to become observable; all eight are abandoned with "Check timed out after 60.0s" without reaching a verdict. The mapping is created and reports Enabled, but it never consumes the queue and never invokes the function, so the entire SQS-to-Lambda data plane — envelope shape, attribute fidelity, filter-criteria semantics, FIFO ordering metadata, redelivery identity, and partial-batch-response handling — is unobservable in this scenario group.
Why: a client whose architecture is "queue drives function" gets a configuration that looks correct and does nothing: messages accumulate in the queue, the function never runs, and nothing in the control plane reports a problem. Every behaviour layered on delivery is untestable, and the negative cases (a filter that should drop a message, a mapping that should ignore batchItemFailures) pass vacuously in the sense that no evidence is ever produced.
Fix: the Lambda event-source-mapping poller for SQS sources — it must poll the queue for an Enabled mapping, apply FilterCriteria, build the aws:sqs Records envelope, invoke the target function, and delete or redrive messages according to the handler's result and the mapping's FunctionResponseTypes. Open issues describe the same inert poller in other scenario groups (for example 20261029T183635Z-lambda-sns-sqs-sqs-event-source-mappings-never.md and 20261029T193114Z-lambda-s3-sqs-5-net-new-causes-sqs.md); neither carries these checks' replay keys.
Traces: all eight check ids listed in trace_ids.

## Evidence
Expected for each check: after SendMessage/SendMessageBatch on the mapped queue, the target function is invoked and the assertion about the delivered Records envelope (its members, its messageAttributes types and bytes, its FIFO MessageGroupId/MessageDeduplicationId/SequenceNumber, its unchanged messageId and ApproximateFirstReceiveTimestamp on redelivery, or the absence of a delivery for a non-matching filter) can be evaluated.
Actual (replay): `Error: Check timed out after 60.0s` for every one of lambda__sqs.batch-item-failures-ignored-without-functionresponsetypes, .esm-filter-criteria-non-matching-not-invoked, .sqs-esm-filters-are-ored-and-numeric-and-case-exact, .sqs-esm-message-attributes-keep-type-and-bytes, .sqs-esm-plain-string-body-filter-drops-non-matching-prefix, .sqs-esm-record-envelope-matches-the-sent-message, .sqs-esm-redelivered-record-keeps-identity-and-first-receive-time, .sqs-fifo-esm-record-carries-group-dedup-and-sequence-number — the invocation never becomes observable, so no envelope is ever produced to compare.

## Steps to Reproduce
1. Create an SQS queue (and a FIFO queue for the FIFO case) and a Lambda function that records the event it receives.
2. Call lambda CreateEventSourceMapping with EventSourceArn = the queue ARN, FunctionName = the function, Enabled = true (optionally with FilterCriteria, or with FunctionResponseTypes left unset); poll GetEventSourceMapping until State is Enabled.
3. SendMessage to the queue with message attributes of String/Binary/Number types.
4. Wait for the function to be invoked — it never is; the queue is never consumed and no aws:sqs Records envelope is ever delivered.
