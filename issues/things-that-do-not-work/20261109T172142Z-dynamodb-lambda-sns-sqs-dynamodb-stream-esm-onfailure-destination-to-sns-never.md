Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__sns__sqs.ddb-stream-esm-sns-on-failure-fans-out-batch-info-to-an-sqs-subscriber
Issue-Key: dynamodb__lambda__sns__sqs.ddb-stream-esm-sns-on-failure-fans-out-batch-info-to-an-sqs-subscriber|_poll_until#12|TimeoutError|raise TimeoutError(f"{description} not observed within {timeout_s:.0f}s; last saw {last!r}")
Issue-Commit: 545841f07feb57b92e392d55db412e521f655a7b

# DynamoDB-stream ESM OnFailure destination to SNS never publishes, so the SQS subscriber receives no batch-info record

> _Note: this issue was identified against the code base at commit `545841f07feb57b92e392d55db412e521f655a7b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Cause 1 of 1: the ESM's OnFailure destination is never written when the destination is an SNS topic.

AWS: a stream event source mapping with `DestinationConfig.OnFailure.Destination` set to an SNS topic ARN publishes, after the retry policy is exhausted for a poison batch, a batch-info record (`requestContext` with the function/condition/approximateInvokeCount, and `DDBStreamBatchInfo` with shardId, startSequenceNumber, endSequenceNumber, batchSize and streamArn) to that topic; the topic then fans it out to every subscriber, so an SQS queue subscribed to the topic receives the message. Smithy (lambda): CreateEventSourceMapping input DestinationConfig -> OnFailure{Destination}, MaximumRetryAttempts.

Emulator: no OnFailure record is ever observed on the subscribed queue — the poll ran out at 49s having seen `[]`. Either the destination publish never happens or it is not routed through SNS to the subscription.

Why: the OnFailure destination is the only record that a batch was dropped. With nothing published, poison batches disappear silently and no dead-letter processing ever runs.

Fix: the event-source-mapping failure-destination dispatcher (SNS destination support) in the Lambda ESM component, plus SNS->SQS subscription delivery for that publish.

Traces: dynamodb__lambda__sns__sqs.ddb-stream-esm-sns-on-failure-fans-out-batch-info-to-an-sqs-subscriber.

## Evidence
Expected: after retries are exhausted on the poisoned record, the SNS topic receives the ESM batch-info record and the subscribed SQS queue delivers it.
Actual: TimeoutError: the SNS-fanned-out OnFailure record for the poisoned record not observed within 49s; last saw []

## Steps to Reproduce
1. Create a table with a stream, a function that always raises, an SNS topic and an SQS queue subscribed to the topic.
2. lambda:CreateEventSourceMapping with DestinationConfig.OnFailure.Destination = the topic ARN and a small MaximumRetryAttempts.
3. PutItem to generate a record.
4. Poll the queue: no batch-info record arrives.
