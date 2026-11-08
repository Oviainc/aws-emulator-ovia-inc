Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose__glue__kinesis__s3.kds-one-kinesis-stream-feeds-two-delivery-streams-each-with-every-record
Issue-Key: firehose__glue__kinesis__s3.kds-one-kinesis-stream-feeds-two-delivery-streams-each-with-every-record|run#14|Timeout|put = kin.put_records(StreamName=stream_name, Records=[
Issue-Commit: 8f7c6ce558c7d939146a7c42cc95df5134e5d406

# One Kinesis stream feeding two Firehose delivery streams delivers to neither: the fan-out never lands any object

> _Note: this issue was identified against the code base at commit `8f7c6ce558c7d939146a7c42cc95df5134e5d406`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRecord

## Description
One cause.

Cause 1 of 1: with two KinesisStreamAsSource delivery streams attached to the same Kinesis data stream, records put to that stream are never delivered to either destination, so the check never observes the expected objects and is killed at its 180 s budget.
Failing check ids: firehose__glue__kinesis__s3.kds-one-kinesis-stream-feeds-two-delivery-streams-each-with-every-record

AWS: recorded behavior only (Firehose's delivery timing/fan-out is not expressed in the Smithy model). Recorded: each KinesisStreamAsSource delivery stream consumes the shared Kinesis stream independently, so every record put to the stream is delivered to every attached delivery stream's destination — each sink bucket ends up with objects under the delivery prefix whose decoded contents contain the full record set, not a partition of it. The bucket names, stream names and object keys are minted per run; the requirement is that both sinks receive every record.

Emulator: nothing is delivered. The check polls each sink bucket with ListObjectsV2 and finds no keys, and the run ends with "Check timed out after 180.0s" — no object ever appears for either delivery stream.

Why: a client building a Kinesis fan-out (one stream, several Firehose consumers writing to different S3 prefixes/buckets — the standard raw-plus-curated pattern) sees its data silently disappear: PutRecord succeeds, the delivery streams exist and report healthy, and no object is ever written. Any pipeline or test that waits for the delivered objects hangs rather than failing with a diagnosable error.

Fix: the Firehose delivery engine's Kinesis source consumption and S3 delivery path — the component that reads the configured KinesisStreamSourceConfiguration stream and buffers/flushes records to the S3 destination. It must run for every delivery stream attached to a stream, independently, so each sink receives the full record set.

Traces: firehose__glue__kinesis__s3.kds-one-kinesis-stream-feeds-two-delivery-streams-each-with-every-record. Open issues 20261102T100157Z-firehose-glue-kinesis-s3-5-net-new-root.md and 20261028T122732Z-firehose-glue-kinesis-s3-the-apache-iceberg-path.md cover other causes in this suite and do not carry this check's replay key.

## Evidence
Check: firehose__glue__kinesis__s3.kds-one-kinesis-stream-feeds-two-delivery-streams-each-with-every-record

Actual (emulator): "Error: Check timed out after 180.0s" — the check's polling helper (ListObjectsV2 over each sink bucket/prefix, retried every 3 s) never satisfied its predicate, so no delivered object existed for either delivery stream inside the budget.

Expected (AWS): both KinesisStreamAsSource delivery streams attached to the one Kinesis stream deliver, and each sink bucket holds object(s) under its delivery prefix whose decoded bodies contain every record put to the stream.

## Steps to Reproduce
1. CreateStream on Kinesis; create two S3 sink buckets.
2. CreateDeliveryStream twice with DeliveryStreamType=KinesisStreamAsSource, both naming the same KinesisStreamSourceConfiguration.KinesisStreamARN, each with its own S3 destination bucket/prefix and a role that can read the stream and write the bucket.
3. Wait for both delivery streams to report ACTIVE.
4. PutRecord(s) to the Kinesis stream.
5. Poll ListObjectsV2 on each sink bucket under its delivery prefix: against AWS objects appear in both and together contain every record; in the emulator neither bucket ever receives a key and the check is cut off at its 180 s budget.
