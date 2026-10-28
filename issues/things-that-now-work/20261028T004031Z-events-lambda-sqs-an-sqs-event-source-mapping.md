Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__sqs.sqs-lambda-function-error-redrive-to-dlq
Issue-Key: events__lambda__sqs.sqs-lambda-function-error-redrive-to-dlq|_poll_until#20|Timeout|time.sleep(interval_s)
Issue-Commit: 14860775d16e18fa487d5e79f3258080516d0017

# events__lambda__sqs: an SQS event-source mapping never consumes, so a throwing handler never raises ApproximateReceiveCount and SQS never redrives the message to the DLQ

> _Note: this issue was identified against the code base at commit `14860775d16e18fa487d5e79f3258080516d0017`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Cause 1 of 1: the SQS -> Lambda event-source-mapping poller never receives from the source queue, so the SQS-side redrive state machine (maxReceiveCount -> dead-letter queue) never advances and the check is killed at the suite's 60 s cap.

AWS: Smithy (service-com-amazonaws-lambda-awsgirapiservice): CreateEventSourceMapping POST /2015-03-31/event-source-mappings, response code 202, output EventSourceMappingConfiguration with State/StateTransitionReason/LastProcessingResult; input EventSourceArn (Arn), FunctionName (required), BatchSize @range 1..10000. Smithy (service-com-amazonaws-sqs-amazonsqs): QueueAttributeName includes RedrivePolicy and VisibilityTimeout; MessageSystemAttributeName includes ApproximateReceiveCount. On real AWS, once the mapping reports State=Enabled, Lambda's poller receives the queued message and invokes the function. When the handler raises an unhandled exception and FunctionResponseTypes does not include ReportBatchItemFailures, the mapping does not delete the batch, so the message becomes visible again after the queue's VisibilityTimeout and its ApproximateReceiveCount rises on each receive. SQS itself then applies the source queue's RedrivePolicy: when the receive count exceeds maxReceiveCount (1 here) the message is moved to deadLetterTargetArn with its body byte-for-byte unchanged, and the source queue's ApproximateNumberOfMessages returns to 0. The rule, not one run's numbers, is the requirement: delivery count > maxReceiveCount => move to the DLQ, body preserved, source drained.

Emulator: nothing in that chain happens. The check's control-plane calls all succeed (both queues are created with VisibilityTimeout=5 and RedrivePolicy maxReceiveCount=1 and read back correctly, the function reaches State=Active with Timeout=3, and CreateEventSourceMapping returns a mapping that polls to State=Enabled), but after SendMessage the DLQ is never populated: the replay reports `Error: Check timed out after 60.0s` while the check is polling ReceiveMessage on the dead-letter queue. No invocation is ever made, so the message is never received by the mapping, ApproximateReceiveCount never increases, maxReceiveCount is never exceeded and the message is never redriven. There is no error code and no non-2xx response to quote — every individual API call is accepted; the divergence is the absent delivery/redrive progress.

Why: a client cannot verify the failure path of an SQS-triggered function at all. The guarantee that a throwing handler leaves its batch on the queue, that repeated delivery is counted, and that the queue's redrive policy eventually parks the poison message in the DLQ with its original body, is the entire basis of poison-message handling; here every such flow silently makes no progress instead of either working or refusing, which reads to a client as a stuck queue rather than as an unimplemented feature.

Fix: Lambda's event-source-mapping poller and delivery path in the lambda crate (services/service-com-amazonaws-lambda-awsgirapiservice) — the component behind CreateEventSourceMapping/UpdateEventSourceMapping that must receive from the SQS emulator's queue store, batch by BatchSize, invoke the mapping's function, and leave the batch undeleted when the invocation fails — plus the SQS crate's receive-count/redrive handling (services/service-com-amazonaws-sqs-amazonsqs) so that a message whose receive count exceeds the source queue's RedrivePolicy maxReceiveCount is moved to deadLetterTargetArn and removed from the source queue. Note the known blocker: deliveries here have to reach a customer handler, and Invoke of a real deployment package is itself unimplemented in this emulator.

Related but NOT covering: open issue 20261027T180000Z-lambda-an-sqs-event-source-mapping-never-consumes-or-delivers.md describes the same poller gap for whole-batch/partial-batch redelivery and the Enabled/Disabled gate, and 20261025T002357Z-events-lambda-sqs-eventbridge-lambda-targets-and-sqs-event-source-mappings.md and 20261026T094219Z-dynamodb-lambda-sqs-an-sqs-event-source-mapping.md cover adjacent EventBridge-target and DLQ/permission flows; none of them carries this check's replay key, and none covers the SQS-side maxReceiveCount -> DLQ transition asserted here.

Traces: events__lambda__sqs.sqs-lambda-function-error-redrive-to-dlq.

## Evidence
From the simparity-behavioral-ga-prod-v1 replay against the emulator built from this checkout:

```
#### [mismatch] events__lambda__sqs.sqs-lambda-function-error-redrive-to-dlq
Error: Check timed out after 60.0s
```

Everything before the delivery succeeds: GetQueueAttributes reads VisibilityTimeout=5 on the source queue, GetFunctionConfiguration reads Timeout=3, GetEventSourceMapping reaches State="Enabled", and SendMessage(MessageBody="always-fails") is accepted.

Expected (real AWS): within a few visibility windows the handler's exception leaves the batch on the queue, the receive count exceeds the RedrivePolicy maxReceiveCount of 1, SQS moves the message to the dead-letter queue, ReceiveMessage on the DLQ returns one message whose Body is exactly the body that was sent, and GetQueueAttributes(ApproximateNumberOfMessages) on the source queue reads "0".

Actual (emulator): the dead-letter queue stays empty for the whole run; ReceiveMessage on it never returns a message, so the check never reaches its body/drain assertions and is killed by the harness at 60 s. No invocation and no receive-count increase is observable at any point.

## Steps to Reproduce
1. CreateQueue a DLQ, then CreateQueue a source queue with Attributes {VisibilityTimeout: "5", RedrivePolicy: {"deadLetterTargetArn": <dlq arn>, "maxReceiveCount": 1}}.
2. CreateRole trusting lambda.amazonaws.com with sqs:ReceiveMessage/DeleteMessage/GetQueueAttributes on the source queue, and CreateFunction (python3.12, Timeout=3) whose handler raises an unhandled exception; wait for State=Active.
3. CreateEventSourceMapping(EventSourceArn=<source queue arn>, FunctionName=<fn>, BatchSize=10) and poll GetEventSourceMapping until State=Enabled (this succeeds).
4. SendMessage(QueueUrl=<source queue>, MessageBody="always-fails").
5. Poll ReceiveMessage(QueueUrl=<dlq>, WaitTimeSeconds=5): expect one message whose Body is the body sent in step 4 — the DLQ stays empty.
6. Poll GetQueueAttributes(QueueUrl=<source queue>, AttributeNames=[ApproximateNumberOfMessages]): expect "0" after the redrive — it never drains.

Replay: events__lambda__sqs.sqs-lambda-function-error-redrive-to-dlq.
