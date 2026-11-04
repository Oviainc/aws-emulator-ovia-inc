Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose__kinesis__s3.firehose-kinesis-source-delivers-to-s3-positive
Issue-Key: firehose__kinesis__s3.firehose-kinesis-source-delivers-to-s3-positive|_list_objects#12|Timeout|resp = s3.list_objects_v2(Bucket=bucket, Prefix=prefix)
Issue-Commit: ad24016a8a179a73f06eb7df572fd7385de96eb5

# firehose__kinesis__s3: a KinesisStreamAsSource delivery stream never ingests its source stream — a record put to Kinesis produces no S3 object (firehose-kinesis-source-delivers-to-s3-positive)

> _Note: this issue was identified against the code base at commit `ad24016a8a179a73f06eb7df572fd7385de96eb5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeliveryStream

## Description
One net-new root cause.

Cause 1 of 1: a delivery stream created with DeliveryStreamType=KinesisStreamAsSource never consumes the Kinesis stream named by its KinesisStreamSourceConfiguration, so records put to that stream are never buffered and never written to the ExtendedS3DestinationConfiguration bucket. Failing check id: firehose__kinesis__s3.firehose-kinesis-source-delivers-to-s3-positive.

AWS: Smithy (services/service-com-amazonaws-firehose-firehose-20150804/smithy-model/com-amazonaws-firehose-firehose-20150804.json) models CreateDeliveryStream with DeliveryStreamType (enum, value KinesisStreamAsSource), KinesisStreamSourceConfiguration {KinesisStreamARN, RoleARN} and ExtendedS3DestinationConfiguration {RoleARN, BucketARN, BufferingHints {SizeInMBs, IntervalInSeconds}}. On real AWS, once such a stream reaches DeliveryStreamStatus ACTIVE, Firehose assumes the configured RoleARN, reads the named Kinesis data stream continuously, and flushes the buffered records to the destination bucket when either buffering hint is reached (here SizeInMBs=1 or IntervalInSeconds=60, whichever comes first), writing at least one object under the stream's configured prefix whose body contains the record's payload bytes verbatim. The recording shows exactly that: a single kinesis PutRecord of a payload is followed by a ListObjectsV2 on the destination bucket that returns one or more keys, and a GetObject on one of them whose body contains the payload bytes. No error code or non-2xx is involved anywhere in this path — every API call is accepted; the divergence is purely the absent side effect.

Emulator: the delivery stream is created and reaches ACTIVE, and kinesis PutRecord is accepted, but the destination bucket stays empty for the whole polling window — the replay reports `Error: Check timed out after 60.0s` with the check still polling _list_objects(s3, bucket) for its first object, so no object is ever created and the `expected firehose to deliver at least one object to <bucket>, found none` criterion is never reachable. Nothing consumes the source stream: the Kinesis→Firehose ingestion loop does not exist, so neither the size nor the interval buffering hint can ever fire.

Why: a client that wires Kinesis Data Streams to S3 through Firehose — the standard ingest/landing-zone pattern — sees a healthy ACTIVE delivery stream, a successful PutRecord, and then silently loses every record. There is no error to react to and no retry signal, so pipelines that verify landed objects (or read them back for downstream ETL/Athena/Glue) hang or conclude the data was dropped, and any test that asserts end-to-end delivery can never pass.

Fix: the Firehose delivery-stream runtime/state for KinesisStreamAsSource sources — the component that, on CreateDeliveryStream with a KinesisStreamSourceConfiguration, must register a consumer against the named Kinesis stream and drive the buffering/flush path into the ExtendedS3 destination writer. It belongs in the firehose service's delivery-stream ingestion machinery, not in the CreateDeliveryStream request validator (which already accepts the request correctly).

Related open issues that describe nearby symptoms but do NOT carry this check's replay key, so none of them covers it: 20261026T074043Z-firehose-kinesis-s3-a-kinesisstreamassource-delivery-stream-never.md, 20261029T175617Z-firehose-kinesis-s3-4-net-new-root-causes.md, 20261101T223651Z-firehose-kinesis-s3-cloudformation-deployed-kinesis-firehose-s3.md and 20261102T100157Z-firehose-kinesis-s3-the-cli-driven-kinesis-firehose.md (that last one covers the CLI-driven variant). A fixer closing this issue should re-replay the check id listed below.

Traces: firehose__kinesis__s3.firehose-kinesis-source-delivers-to-s3-positive (replay detail: `Error: Check timed out after 60.0s`, raised while polling the destination bucket for the first delivered object).

## Evidence
Expected (real AWS, per the recording): CreateDeliveryStream(DeliveryStreamType="KinesisStreamAsSource", KinesisStreamSourceConfiguration={KinesisStreamARN:<source stream arn>, RoleARN:<role arn>}, ExtendedS3DestinationConfiguration={RoleARN:<role arn>, BucketARN:<bucket arn>, BufferingHints:{SizeInMBs:1, IntervalInSeconds:60}}) -> 200 with a DeliveryStreamARN; DescribeDeliveryStream -> DeliveryStreamStatus "ACTIVE"; kinesis PutRecord(Data=<payload>) -> 200; then ListObjectsV2 on the destination bucket returns >= 1 key and GetObject on that key returns a body containing the payload bytes verbatim.

Actual (emulator): every call above is accepted with the same success statuses, but ListObjectsV2 on the destination bucket returns zero keys for the entire polling window and the check ends with `Error: Check timed out after 60.0s` inside the delivery poll (`_poll_until(lambda: _list_objects(s3, bucket), lambda objs: len(objs) >= 1, timeout_s=240, interval_s=10)`), i.e. the assertion `expected firehose to deliver at least one object to <bucket>, found none` is never satisfiable. No error code and no non-2xx response is returned at any point — the divergence is the missing ingestion side effect, not a refusal.

The bucket name, stream name, role name, payload marker and request ids differ per run and are not part of the requirement; the requirement is that at least one object exists in the destination bucket after the buffering interval and that its body contains the record payload.

## Steps to Reproduce
1. Create an S3 bucket and an IAM role whose policy allows kinesis:GetRecords/GetShardIterator/DescribeStream on a Kinesis stream and s3:PutObject/AbortMultipartUpload/GetBucketLocation/ListBucket on that bucket, with a trust policy for firehose.amazonaws.com.
2. Create a Kinesis data stream and wait for it to be ACTIVE.
3. firehose CreateDeliveryStream with DeliveryStreamName=<name>, DeliveryStreamType="KinesisStreamAsSource", KinesisStreamSourceConfiguration={KinesisStreamARN:<stream arn>, RoleARN:<role arn>}, ExtendedS3DestinationConfiguration={RoleARN:<role arn>, BucketARN:<bucket arn>, BufferingHints:{SizeInMBs:1, IntervalInSeconds:60}}.
4. Poll DescribeDeliveryStream until DeliveryStreamStatus is ACTIVE.
5. kinesis PutRecord(StreamName=<stream>, Data=b"<payload-with-unique-marker>", PartitionKey="pk-1").
6. Poll s3 ListObjectsV2 on the destination bucket.
   Expected: at least one object appears once a buffering hint is reached, and GetObject on it returns a body containing the marker.
   Actual: the bucket stays empty indefinitely; the check times out with `Check timed out after 60.0s` while polling.
Replay: run the behavioral suite and re-run check firehose__kinesis__s3.firehose-kinesis-source-delivers-to-s3-positive.
