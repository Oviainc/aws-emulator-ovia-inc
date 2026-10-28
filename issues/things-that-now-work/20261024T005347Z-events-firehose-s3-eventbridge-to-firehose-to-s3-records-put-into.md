Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__firehose__s3.firehose-buffering-hints-batch-delivery-to-s3 events__firehose__s3.firehose-s3-destination-wrong-bucket-arn-rejected
Issue-Key: events__firehose__s3.firehose-buffering-hints-batch-delivery-to-s3|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Key: events__firehose__s3.firehose-s3-destination-wrong-bucket-arn-rejected|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Commit: 4affd7a378830b3c6d8042007351e66db953803d

# EventBridge-to-Firehose-to-S3: records put into a delivery stream never become S3 objects, so neither batched delivery nor the wrong-BucketARN differential can be observed

> _Note: this issue was identified against the code base at commit `4affd7a378830b3c6d8042007351e66db953803d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRecord

## Description
One net-new root cause behind the events__firehose__s3 failures that are not OWNED by 20261022T194351Z-events-firehose-s3-eventbridge-firehose-s3-eventing-eventbridge-tagresource-and-s3.md (that issue covers EventBridge TagResource and S3 PutBucketReplication 501 stubs and DLQ routing; the two checks below never reach those surfaces).

Cause 1 of 1: the Firehose destination-delivery path does not exist, so no object is ever written to the destination bucket and both checks hang on their terminal destination read.
AWS: Smithy Firehose PutRecord doc: "Firehose buffers records before delivering them to the destination"; BufferingHints.SizeInMBs (1..128, default 5) and IntervalInSeconds (0..900, default 300) are "Buffer incoming data ... before delivering it to the destination"; S3DestinationConfiguration/ExtendedS3DestinationConfiguration BucketARN + Prefix produce objects keyed under the configured prefix plus the automatic "YYYY/MM/DD/HH" segment. So records put seconds apart inside one buffer window land batched in a small number of objects, and a stream whose destination was left unchanged after a refused UpdateDestination keeps delivering to its original bucket.
Emulator: PutRecord/PutRecordBatch answer 200 with a RecordId and append the data to an in-process buffer only (services/service-com-amazonaws-firehose-firehose-20150804/src/ops/put_record.rs -> state::ingest_record; the buffer in src/state.rs is drained only by the 24 h RECORD_RETENTION expiry). The crate declares no delivery component (src/lib.rs: ops, scaffolded, service, state) and never writes to S3, so list_objects_v2 on the destination bucket returns nothing and the checks run out their budget ("Error: Check timed out after 60.0s").
Why: an eventing pipeline that routes EventBridge events through Firehose to S3 is a silent no-op: producers see 200s, the archive stays empty, and the "destination left unchanged, still delivering to the original bucket" invariant that guards a rejected UpdateDestination cannot be verified at all.
Fix: the Firehose destination-delivery (flush) component — buffered records must be materialised into the configured destination through the S3 service's state, honouring BufferingHints and Prefix; the records already sit in services/service-com-amazonaws-firehose-firehose-20150804/src/state.rs with nothing consuming them.
Traces: events__firehose__s3.firehose-buffering-hints-batch-delivery-to-s3, events__firehose__s3.firehose-s3-destination-wrong-bucket-arn-rejected.

## Evidence
Both checks: "Error: Check timed out after 60.0s".
Expected (real AWS): events__firehose__s3.firehose-buffering-hints-batch-delivery-to-s3 — several PutRecord calls inside one 60 s buffer window appear batched in objects under the configured Prefix; events__firehose__s3.firehose-s3-destination-wrong-bucket-arn-rejected — after UpdateDestination with a BucketARN naming a non-existent bucket is refused, the stream keeps delivering to the original bucket, proven by reading the record back out of it.
Actual (emulator): put_record returns 200 with a RecordId, DescribeDeliveryStream reports ACTIVE, and the destination bucket never receives any object, so the delivery barrier both checks grade against never arrives.

## Steps to Reproduce
1. Boot the emulator; create an S3 bucket, a CloudWatch Logs log group, and an IAM role trusted by firehose.amazonaws.com with s3 write and logs:PutLogEvents.
2. firehose.create_delivery_stream(DeliveryStreamName=s, S3DestinationConfiguration={'BucketARN': 'arn:aws:s3:::'+bucket, 'RoleARN': role, 'Prefix': 'data/', 'BufferingHints': {'SizeInMBs': 1, 'IntervalInSeconds': 60}, 'CloudWatchLoggingOptions': {...}}); wait for ACTIVE.
3. firehose.put_record(...) a few times inside one buffer window.
4. Poll s3.list_objects_v2(Bucket=bucket, Prefix='data/') for several minutes: expect batched objects containing the record bytes; observe the bucket stays empty.
5. Re-replay the check ids in trace_ids.
