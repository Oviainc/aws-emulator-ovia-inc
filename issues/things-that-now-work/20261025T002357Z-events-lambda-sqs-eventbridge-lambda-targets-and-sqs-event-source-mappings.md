Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__sqs.eventbridge-rule-invokes-lambda-target events__lambda__sqs.events-rule-sqs-esm-lambda-pipeline events__lambda__sqs.sqs-lambda-partial-batch-failure-reprocessing events__lambda__sqs.sqs-fifo-lambda-message-group-order-preserved
Issue-Key: events__lambda__sqs.eventbridge-rule-invokes-lambda-target|_collect_messages#14|Timeout|resp = sqs.receive_message(
Issue-Key: events__lambda__sqs.events-rule-sqs-esm-lambda-pipeline|_reemit_until_written#30|Timeout|time.sleep(1)
Issue-Key: events__lambda__sqs.sqs-lambda-partial-batch-failure-reprocessing|_poll_until#21|Timeout|time.sleep(interval_s)
Issue-Key: events__lambda__sqs|SendMessage|NotImplemented
Issue-Commit: 6f030629c911e30ed6652430413113629aa7dccf

# EventBridge Lambda targets and SQS event-source mappings never run the function, and SendMessage to a FIFO queue is an unimplemented stub (3 causes)

> _Note: this issue was identified against the code base at commit `6f030629c911e30ed6652430413113629aa7dccf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTargets

## Description
Cause 1 of 3 — a rule whose target is a Lambda function never invokes it.
AWS: PutTargets with `Arn: arn:aws:lambda:...:function:<fn>` plus a lambda:InvokeFunction resource policy granting events.amazonaws.com makes EventBridge invoke the function asynchronously with the event envelope as the payload.
Emulator: the function is never run — the check's terminal read (the function's own recorded side effect) never appears and the replay cuts the check off at 60 s.
Why: the single most common EventBridge target type produces nothing; a rule that reports FailedEntryCount 0 and a function that reports no errors combine into silent data loss.
Fix: the Lambda target in the EventBridge target dispatcher (services/service-com-amazonaws-eventbridge-awsevents) and the invocation path it calls. The already-open 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md covers the synchronous Invoke stub; it does not carry these checks' replay keys and does not cover the EventBridge-to-Lambda dispatch or the ESM poller below.
Traces: events__lambda__sqs.eventbridge-rule-invokes-lambda-target.

Cause 2 of 3 — an SQS event-source mapping never delivers a batch to its function.
AWS: a Lambda event-source mapping on an SQS queue polls the queue and invokes the function with a Records batch; messages the function reports in `batchItemFailures` (ReportBatchItemFailures) become visible again and are redelivered, while the rest are deleted.
Emulator: the function is never invoked, so no batch is consumed, nothing is deleted from the queue and partial-batch-failure reprocessing cannot occur.
Why: the EventBridge -> SQS -> Lambda pipeline, and SQS-backed worker queues generally, do nothing; messages pile up with no consumer and no error reported anywhere.
Fix: the SQS event-source-mapping poller and its batch-response handling (partial-batch failures), in the Lambda service's event-source-mapping component.
Traces: events__lambda__sqs.events-rule-sqs-esm-lambda-pipeline, events__lambda__sqs.sqs-lambda-partial-batch-failure-reprocessing.

Cause 3 of 3 — SQS SendMessage to a FIFO queue is an unimplemented scaffold stub.
AWS: `SendMessage` to a `.fifo` queue takes MessageGroupId (and MessageDeduplicationId unless content-based deduplication is enabled) and returns 200 with MessageId and SequenceNumber; messages in one MessageGroupId are then delivered in order.
Emulator: answers HTTP 400 with error code `NotImplemented` and "operation SendMessage to a FIFO queue is scaffolded but not implemented in this emulator". `NotImplemented` is not a modeled SendMessage error.
Why: no FIFO queue can be written to at all, so FIFO ordering guarantees — the only reason to choose a FIFO queue — are unavailable, and a client gets a non-modeled error code it cannot classify.
Fix: the FIFO branch of the SendMessage handler, services/service-com-amazonaws-sqs-amazonsqs/src/ops/send_message.rs, and the per-message-group ordering in SQS queue state.
Traces: events__lambda__sqs.sqs-fifo-lambda-message-group-order-preserved.

## Evidence
Cause 1: "Check timed out after 60.0s" after PutRule, lambda AddPermission for events.amazonaws.com, PutTargets (FailedEntryCount 0) and PutEvents (FailedEntryCount 0) all succeeded; the function's recorded effect never appears. Expected: the function invoked once per matched event.
Cause 2: "Check timed out after 60.0s" on both checks after CreateEventSourceMapping succeeded and messages were sent to the queue; no invocation and no message consumption is observable. Expected: a Records batch delivered, non-failed messages deleted, and messages named in batchItemFailures redelivered.
Cause 3: "botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the SendMessage operation: operation SendMessage to a FIFO queue is scaffolded but not implemented in this emulator". Expected: HTTP 200 with MessageId and SequenceNumber.

## Steps to Reproduce
1. CreateFunction(fn); lambda AddPermission(fn, Principal='events.amazonaws.com', Action='lambda:InvokeFunction', SourceArn=<rule arn>); PutRule(r, EventPattern); PutTargets(Rule=r, Targets=[{Id:'t1', Arn:<fn arn>}]); PutEvents a matching event -> the function is never invoked.
2. CreateQueue(q); CreateEventSourceMapping(EventSourceArn=<q arn>, FunctionName=fn, FunctionResponseTypes=['ReportBatchItemFailures']); SendMessage a batch to q -> no invocation; nothing consumed; no redelivery of reported failures.
3. CreateQueue(QueueName='x.fifo', Attributes={'FifoQueue':'true'}); SendMessage(QueueUrl=<fifo url>, MessageBody='m', MessageGroupId='g1', MessageDeduplicationId='d1') -> 400 NotImplemented.
