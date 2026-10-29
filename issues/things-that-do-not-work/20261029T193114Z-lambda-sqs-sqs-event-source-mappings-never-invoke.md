Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sqs.batch-item-failures-ignored-without-functionresponsetypes lambda__sqs.esm-disabled-not-invoked lambda__sqs.esm-filter-criteria-non-matching-not-invoked lambda__sqs.sqs-esm-filters-are-ored-and-numeric-and-case-exact lambda__sqs.sqs-esm-message-attributes-keep-type-and-bytes lambda__sqs.sqs-esm-plain-string-body-filter-drops-non-matching-prefix lambda__sqs.sqs-esm-record-envelope-matches-the-sent-message lambda__sqs.sqs-esm-redelivered-record-keeps-identity-and-first-receive-time lambda__sqs.sqs-fifo-esm-record-carries-group-dedup-and-sequence-number
Issue-Key: lambda__sqs.batch-item-failures-ignored-without-functionresponsetypes|_receive#17|Timeout|got = sqs.receive_message(
Issue-Key: lambda__sqs.esm-disabled-not-invoked|_collect#18|Timeout|resp = sqs.receive_message(
Issue-Key: lambda__sqs.esm-filter-criteria-non-matching-not-invoked|_collect#12|Timeout|resp = sqs.receive_message(
Issue-Key: lambda__sqs.sqs-esm-filters-are-ored-and-numeric-and-case-exact|_drain#10|Timeout|resp = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10,
Issue-Key: lambda__sqs.sqs-esm-message-attributes-keep-type-and-bytes|_drain#8|Timeout|resp = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10,
Issue-Key: lambda__sqs.sqs-esm-plain-string-body-filter-drops-non-matching-prefix|_drain#12|Timeout|resp = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10,
Issue-Key: lambda__sqs.sqs-esm-record-envelope-matches-the-sent-message|_drain#11|Timeout|resp = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10,
Issue-Key: lambda__sqs.sqs-esm-redelivered-record-keeps-identity-and-first-receive-time|_drain#10|Timeout|resp = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10,
Issue-Key: lambda__sqs.sqs-fifo-esm-record-carries-group-dedup-and-sequence-number|_drain#11|Timeout|resp = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10,
Issue-Commit: 6712e9b9ed86b1379f14acf29b1637f3b8f9994b

# lambda__sqs: SQS event source mappings never invoke the function, so no record envelope, filter, attribute or redelivery behaviour is observable

> _Note: this issue was identified against the code base at commit `6712e9b9ed86b1379f14acf29b1637f3b8f9994b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
One cause, nine checks.

Cause 1 of 1: an SQS event source mapping is created and reported healthy, but the poller never invokes the function and never consumes the messages, so every check that waits for an invocation is killed at the 60s harness budget.
AWS: recorded behavior only for the poller; the model fixes the surface (lambda CreateEventSourceMapping takes EventSourceArn, FunctionName, BatchSize, FilterCriteria, Enabled and FunctionResponseTypes, and reports State, which AWS drives to Enabled). AWS then polls the source queue and invokes the function with an SQS Records envelope in which each record carries messageId, receiptHandle, body, md5OfBody, the message attributes with their DataType and bytes preserved, eventSource='aws:sqs' and eventSourceARN; for a FIFO source the record's attributes also carry MessageGroupId, MessageDeduplicationId and SequenceNumber. Filter criteria are OR-ed across patterns and matched exactly (numeric and case-sensitive); a redelivered message keeps its messageId and ApproximateFirstReceiveTimestamp; batchItemFailures in the response are honoured only when FunctionResponseTypes includes ReportBatchItemFailures; Enabled=false stops polling.
Emulator: no invocation is ever produced and nothing is dequeued, so none of those properties — envelope shape, attribute fidelity, filter semantics, redelivery identity, FIFO metadata, partial-batch handling, disabled-mapping suppression — can be observed at all.
Why: the queue -> function topology is the canonical serverless consumer. A client sees State=Enabled and a healthy mapping while messages pile up untouched; every positive behaviour is unavailable and every negative assertion ("must not be invoked") passes vacuously, so a later regression in filtering or disabling is undetectable.
Fix: the lambda event-source-mapping poller/runtime — dequeue from the source, build the SQS Records envelope with full per-record fidelity, apply FilterCriteria before invoking, invoke the target function, and then delete or re-expose messages according to the handler result and FunctionResponseTypes.
Traces: lambda__sqs.batch-item-failures-ignored-without-functionresponsetypes, lambda__sqs.esm-disabled-not-invoked, lambda__sqs.esm-filter-criteria-non-matching-not-invoked, lambda__sqs.sqs-esm-filters-are-ored-and-numeric-and-case-exact, lambda__sqs.sqs-esm-message-attributes-keep-type-and-bytes, lambda__sqs.sqs-esm-plain-string-body-filter-drops-non-matching-prefix, lambda__sqs.sqs-esm-record-envelope-matches-the-sent-message, lambda__sqs.sqs-esm-redelivered-record-keeps-identity-and-first-receive-time, lambda__sqs.sqs-fifo-esm-record-carries-group-dedup-and-sequence-number

Related open issue that does not carry these replay keys: 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md.

## Evidence
All nine checks: `Error: Check timed out after 60.0s`.
Expected: after SendMessage on the mapped queue, the function is invoked with Records[0] carrying the sent body, messageId, md5OfBody, messageAttributes (DataType + value bytes), eventSource='aws:sqs' and eventSourceARN (plus MessageGroupId/MessageDeduplicationId/SequenceNumber for a FIFO source).
Actual: no invocation is ever observed and the message remains on the queue.

## Steps to Reproduce
1. CreateQueue (and a .fifo queue for the FIFO check) and CreateFunction.
2. CreateEventSourceMapping(EventSourceArn=<queue arn>, FunctionName=<fn>, BatchSize=n) and wait for State='Enabled'.
3. SendMessage with message attributes.
4. Poll for any evidence of an invocation (the function's recorded side effect) — none appears, and the message is never deleted from the queue.
