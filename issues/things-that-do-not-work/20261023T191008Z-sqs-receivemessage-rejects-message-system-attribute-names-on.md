Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: scheduler.schedule-flexible-time-window-invocation-within-bounds scheduler.server-side-flexible-time-window-jitter-within-bounds scheduler.server-side-rate-expression-cadence-enforced lambda__scheduler.scheduler-lambda-flexible-time-window-invocation
Issue-Key: sqs|ReceiveMessage|InvalidAttributeName
Issue-Commit: 3dc196f76d19116b5d570e1dc438e2b93d0e0d47

# SQS ReceiveMessage rejects message system attribute names on the deprecated AttributeNames member (InvalidAttributeName for SentTimestamp)

> _Note: this issue was identified against the code base at commit `3dc196f76d19116b5d570e1dc438e2b93d0e0d47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ReceiveMessage

## Description
Single root cause (1 of 1): ReceiveMessage validates `AttributeNames` against the wrong value set, so the legacy member no longer accepts message system attribute names.

AWS: `AttributeNames` is the deprecated alias of `MessageSystemAttributeNames` and accepts the same values, so ReceiveMessage(AttributeNames=['SentTimestamp']) succeeds and returns the attribute in each message's Attributes map. Smithy (sqs model, ReceiveMessage): `AttributeNames` carries @deprecated{message: "AttributeNames has been replaced by MessageSystemAttributeNames"}, and `MessageSystemAttributeName` enumerates All | SenderId | SentTimestamp | ApproximateReceiveCount | ApproximateFirstReceiveTimestamp | SequenceNumber | MessageDeduplicationId | MessageGroupId | AWSTraceHeader | DeadLetterQueueSourceArn. (The model targets AttributeNames at QueueAttributeName, whose value list omits the message system attributes — validating against that list is what produces the refusal, but AWS accepts them on this member regardless of the model's shape target, which is why it is documented as the replaced alias of the system-attribute member.)

Emulator: the call is refused with error code InvalidAttributeName, message 'Unknown Attribute SentTimestamp.' (HTTP 400), so the receive never happens.

Why: every boto3/SDK caller written against the long-standing API — including anything that measures message age or ordering from SentTimestamp — fails outright on receive rather than getting its messages. The failure is at the call, not just a missing attribute, so the consumer loop cannot proceed at all.

Fix: the attribute-name validation in ReceiveMessage's handler (services/service-com-amazonaws-sqs-amazonsqs/src/ops/receive_message.rs) must accept the MessageSystemAttributeName value set on `AttributeNames` (treating it as the alias of `MessageSystemAttributeNames`) and populate the returned Attributes map accordingly.

Traces: the four failing checks in trace_ids, which all die on the first receive_message(AttributeNames=['SentTimestamp']) while observing scheduler deliveries. (Their service token could not be taken from the check id — the ids are multi-service, `scheduler.*` and `lambda__scheduler.*` — so this issue is filed under sqs, the service whose operation diverges.)

## Evidence
Expected: ReceiveMessage(QueueUrl=..., AttributeNames=['SentTimestamp']) returns 200 with each message's Attributes containing SentTimestamp.
Actual: botocore.errorfactory.InvalidAttributeName: An error occurred (InvalidAttributeName) when calling the ReceiveMessage operation: Unknown Attribute SentTimestamp.
Reproduced identically in 4 checks (scheduler.schedule-flexible-time-window-invocation-within-bounds, scheduler.server-side-flexible-time-window-jitter-within-bounds, scheduler.server-side-rate-expression-cadence-enforced, lambda__scheduler.scheduler-lambda-flexible-time-window-invocation).

## Steps to Reproduce
1. url = sqs.create_queue(QueueName='q')['QueueUrl'].
2. sqs.send_message(QueueUrl=url, MessageBody='x').
3. sqs.receive_message(QueueUrl=url, AttributeNames=['SentTimestamp'], MaxNumberOfMessages=1) -> InvalidAttributeName 'Unknown Attribute SentTimestamp.' instead of a 200 whose message carries Attributes['SentTimestamp'].
