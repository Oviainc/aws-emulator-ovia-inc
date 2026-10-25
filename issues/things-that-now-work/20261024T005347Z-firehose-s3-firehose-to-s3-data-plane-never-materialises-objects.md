Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose__s3.firehose-destination-repointed-mid-stream-and-the-differential-barrier-proves-the-switch firehose__s3.firehose-dynamic-partitioning-places-records-in-jq-derived-prefixes firehose__s3.firehose-put-record-batch-above-five-hundred-records-rejected-and-none-delivered firehose__s3.firehose-record-above-the-per-record-ceiling-rejected-while-a-just-under-record-delivers
Issue-Key: firehose__s3.firehose-destination-repointed-mid-stream-and-the-differential-barrier-proves-the-switch|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Key: firehose__s3.firehose-dynamic-partitioning-places-records-in-jq-derived-prefixes|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Key: firehose__s3.firehose-put-record-batch-above-five-hundred-records-rejected-and-none-delivered|_poll_until#21|Timeout|time.sleep(interval_s)
Issue-Key: firehose__s3.firehose-record-above-the-per-record-ceiling-rejected-while-a-just-under-record-delivers|_poll_until#19|Timeout|time.sleep(interval_s)
Issue-Commit: 4affd7a378830b3c6d8042007351e66db953803d

# Firehose-to-S3 data plane never materialises objects: dynamic partitioning, mid-stream destination repointing and the reject-nothing-delivered differentials are all unobservable

> _Note: this issue was identified against the code base at commit `4affd7a378830b3c6d8042007351e66db953803d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRecordBatch

## Description
One net-new root cause behind the firehose__s3 failures.

Cause 1 of 1: buffered records are never written to the destination bucket, so every check that grades an outcome by reading S3 — including the ones whose API-level rejection assertions pass and that then need a positive delivery barrier to prove an absence is meaningful — cannot complete.
AWS: Smithy Firehose PutRecord doc: "Firehose buffers records before delivering them to the destination" and records are "stored for 24 hours ... as it tries to send the records to the destination"; BufferingHints.SizeInMBs (1..128, default 5) and IntervalInSeconds (0..900, default 300) decide when the buffer is flushed; ExtendedS3DestinationConfiguration.Prefix gets the automatic "YYYY/MM/DD/HH" expansion (or the jq-derived partition keys when DynamicPartitioningConfiguration is enabled, with the !{partitionKeyFromQuery:...} namespace in the prefix), ErrorOutputPrefix receives failed records, and CompressionFormat (UNCOMPRESSED|GZIP|ZIP|Snappy|HADOOP_SNAPPY) decides the body encoding. An UpdateDestination that repoints the BucketARN makes subsequent records land in the new bucket while the old one stops receiving. PutRecordBatch over the 500-record ceiling, and a record over the per-record ceiling, are refused and none of the refused records is delivered — while a sibling batch in the same buffer window does arrive.
Emulator: PutRecord/PutRecordBatch answer 200 with RecordIds and append the data to an in-process buffer only (services/service-com-amazonaws-firehose-firehose-20150804/src/ops/put_record.rs -> state::ingest_record; services/service-com-amazonaws-firehose-firehose-20150804/src/state.rs holds DeliveryStream.records / buffered_bytes and drops them only at the 24 h RECORD_RETENTION expiry inside materialise_due). The crate declares no delivery component at all (src/lib.rs: ops, scaffolded, service, state) and nothing writes into the S3 service's state, so list_objects_v2 on the destination bucket never returns anything and all four checks run out the suite budget ("Error: Check timed out after 60.0s").
Why: the primary Firehose data path is a silent drop — a producer is told 200/RecordId for data that never arrives. Dynamic partitioning prefixes, the per-record and per-batch ceiling enforcement ("rejected and none delivered"), and a mid-stream destination switch all become unverifiable, so client code and IaC validated against the emulator cannot be trusted against AWS.
Fix: the Firehose destination-delivery (flush) component in the Firehose service crate: materialise buffered records into the configured destination through the S3 service's state, driven by BufferingHints, applying Prefix/ErrorOutputPrefix expansion (including DynamicPartitioningConfiguration's jq-derived keys), CompressionFormat, and the destination currently in effect after an UpdateDestination.
Traces: firehose__s3.firehose-destination-repointed-mid-stream-and-the-differential-barrier-proves-the-switch, firehose__s3.firehose-dynamic-partitioning-places-records-in-jq-derived-prefixes, firehose__s3.firehose-put-record-batch-above-five-hundred-records-rejected-and-none-delivered, firehose__s3.firehose-record-above-the-per-record-ceiling-rejected-while-a-just-under-record-delivers.

## Evidence
All four checks: "Error: Check timed out after 60.0s".
Expected (real AWS): a 500-record PutRecordBatch issued into the same 60 s buffer window as a refused 501-record batch has all 500 of its markers readable out of the destination bucket (and none of the refused batch's markers); a just-under-ceiling record arrives while the over-ceiling one does not; records with distinct jq partition values appear under distinct derived prefixes; after UpdateDestination repoints the bucket, later records appear only in the new bucket.
Actual (emulator): the put calls return 200 with RecordIds (e.g. 'FailedPutCount': 0 with a RecordId per record), DescribeDeliveryStream reports ACTIVE, and no object is ever written to any prefix of any destination bucket, so each check hangs in its destination poll.

## Steps to Reproduce
1. Boot the emulator; create an S3 bucket and a firehose-trusted role with s3:PutObject on it.
2. firehose.create_delivery_stream(DeliveryStreamName=s, DeliveryStreamType='DirectPut', ExtendedS3DestinationConfiguration={'BucketARN': ..., 'RoleARN': ..., 'Prefix': 'q/', 'BufferingHints': {'SizeInMBs': 1, 'IntervalInSeconds': 60}}); wait for ACTIVE.
3. put_record_batch with 500 records carrying a batch marker -> 200, FailedPutCount 0.
4. Poll s3.list_objects_v2(Bucket=..., Prefix='q/') for several minutes: expect objects containing all 500 markers; observe the bucket stays empty.
5. Repeat with a DynamicPartitioningConfiguration (jq MetadataExtractionQuery) and with an UpdateDestination that repoints BucketARN to a second bucket, to see that no prefix in either bucket ever receives an object.
6. Re-replay the check ids in trace_ids.
