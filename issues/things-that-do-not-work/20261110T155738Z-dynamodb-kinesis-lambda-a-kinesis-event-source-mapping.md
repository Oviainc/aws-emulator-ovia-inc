Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__kinesis__lambda.kinesis-esm-through-enhanced-fan-out-consumer-delivers-each-record
Issue-Key: dynamodb__kinesis__lambda.kinesis-esm-through-enhanced-fan-out-consumer-delivers-each-record|_poll_until#17|Timeout|time.sleep(interval_s)
Issue-Commit: b11785bf9bf8af57a00ad7e4d814b1c5f1d66117

# dynamodb__kinesis__lambda: a Kinesis event source mapping through an enhanced fan-out consumer delivers no records to the function

> _Note: this issue was identified against the code base at commit `b11785bf9bf8af57a00ad7e4d814b1c5f1d66117`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Cause 1 of 1: an event source mapping whose source is a Kinesis enhanced fan-out consumer (a registered StreamConsumer ARN rather than the stream ARN) never invokes the function with the stream's records.

AWS: RegisterStreamConsumer creates a consumer that goes ACTIVE; CreateEventSourceMapping with that consumer ARN as EventSourceArn reaches State `Enabled`, and every record put to the stream is delivered to the function exactly once as a `aws:kinesis` event record (kinesis.data base64-encoded, partitionKey, sequenceNumber, eventSourceARN naming the consumer), observable through the handler's side effect within seconds. Smithy: Lambda models CreateEventSourceMapping (EventSourceArn, StartingPosition, BatchSize) and Kinesis models RegisterStreamConsumer / DescribeStreamConsumer / SubscribeToShard; that records are delivered through the consumer is AWS documented behavior.

Emulator: the check ends with `Check timed out after 60.0s` — the function is never invoked for the records put to the stream, so there is no event payload to compare. Nothing is refused: the mapping is accepted and reports healthy while the delivery path is inert. Hypothesis: the enhanced-fan-out read path (SubscribeToShard / consumer-based polling) is not wired to the ESM poller, so no batch is ever assembled.

Why: a client that uses enhanced fan-out — the recommended way to add a dedicated-throughput consumer — gets a mapping that looks enabled and a function that never runs. The pipeline fails silently, with no error to diagnose.

Fix: the event-source-mapping poller for Kinesis in the lambda service crate together with the consumer (enhanced fan-out) read path in the kinesis service crate, so an ESM whose EventSourceArn is a StreamConsumer ARN assembles and delivers batches to the function. Open issue 20261109T172142Z-dynamodb-kinesis-lambda-kinesis-event-source-mapping-through-an-enhanced-fan.md describes the same symptom but does not carry this check's replay key; this filing records it.

Traces: dynamodb__kinesis__lambda.kinesis-esm-through-enhanced-fan-out-consumer-delivers-each-record.

## Evidence
Expected: with a RegisterStreamConsumer consumer ACTIVE and an ESM on the consumer ARN reporting State Enabled, each record put to the stream is delivered to the function once (observable through the handler's DynamoDB writes).
Actual: `Error: Check timed out after 60.0s` — the function records no invocation for any of the records put.

## Steps to Reproduce
1. CreateStream; RegisterStreamConsumer on it and wait for ConsumerStatus ACTIVE.
2. Create a Lambda function that records each received record, and CreateEventSourceMapping(EventSourceArn=<consumer ARN>, StartingPosition=TRIM_HORIZON); wait for State Enabled.
3. PutRecord(s) to the stream and poll the function's side-effect table — nothing is ever recorded.
Replay dynamodb__kinesis__lambda.kinesis-esm-through-enhanced-fan-out-consumer-delivers-each-record.
