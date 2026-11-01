Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__kinesis__lambda.xsvc-lambda-2-ddb-stream-esm-on-failure-sqs-receives-batch-info-pointer dynamodb__kinesis__lambda.xsvc-lambda-2-kinesis-esm-on-failure-sqs-receives-batch-info-pointer
Issue-Key: dynamodb__kinesis__lambda.xsvc-lambda-2-ddb-stream-esm-on-failure-sqs-receives-batch-info-pointer|_probe#12|Timeout|return got
Issue-Key: dynamodb__kinesis__lambda.xsvc-lambda-2-kinesis-esm-on-failure-sqs-receives-batch-info-pointer|_probe#16|Timeout|return got
Issue-Commit: 62e4c5cd3531cb5ea16934ee4e8b2e43c8dd65a3

# dynamodb__kinesis__lambda: an event source mapping's OnFailure destination never receives the batch-info pointer message, for either a DynamoDB-Streams or a Kinesis source

> _Note: this issue was identified against the code base at commit `62e4c5cd3531cb5ea16934ee4e8b2e43c8dd65a3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Two distinct causes, one per stream source, in the Lambda event-source-mapping failure-destination path. Both end in `Error: Check timed out after 60.0s`.

Cause 1 of 2 — DynamoDB Streams OnFailure destination: AWS: recorded behavior only (the Smithy model carries DestinationConfig.OnFailure.Destination on CreateEventSourceMapping but no delivery semantics). When a DynamoDB-Streams ESM configured with DestinationConfig.OnFailure pointing at an SQS queue exhausts its retries on a poison batch, AWS sends exactly one message to that queue carrying the batch-info pointer envelope (the failure record naming the function ARN, the stream ARN, the shard id and the batch's start/end sequence numbers and record count) rather than the record payloads.
Emulator: no message ever arrives on the destination queue; the check polls for it and is killed at the 60s budget.
Failing check: dynamodb__kinesis__lambda.xsvc-lambda-2-ddb-stream-esm-on-failure-sqs-receives-batch-info-pointer.

Cause 2 of 2 — Kinesis OnFailure destination: AWS: recorded behavior only. The same contract holds for a Kinesis-stream ESM: after retry exhaustion the OnFailure SQS destination receives one batch-info pointer message describing the failed batch (stream ARN, shard id, sequence-number window, record count), and a healthy batch sends nothing.
Emulator: nothing is delivered to the destination; the check times out at 60s.
Failing check: dynamodb__kinesis__lambda.xsvc-lambda-2-kinesis-esm-on-failure-sqs-receives-batch-info-pointer.

Why: the OnFailure destination is the only durable record of a poison batch for stream-based ESMs. With nothing delivered, a client cannot tell a batch that failed from one that was never polled, loses the sequence-number window needed to replay it, and any operational workflow keyed off those pointer messages never fires.

Fix: the event-source-mapping poller's failure path — retry exhaustion must emit the batch-info pointer envelope to the configured DestinationConfig.OnFailure target (SQS SendMessage) for both the DynamoDB-Streams and Kinesis source types.

Traces: the two check ids listed above.

## Evidence
Both failures report `Error: Check timed out after 60.0s` (replay verdicts [mismatch]). Expected: exactly one SQS message on the OnFailure destination per exhausted batch, carrying the batch-info pointer fields (function ARN, source stream ARN, shard id, start/end sequence numbers, record count) — and no message for a healthy batch. Actual: the destination queue stays empty for the whole 60s budget, for both the DynamoDB-Streams and the Kinesis mapping.

## Steps to Reproduce
1. Create a Lambda whose handler raises for a marked record.
2. CreateEventSourceMapping from a DynamoDB table stream with MaximumRetryAttempts small and DestinationConfig={'OnFailure': {'Destination': '<sqs-queue-arn>'}}.
3. Write the poison item and poll the destination queue: AWS delivers one batch-info pointer message after retries are exhausted.
4. Repeat with a Kinesis stream source and a poison record.
5. Against the emulator neither destination queue ever receives a message; both checks are killed at the 60s harness budget.
