Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__lambda__s3__sqs.sqs-esm-lambda-trigger-positive cloudformation__lambda__s3__sqs.sqs-esm-batch-attributes-preserved cloudformation__lambda__s3__sqs.sqs-esm-lambda-partial-batch-response-positive cloudformation__lambda__s3__sqs.sqs-to-lambda-to-s3-pipeline-positive cloudformation__lambda__s3__sqs.s3-to-sqs-to-lambda-pipeline-positive
Issue-Key: cloudformation__lambda__s3__sqs.sqs-esm-lambda-trigger-positive|_poll_until#10|AssertionError|raise AssertionError(
Issue-Key: cloudformation__lambda__s3__sqs.sqs-esm-batch-attributes-preserved|_poll_until#10|AssertionError|raise AssertionError(
Issue-Key: cloudformation__lambda__s3__sqs.sqs-esm-lambda-partial-batch-response-positive|_poll_until#17|AssertionError|raise AssertionError(
Issue-Key: cloudformation__lambda__s3__sqs.sqs-to-lambda-to-s3-pipeline-positive|_poll_until#10|AssertionError|raise AssertionError(
Issue-Key: cloudformation__lambda__s3__sqs.s3-to-sqs-to-lambda-pipeline-positive|_poll_until#16|AssertionError|raise AssertionError(
Issue-Commit: 8662809dae52673eae9c053e761f1b93b4d6fc67

# An SQS event source mapping never invokes its function, so every SQS->Lambda pipeline produces nothing

> _Note: this issue was identified against the code base at commit `8662809dae52673eae9c053e761f1b93b4d6fc67`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping (SQS source) / Invoke

## Description
This issue carries 1 net-new root cause.

Cause 1 of 1: a Lambda event source mapping over an SQS queue never delivers a batch — messages sent to the queue do not invoke the function, so none of the handler's downstream effects (here S3 objects) is ever produced.
Failing check ids for this cause: cloudformation__lambda__s3__sqs.sqs-esm-lambda-trigger-positive, cloudformation__lambda__s3__sqs.sqs-esm-batch-attributes-preserved, cloudformation__lambda__s3__sqs.sqs-esm-lambda-partial-batch-response-positive, cloudformation__lambda__s3__sqs.sqs-to-lambda-to-s3-pipeline-positive, cloudformation__lambda__s3__sqs.s3-to-sqs-to-lambda-pipeline-positive.

AWS: recorded behavior only for the poller's timing; the Smithy surface is Lambda CreateEventSourceMapping (EventSourceArn = the queue ARN, FunctionName, BatchSize, FunctionResponseTypes), SQS SendMessage/ReceiveMessage and S3 ListObjectsV2/GetObject. The recordings show four things, all of which begin with the mapping actually polling: (a) a message sent to the queue invokes the function within a short bounded wait and the handler's S3 object appears; (b) the event the handler receives carries each record's messageAttributes with the names, DataType and values the sender set; (c) a handler that returns {"batchItemFailures": [{"itemIdentifier": <messageId>}]} has only that message redelivered after the queue's VisibilityTimeout while the other records are not reprocessed; (d) an S3 ObjectCreated notification routed to the queue drives the same chain end to end, after the s3:TestEvent probe is drained.
Emulator: every one of these checks fails on its first observation of the handler's effect, with the poll returning nothing for its whole budget: "condition not met within 90s. Last result: []" (sqs-esm-lambda-trigger-positive, sqs-esm-batch-attributes-preserved, sqs-to-lambda-to-s3-pipeline-positive), "condition not met within 120s. Last result: []" (sqs-esm-lambda-partial-batch-response-positive) and "condition not met within 90s. Last result: None" (s3-to-sqs-to-lambda-pipeline-positive). No invocation happens, so the batch envelope, the message attributes and the partial-batch redelivery semantics are all unreachable.
Why: the single most common serverless consumer pattern does not work. Queues are never drained, work is never processed, and because nothing is ever invoked a client cannot distinguish a mapping that is mis-filtered from one that never polls — partial-batch failure handling, which exists precisely to avoid reprocessing a whole batch, is inert.
Fix: the Lambda event-source-mapping poller for SQS sources — poll the mapped queue, build the Records envelope (including each record's messageAttributes, attributes and messageId), invoke the function with BatchSize records, delete the records the invocation succeeded for, and honour a ReportBatchItemFailures response by leaving only the reported itemIdentifiers for redelivery after the queue's VisibilityTimeout.
Traces: the five check ids listed above; sqs-esm-lambda-trigger-positive is the minimal reproducer and s3-to-sqs-to-lambda-pipeline-positive the three-service chain.

## Evidence
Expected (recorded AWS): a SendMessage to the mapped queue invokes the function within seconds and the handler's S3 object is listable; the event's Records carry the sender's messageAttributes verbatim; a batchItemFailures response redelivers only the named message after VisibilityTimeout; an S3 ObjectCreated notification onto the queue drives the same chain.

Actual (emulator):
- sqs-esm-lambda-trigger-positive: AssertionError: condition not met within 90s. Last result: []
- sqs-esm-batch-attributes-preserved: AssertionError: condition not met within 90s. Last result: []
- sqs-esm-lambda-partial-batch-response-positive: AssertionError: condition not met within 120s. Last result: []
- sqs-to-lambda-to-s3-pipeline-positive: AssertionError: condition not met within 90s. Last result: []
- s3-to-sqs-to-lambda-pipeline-positive: AssertionError: condition not met within 90s. Last result: None

## Steps to Reproduce
1. CreateQueue and CreateBucket.
2. CreateFunction with a python handler that writes one S3 object per received record.
3. CreateEventSourceMapping with EventSourceArn = the queue ARN, that FunctionName, a BatchSize, and (for the partial-batch case) FunctionResponseTypes=["ReportBatchItemFailures"].
4. SendMessage (with MessageAttributes) to the queue.
5. Poll ListObjectsV2 on the bucket for the handler's object.
6. Observe the listing stays empty for the whole wait; the function is never invoked.
Or replay the five check ids in trace_ids.
