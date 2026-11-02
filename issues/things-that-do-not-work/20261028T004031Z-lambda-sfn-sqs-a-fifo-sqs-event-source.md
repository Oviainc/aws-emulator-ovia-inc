Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sqs__stepfunctions.sqs-esm-fifo-queue-message-group-ordering-preserved
Issue-Key: lambda__sqs__stepfunctions.sqs-esm-fifo-queue-message-group-ordering-preserved|run#13|Timeout|resp = sqs.receive_message(QueueUrl=results["url"], MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Commit: 14860775d16e18fa487d5e79f3258080516d0017

# lambda__sfn__sqs: a FIFO SQS event-source mapping never delivers, so per-MessageGroupId ordering and strict serial processing within a group are unobservable

> _Note: this issue was identified against the code base at commit `14860775d16e18fa487d5e79f3258080516d0017`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Cause 1 of 1: an event-source mapping on a FIFO queue reports State=Enabled but never consumes the queued messages, so the FIFO contract it exists to provide (per-message-group order, one in-flight message per group) produces no observable result and the check is killed at the suite's 60 s cap.

AWS: Smithy (service-com-amazonaws-lambda-awsgirapiservice): CreateEventSourceMapping POST /2015-03-31/event-source-mappings, code 202, output EventSourceMappingConfiguration with State and LastProcessingResult; BatchSize @range 1..10000. Smithy (service-com-amazonaws-sqs-amazonsqs): QueueAttributeName includes FifoQueue and ContentBasedDeduplication; MessageSystemAttributeName includes MessageGroupId, which is the attribute the SQS event record carries to the handler. On real AWS, once the mapping is Enabled, every message sent to the FIFO source is delivered at least once; records are delivered in send order within each MessageGroupId; and only one batch per message group is in flight at a time, so processing of message i in a group completes before processing of message i+1 in that group begins. Different message groups progress independently/concurrently. The ordering and one-batch-per-group rules are the requirement, not any particular timestamp from one run.

Emulator: nothing is delivered. The control plane is accepted end to end — CreateQueue with FifoQueue=true/ContentBasedDeduplication=true, CreateFunction reaching State=Active with the RESULTS_QUEUE_URL environment variable, CreateEventSourceMapping(BatchSize=1) polling to State=Enabled, and six SendMessage calls across two MessageGroupIds — but the handler is never invoked, so the results queue stays empty and the replay reports `Error: Check timed out after 60.0s` while the check is polling ReceiveMessage on the results queue. There is no error code or non-2xx to quote: every call is accepted and the divergence is the absent delivery, which leaves the ordering and serialization assertions unreachable.

Why: a client gets silence where AWS gives ordered delivery. FIFO ordering and the one-in-flight-batch-per-group rule are the only reasons to choose a FIFO source, and here they can be neither relied on nor detected as missing: the queue simply never drains, which a client reads as a stalled consumer rather than as an unimplemented poller. Any ordering-sensitive design validated against this emulator is validated against nothing.

Fix: Lambda's event-source-mapping poller and delivery path in the lambda crate (services/service-com-amazonaws-lambda-awsgirapiservice) — the component behind CreateEventSourceMapping/UpdateEventSourceMapping — must receive from the SQS emulator's queue store for a FIFO source, group by MessageGroupId, batch by BatchSize/MaximumBatchingWindowInSeconds, invoke the function with the SQS event shape (including the MessageGroupId system attribute on each record), and keep at most one batch per message group in flight until it is resolved, deleting a successful batch and returning a failed one for redelivery. The known blocker applies: a delivery has to reach a customer handler, and Invoke of a real deployment package is itself unimplemented in this emulator.

Related but NOT covering: open issue 20261027T180000Z-lambda-an-sqs-event-source-mapping-never-consumes-or-delivers.md covers the same poller gap for whole-batch/partial-batch redelivery and the Enabled=False gate (including lambda__sfn__sqs.sqs-esm-disabled-mapping-no-invocation-negative), and 20261025T022656Z-lambda-s3-sqs-sqs-event-source-mappings-never.md covers BatchSize having no observable effect; neither carries this check's replay key, and neither covers FIFO per-group ordering or the one-batch-per-group serialization asserted here.

Traces: lambda__sfn__sqs.sqs-esm-fifo-queue-message-group-ordering-preserved.

## Evidence
From the simparity-behavioral-ga-prod-v1 replay against the emulator built from this checkout:

```
#### [mismatch] lambda__sfn__sqs.sqs-esm-fifo-queue-message-group-ordering-preserved
Error: Check timed out after 60.0s
```

Everything before the delivery succeeds: the FIFO source queue and the plain results queue are created and their QueueArn read back, the function reaches Configuration.State="Active", GetEventSourceMapping reaches State="Enabled" for a mapping created with BatchSize=1, and six SendMessage calls (bodies group-a-0..2 and group-b-0..2 across MessageGroupId group-a and group-b) are accepted.

Expected (real AWS): the handler is invoked once per record and writes one result message per record to the results queue; ReceiveMessage on the results queue yields all 6 entries, each carrying the record's MessageGroupId; within each group the bodies appear in send order (group-x-0, group-x-1, group-x-2) and each record's processing end timestamp is <= the next record's start timestamp in the same group.

Actual (emulator): the results queue is never written to — no invocation occurs — so fewer than 6 entries are collected, none of the ordering or serialization assertions is ever evaluated, and the harness kills the check at 60 s.

## Steps to Reproduce
1. CreateQueue a FIFO source queue (Attributes {FifoQueue: "true", ContentBasedDeduplication: "true"}) and a standard results queue.
2. CreateRole trusting lambda.amazonaws.com with AWSLambdaSQSQueueExecutionRole and AmazonSQSFullAccess attached; CreateFunction (python3.12, Timeout=30, Publish=true) whose handler sleeps briefly per record and then SendMessage's {body, MessageGroupId, start, end} to the results queue; wait for State=Active.
3. CreateEventSourceMapping(EventSourceArn=<fifo queue arn>, FunctionName=<fn>, BatchSize=1) and poll GetEventSourceMapping until State=Enabled (this succeeds).
4. SendMessage 3 messages for MessageGroupId=group-a and 3 for MessageGroupId=group-b, interleaved, with bodies <group>-0..<group>-2.
5. Poll ReceiveMessage on the results queue: expect 6 entries, in send order within each group, with each entry's end <= the next entry's start inside the same group — the results queue stays empty and nothing is ever delivered.

Replay: lambda__sfn__sqs.sqs-esm-fifo-queue-message-group-ordering-preserved.
