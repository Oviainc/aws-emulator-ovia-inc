Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose.buffering-hints-delay-delivery-to-s3 firehose.buffering-size-threshold-triggers-early-flush firehose.concurrent-put-record-batch-calls-no-record-loss firehose.create-stream-role-without-s3-permissions-denied firehose.custom-time-zone-shifts-the-timestamp-prefix-expansion firehose.delivered-object-content-type-and-encoding-match-declared-compression firehose.delivered-object-suffix-carries-the-stream-version-reported-by-describe firehose.deploy-cfn-firehose-stream-becomes-active firehose.deploy-cfn-stream-active-and-delivers-records-to-s3 firehose.deploy-cfn-stream-s3-destination-config-applied firehose.e2e-create-put-deliver-to-s3-delete-and-confirm-gone firehose.e2e-create-put-flush-verify-delete-preserves-s3-objects firehose.e2e-create-put-land-delete-full-flow firehose.e2e-tag-encrypt-update-destination-consequence-flow firehose.empty-record-data-preserved firehose.file-extension-overrides-the-compression-suffix-without-changing-the-bytes firehose.put-record-base64-binary-blob-preserved firehose.put-record-batch-large-payload-byte-equality-preserved firehose.put-record-batch-partial-failure-reported firehose.put-record-batch-success-all-ids-unique firehose.put-record-data-exceeds-max-size firehose.put-record-max-size-payload-boundary-preserved firehose.put-record-oversized-payload-rejected firehose.put-record-rejected-while-stream-creating firehose.put-record-returns-unique-record-id firehose.put-record-then-batch-preserve-arrival-order-in-s3 firehose.put-record-unicode-payload-byte-equal-round-trip firehose.record-de-aggregation-json-splits-one-put-across-jq-derived-prefixes firehose.s3-buffering-hint-controls-delivery-latency firehose.s3-delivery-triggered-by-buffer-interval-timeout firehose.server-side-enforces-single-active-role-after-update-destination firehose.server-side-kms-encryption-enforced-for-s3-objects firehose.server-side-managed-flush-without-explicit-trigger firehose.server-side-role-permission-removal-blocks-delivery firehose.snappy-and-hadoop-snappy-are-distinct-compressed-deliveries firehose.zip-compression-delivers-a-readable-zip-archive
Issue-Shape-Id: com.amazonaws.firehose#Firehose_20150804
Issue-Key: firehose.buffering-hints-delay-delivery-to-s3|run#12|Timeout|time.sleep(min(10, early_deadline - time.monotonic()))
Issue-Key: firehose.buffering-size-threshold-triggers-early-flush|_poll_s3_objects#17|Timeout|time.sleep(interval_s)
Issue-Key: firehose.concurrent-put-record-batch-calls-no-record-loss|_poll_until#18|Timeout|time.sleep(interval_s)
Issue-Key: firehose.create-stream-role-without-s3-permissions-denied|run#23|Timeout|time.sleep(10)
Issue-Key: firehose.custom-time-zone-shifts-the-timestamp-prefix-expansion|_wait_for_one_object#23|Timeout|time.sleep(_POLL_INTERVAL_S)
Issue-Key: firehose.delivered-object-content-type-and-encoding-match-declared-compression|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Key: firehose.delivered-object-suffix-carries-the-stream-version-reported-by-describe|_wait_for_keys#15|Timeout|time.sleep(_POLL_INTERVAL_S)
Issue-Key: firehose.deploy-cfn-firehose-stream-becomes-active|_poll_until#18|Timeout|time.sleep(interval_s)
Issue-Key: firehose.deploy-cfn-stream-active-and-delivers-records-to-s3|_poll_until#17|Timeout|time.sleep(interval_s)
Issue-Key: firehose.deploy-cfn-stream-s3-destination-config-applied|_wait_for_object#10|Timeout|time.sleep(15)
Issue-Key: firehose.e2e-create-put-deliver-to-s3-delete-and-confirm-gone|_wait_for_lines#15|Timeout|time.sleep(15)
Issue-Key: firehose.e2e-create-put-flush-verify-delete-preserves-s3-objects|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Key: firehose.e2e-create-put-land-delete-full-flow|_wait_for_objects_containing#14|Timeout|time.sleep(interval_s)
Issue-Key: firehose.e2e-tag-encrypt-update-destination-consequence-flow|_wait_for_gzip_object_with_marker#27|Timeout|time.sleep(15)
Issue-Key: firehose.empty-record-data-preserved|_wait_for_s3_object#13|Timeout|time.sleep(interval_s)
Issue-Key: firehose.file-extension-overrides-the-compression-suffix-without-changing-the-bytes|_wait_for_one_object#22|Timeout|time.sleep(_POLL_INTERVAL_S)
Issue-Key: firehose.put-record-base64-binary-blob-preserved|_wait_for_s3_bytes#9|Timeout|time.sleep(interval_s)
Issue-Key: firehose.put-record-batch-large-payload-byte-equality-preserved|_wait_for_s3_bytes#13|Timeout|time.sleep(interval_s)
Issue-Key: firehose.put-record-batch-partial-failure-reported|_wait_for_marker_delivered#22|Timeout|time.sleep(5.0)
Issue-Key: firehose.put-record-batch-success-all-ids-unique|_poll_until#16|Timeout|time.sleep(interval_s)
Issue-Key: firehose.put-record-data-exceeds-max-size|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Key: firehose.put-record-max-size-payload-boundary-preserved|_wait_for_object#9|Timeout|time.sleep(5)
Issue-Key: firehose.put-record-oversized-payload-rejected|run#13|Timeout|time.sleep(10)
Issue-Key: firehose.put-record-rejected-while-stream-creating|_poll#19|Timeout|time.sleep(interval_s)
Issue-Key: firehose.put-record-returns-unique-record-id|_poll_until#7|Timeout|time.sleep(interval_s)
Issue-Key: firehose.put-record-then-batch-preserve-arrival-order-in-s3|_poll_s3_objects#13|Timeout|time.sleep(interval_s)
Issue-Key: firehose.put-record-unicode-payload-byte-equal-round-trip|_wait_for_object#7|Timeout|time.sleep(5)
Issue-Key: firehose.record-de-aggregation-json-splits-one-put-across-jq-derived-prefixes|_poll_until#20|Timeout|time.sleep(_POLL_INTERVAL_S)
Issue-Key: firehose.s3-buffering-hint-controls-delivery-latency|run#18|Timeout|time.sleep(10)
Issue-Key: firehose.s3-delivery-triggered-by-buffer-interval-timeout|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Key: firehose.server-side-enforces-single-active-role-after-update-destination|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Key: firehose.server-side-kms-encryption-enforced-for-s3-objects|_poll_until#24|Timeout|time.sleep(interval_s)
Issue-Key: firehose.server-side-managed-flush-without-explicit-trigger|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Key: firehose.server-side-role-permission-removal-blocks-delivery|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: firehose.snappy-and-hadoop-snappy-are-distinct-compressed-deliveries|_wait_for_objects#14|Timeout|time.sleep(_POLL_INTERVAL_S)
Issue-Key: firehose.zip-compression-delivers-a-readable-zip-archive|_wait_for_objects#20|Timeout|time.sleep(_POLL_INTERVAL_S)
Issue-Commit: 4affd7a378830b3c6d8042007351e66db953803d

