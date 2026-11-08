Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose__kinesis__s3.firehose-kinesis-source-delivers-to-s3-positive firehose__kinesis__s3.firehose-s3-buffering-hints-flush-positive
Issue-Key: firehose__kinesis__s3.firehose-kinesis-source-delivers-to-s3-positive|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Key: firehose__kinesis__s3.firehose-s3-buffering-hints-flush-positive|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Commit: 5c83555598642a9bc89b001ced3efa04a61d0066

# firehose__kinesis__s3: a KinesisStreamAsSource delivery stream never consumes its source Kinesis stream, so records put to Kinesis never become S3 objects (1 cause)

> _Note: this issue was identified against the code base at commit `5c83555598642a9bc89b001ced3efa04a61d0066`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeliveryStream

## Description
One net-new root cause behind both remaining firehose__kinesis__s3.* behavioral failures (neither is marked ALREADY FILED nor OWNED by an open issue). Related but NOT covering: 20261024T005347Z-firehose-never-delivers-buffered-records-to-the-s3.md and 20261025T083505Z-firehose-s3-delivery-ignores-bufferinghints-the-destination-role.md both describe the DirectPut path — records that PutRecord/PutRecordBatch already placed in the stream's in-process buffer and that nothing flushes. This cause is one step earlier and on a different ingress: for DeliveryStreamType=KinesisStreamAsSource there is no PutRecord at all, and the emulator has no component that READS the configured source Kinesis data stream, so nothing is ever even buffered. Neither of those issues' replay lists contains the two check ids below.

Cause 1 of 1: a delivery stream created with DeliveryStreamType="KinesisStreamAsSource" plus KinesisStreamSourceConfiguration{KinesisStreamARN, RoleARN} reaches ACTIVE but never consumes the named Kinesis stream; records written with kinesis:PutRecord are therefore never buffered, never flushed, and no object ever appears under the S3/ExtendedS3 destination — neither on the IntervalInSeconds timer nor on the SizeInMBs threshold. Failing checks: firehose__kinesis__s3.firehose-kinesis-source-delivers-to-s3-positive, firehose__kinesis__s3.firehose-s3-buffering-hints-flush-positive.

AWS: Smithy (services/service-com-amazonaws-firehose-firehose-20150804/smithy-model/com-amazonaws-firehose-firehose-20150804.json) — DeliveryStreamType enum includes KinesisStreamAsSource; KinesisStreamSourceConfiguration is "The stream and role Amazon Resource Names (ARNs) for a Kinesis data stream used as the source for a Firehose stream", with required KinesisStreamARN (pattern ^arn:.*:kinesis:[a-zA-Z0-9\-]+:\d{12}:stream/[a-zA-Z0-9_.-]+$) and required RoleARN ("the role that provides access to the source Kinesis data stream"). BufferingHints.SizeInMBs (@range 1..128, default 5) and BufferingHints.IntervalInSeconds (@range 0..900, default 300) are documented as "Buffer incoming data to the specified size, in MiBs, before delivering it to the destination" / "Buffer incoming data for the specified period of time, in seconds, before delivering it to the destination". On real AWS, Firehose assumes the source RoleARN, reads the shards of the source stream with GetShardIterator/GetRecords, buffers the record bytes and writes them as objects under the destination bucket+Prefix (YYYY/MM/DD/HH) once EITHER hint is met; the SizeInMBs hint fires independently of, and well before, a long IntervalInSeconds.

Emulator: both checks create the Kinesis stream (it reaches ACTIVE and resolves on the data plane), create the KinesisStreamAsSource delivery stream, see DescribeDeliveryStream report DeliveryStreamStatus=ACTIVE, and write records with kinesis.put_record — and then s3:ListObjectsV2 on the destination bucket returns no Contents for the whole poll budget, so both checks end at the suite's wall (verdict "Error: Check timed out after 60.0s" against their own 240 s / 280 s destination polls). The Firehose crate has no source-ingestion or delivery component at all: services/service-com-amazonaws-firehose-firehose-20150804/src/lib.rs declares only `ops`, `scaffolded`, `service`, `state`; the source configuration is accepted and echoed by state only, and nothing in the crate reads from the Kinesis service's state or writes into the S3 service's state.

Why: a producer writing to a Kinesis stream that is wired as a Firehose source gets a successful Kinesis write and an ACTIVE delivery stream, and its data is silently discarded — the exact silent-data-loss shape a producer cannot detect from any API answer. Every KDS->Firehose->S3 pipeline (the standard archival/landing-zone pattern, including downstream Athena/Glue tables over the bucket) is inert, and the whole KinesisStreamAsSource half of the service's behavior — source-role assumption, per-shard ordering into the buffer, the size-vs-interval flush distinction (SizeInMBs=1 crossing after ~1.2 MiB while IntervalInSeconds=900 has not elapsed), and Prefix expansion on the resulting keys — is unobservable because step one, consuming the source stream, never happens.

