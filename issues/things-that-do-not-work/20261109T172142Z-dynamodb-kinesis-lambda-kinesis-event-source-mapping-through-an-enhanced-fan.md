Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__kinesis__lambda.kinesis-esm-through-enhanced-fan-out-consumer-delivers-each-record
Issue-Key: dynamodb__kinesis__lambda.kinesis-esm-through-enhanced-fan-out-consumer-delivers-each-record|_poll_until#17|AssertionError|raise AssertionError(
Issue-Commit: 545841f07feb57b92e392d55db412e521f655a7b

# Kinesis event source mapping through an enhanced fan-out consumer never delivers records to the function

> _Note: this issue was identified against the code base at commit `545841f07feb57b92e392d55db412e521f655a7b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Cause 1 of 1: an ESM whose EventSourceArn is a registered stream consumer (enhanced fan-out) never polls.

AWS: `RegisterStreamConsumer` creates a consumer ARN (`arn:aws:kinesis:<region>:<account>:stream/<stream>/consumer/<name>:<ts>`); a Lambda event source mapping created with that consumer ARN as `EventSourceArn` subscribes via `SubscribeToShard` and delivers every record put to the stream to the function exactly as a shared-throughput mapping does. Smithy (lambda): CreateEventSourceMapping input EventSourceArn, StartingPosition; (kinesis): RegisterStreamConsumer -> Consumer{ConsumerARN, ConsumerStatus}, SubscribeToShard (event stream).

Emulator: records put to the stream never reach the function through the consumer-ARN mapping. The function wrote 0 of the 3 expected stream-derived records within 120s (`Last result: []`), i.e. the enhanced fan-out path of the ESM poller delivers nothing.

Why: enhanced fan-out is the recommended way to run multiple independent consumers on a stream. A mapping created against a consumer ARN reports healthy and silently processes nothing, so the pipeline loses every record.

Fix: the event-source-mapping poller's handling of a consumer ARN EventSourceArn (SubscribeToShard-based delivery) in the Lambda ESM component, and Kinesis's stream-consumer subscription.

Traces: dynamodb__kinesis__lambda.kinesis-esm-through-enhanced-fan-out-consumer-delivers-each-record.

## Evidence
Expected: each record put to the stream is delivered through the enhanced fan-out consumer to the function, which writes 3 records.
Actual: AssertionError: 3 stream records written by the function not met within 120s. Last result: []

## Steps to Reproduce
1. Create a Kinesis stream and kinesis:RegisterStreamConsumer on it.
2. lambda:CreateEventSourceMapping with EventSourceArn set to the consumer ARN, StartingPosition TRIM_HORIZON.
3. kinesis:PutRecord(s).
4. Observe the function is never invoked and no records are processed.