# Firehose never delivers buffered records to the S3 destination: PutRecord/PutRecordBatch mint RecordIds into an in-memory buffer that nothing ever flushes

> _Note: this issue was identified against the code base at commit `4affd7a378830b3c6d8042007351e66db953803d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRecord

## Description
One net-new root cause behind every remaining firehose.* behavioral failure that is not marked ALREADY FILED or OWNED by an open issue. The open issue 20261022T194351Z-firehose-control-plane-configuration-echo-drops-members-tag.md covers the Firehose CONTROL plane (configuration echo, tag/KMS validation, message wording, CFN output resolution) and the open issue 20261022T064119Z-firehose-createdeliverystream-putrecordbatch-are-overly-permissive-and-misclassify.md covers create/put permissiveness; neither covers the destination-delivery path described here, and none of the check ids listed below is on their replay lists with this failure signature.

Cause 1 of 1: a record accepted by PutRecord/PutRecordBatch is buffered in process memory and is never written to the stream's configured destination, so no object ever appears in the destination S3 bucket.

AWS: Smithy (services/service-com-amazonaws-firehose-firehose-20150804/smithy-model) PutRecord doc: "Firehose buffers records before delivering them to the destination" and "Data records sent to Firehose are stored for 24 hours from the time they are added to a Firehose stream as it tries to send the records to the destination". BufferingHints.SizeInMBs (range 1..128, default 5) and BufferingHints.IntervalInSeconds (range 0..900, default 300) are documented as "Buffer incoming data ... before delivering it to the destination". ExtendedS3DestinationConfiguration models exactly how that buffered data becomes objects: BucketARN (pattern ^arn:.*:s3:::[\w\.\-]{1,255}$), Prefix ("The \"YYYY/MM/DD/HH\" time format prefix is automatically used for delivered Amazon S3 files"), ErrorOutputPrefix ("a prefix that Firehose evaluates and adds to failed records before writing them to S3"), CompressionFormat (enum UNCOMPRESSED|GZIP|ZIP|Snappy|HADOOP_SNAPPY, default UNCOMPRESSED), FileExtension, CustomTimeZone, EncryptionConfiguration, DataFormatConversionConfiguration, DynamicPartitioningConfiguration and ProcessingConfiguration. So on real AWS, once the size or interval hint is met, objects containing the record bytes exist under the destination bucket/prefix, with the declared compression, extension, SSE and partitioning applied.

Emulator: PutRecord/PutRecordBatch resolve the stream, mint a RecordId and append the data to an in-process buffer only (services/service-com-amazonaws-firehose-firehose-20150804/src/ops/put_record.rs -> state::ingest_record; services/service-com-amazonaws-firehose-firehose-20150804/src/state.rs holds DeliveryStream.records / buffered_bytes under heap budgets MAX_BUFFERED_BYTES_PER_STREAM etc.). The only thing that ever removes a buffered record is expiry at RECORD_RETENTION (24 h) inside materialise_due. The crate declares no delivery component at all (src/lib.rs: ops, scaffolded, service, state) and nothing writes to the S3 service's state. Consequently every check that reads the destination bucket after a successful put sees an empty bucket and polls until the suite's budget expires (verdict "Error: Check timed out after 60.0s"), including the ones whose API-level assertions had already passed and only the terminal destination read remained.

Why: a client gets HTTP 200 and a RecordId for data that is then silently dropped — the exact silent-data-loss failure producers cannot detect. Every downstream consumer (Athena/Glue tables over the bucket, log pipelines, backups) sees nothing. In addition, no delivery-shaped behavior is observable at all: buffer-interval and buffer-size flush triggers, the YYYY/MM/DD/HH prefix expansion and CustomTimeZone shift, CompressionFormat bodies (GZIP/ZIP/Snappy vs HADOOP_SNAPPY) and the matching Content-Type/Content-Encoding, FileExtension override, object key suffix carrying the stream version, EncryptionConfiguration (SSE-KMS on the written object), dynamic partitioning / jq-derived prefixes, record de-aggregation, ErrorOutputPrefix routing, and role-permission enforcement at delivery time (a role without s3:PutObject must stop delivery) are all unreachable because step one — producing an object — never happens.

Fix: a delivery/flush component in the Firehose service crate that materialises buffered records into the configured destination through the S3 service's state, driven by BufferingHints, and that applies Prefix/ErrorOutputPrefix expansion, CompressionFormat, FileExtension, CustomTimeZone, EncryptionConfiguration, DynamicPartitioningConfiguration and ProcessingConfiguration; the buffer in services/service-com-amazonaws-firehose-firehose-20150804/src/state.rs is where the records already sit, and src/lib.rs declares no module that consumes them.

Traces: the 36 firehose.* check ids in trace_ids, each of which asserts on bytes read back out of the destination bucket after a successful put (examples: firehose.put-record-returns-unique-record-id polls s3 for both payloads for 240 s; firehose.s3-delivery-triggered-by-buffer-interval-timeout; firehose.zip-compression-delivers-a-readable-zip-archive; firehose.put-record-rejected-while-stream-creating, whose CREATING-state refusal assertions pass and which then hangs on the ACTIVE-state delivery control). firehose.deploy-cfn-* reach the same terminal delivery read; the CFN output/provisioning side of those checks is the already-open control-plane issue named above, the delivery side is this cause.

## Evidence
Verdict for all 36 checks: "Error: Check timed out after 60.0s" — the checks block in their destination polls (e.g. _wait_for_s3_object(timeout_s=300) / _poll_until(..., timeout_s=240, description="both PutRecord payloads delivered to the S3 destination")) because list_objects_v2 on the destination bucket never returns any Contents.
Expected (real AWS): after at most BufferingHints.IntervalInSeconds (60 s in these checks) list_objects_v2 on the destination bucket returns at least one object whose body contains the exact record bytes, keyed under the configured Prefix + YYYY/MM/DD/HH, with the declared CompressionFormat/FileExtension/SSE applied.
Actual (emulator): PutRecord returns 200 with a RecordId (e.g. '000000000000000018e14f510daddfb10000000000000005'-style ids), DescribeDeliveryStream reports ACTIVE, and the destination bucket stays empty indefinitely; the record is only held in services/service-com-amazonaws-firehose-firehose-20150804/src/state.rs (DeliveryStream.records) until the 24 h RECORD_RETENTION expiry drops it. No module in that crate writes to S3 (src/lib.rs declares only ops, scaffolded, service, state).

## Steps to Reproduce
1. Boot the emulator and point botocore at it.
2. s3.create_bucket(Bucket=b); iam.create_role(...) with a trust policy for firehose.amazonaws.com plus s3:PutObject on b.
3. firehose.create_delivery_stream(DeliveryStreamName=s, DeliveryStreamType='DirectPut', ExtendedS3DestinationConfiguration={'RoleARN': role, 'BucketARN': 'arn:aws:s3:::'+b, 'BufferingHints': {'SizeInMBs': 1, 'IntervalInSeconds': 60}, 'CompressionFormat': 'UNCOMPRESSED'}).
4. Poll describe_delivery_stream until DeliveryStreamStatus == 'ACTIVE' (fast in the emulator).
5. firehose.put_record(DeliveryStreamName=s, Record={'Data': b'hello-firehose-record'}) -> 200 with a RecordId.
6. Poll s3.list_objects_v2(Bucket=b) for several minutes: expect at least one object containing b'hello-firehose-record'; observe Contents stays empty forever.
7. Repeat with CompressionFormat GZIP/ZIP/Snappy/HADOOP_SNAPPY, a FileExtension, a CustomTimeZone, an EncryptionConfiguration and a DynamicPartitioningConfiguration to see that none of those behaviors is observable because no object is ever written.
8. Re-replay the check ids in trace_ids.