Fix: a source-ingestion component in the Firehose service crate that, for DeliveryStreamType=KinesisStreamAsSource, pulls records from the configured KinesisStreamARN (via the Kinesis service's state) into the per-stream buffer that already exists in services/service-com-amazonaws-firehose-firehose-20150804/src/state.rs, feeding the same destination flush path that BufferingHints.SizeInMBs/IntervalInSeconds must drive; src/lib.rs currently declares no module that could do either.

Traces: firehose__kinesis__s3.firehose-kinesis-source-delivers-to-s3-positive (one 60 KB-free record, ExtendedS3 destination, BufferingHints 1 MiB/60 s, polls 240 s for an object containing the marker), firehose__kinesis__s3.firehose-s3-buffering-hints-flush-positive (20 x ~60 KB records = ~1.2 MiB, S3DestinationConfiguration, BufferingHints 1 MiB/900 s, polls 280 s so that any object can only be size-triggered).

## Evidence
Verdict for both checks: "Error: Check timed out after 60.0s" — each blocks in its destination poll (_poll_until(lambda: _list_objects(s3, bucket), lambda o: len(o) >= 1, timeout_s=240 / 280, interval_s=10)) because list_objects_v2 on the destination bucket never returns any Contents.
Expected (real AWS): after at most BufferingHints.IntervalInSeconds (60 s in the first check) — and for the second check as soon as the cumulative buffered bytes cross BufferingHints.SizeInMBs=1, far inside the 900 s interval — list_objects_v2 on the destination bucket returns at least one object whose body contains the exact bytes written to the SOURCE Kinesis stream (payload 'sim2real-payload-<marker>' / '<marker>-xxxx...'), keyed under the destination Prefix + YYYY/MM/DD/HH.
Actual (emulator): kinesis.create_stream -> ACTIVE, kinesis.put_record -> 200, firehose.create_delivery_stream(DeliveryStreamType='KinesisStreamAsSource', KinesisStreamSourceConfiguration={KinesisStreamARN, RoleARN}, [Extended]S3DestinationConfiguration={RoleARN, BucketARN, BufferingHints}) -> 200, describe_delivery_stream -> DeliveryStreamStatus='ACTIVE', and the destination bucket stays empty indefinitely. No module in services/service-com-amazonaws-firehose-firehose-20150804 consumes the source stream or writes to S3 (src/lib.rs: `pub mod ops; pub mod scaffolded; pub mod service; pub mod state;`).

## Steps to Reproduce
1. Boot the emulator and point botocore at it.
2. s3.create_bucket(Bucket=b).
3. kinesis.create_stream(StreamName=k, ShardCount=1); poll describe_stream_summary until StreamStatus == 'ACTIVE'; take StreamARN.
4. iam.create_role(...) with a trust policy for firehose.amazonaws.com and put_role_policy granting kinesis:DescribeStream/DescribeStreamSummary/GetRecords/GetShardIterator/ListShards on the stream ARN plus s3:PutObject/ListBucket/GetBucketLocation/AbortMultipartUpload on arn:aws:s3:::<b> and <b>/*.
5. firehose.create_delivery_stream(DeliveryStreamName=f, DeliveryStreamType='KinesisStreamAsSource', KinesisStreamSourceConfiguration={'KinesisStreamARN': stream_arn, 'RoleARN': role_arn}, ExtendedS3DestinationConfiguration={'RoleARN': role_arn, 'BucketARN': 'arn:aws:s3:::'+b, 'BufferingHints': {'SizeInMBs': 1, 'IntervalInSeconds': 60}}).
6. Poll describe_delivery_stream until DeliveryStreamStatus == 'ACTIVE'.
7. kinesis.put_record(StreamName=k, Data=b'sim2real-payload-marker', PartitionKey='pk-1').
8. Poll s3.list_objects_v2(Bucket=b) for several minutes: expect at least one object whose body contains b'sim2real-payload-marker'; observe Contents stays empty forever.
9. Size-trigger arm: repeat steps 2-6 with S3DestinationConfiguration and BufferingHints {'SizeInMBs': 1, 'IntervalInSeconds': 900}, then kinesis.put_record 20 times with ~60 KB each (~1.2 MiB total) and poll s3.list_objects_v2 for 280 s — an object must appear from the size hint alone; none does.
10. Re-replay the check ids in trace_ids.
