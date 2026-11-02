Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose__kinesis__s3.cli-kinesis-put-record-firehose-delivers-s3-workflow
Issue-Key: firehose__kinesis__s3.cli-kinesis-put-record-firehose-delivers-s3-workflow|run#26|AssertionError|assert found, f"delivered S3 object(s) never contained the expected marker {marker!r} within the poll budget"
Issue-Commit: ebea418ed7238d7557ed4699ab6be023240b4505

# firehose__kinesis__s3: the CLI-driven Kinesis→Firehose→S3 workflow delivers no object — a record put to the source stream never reaches the ExtendedS3 destination (1 cause)

> _Note: this issue was identified against the code base at commit `ebea418ed7238d7557ed4699ab6be023240b4505`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRecord

## Description
One net-new root cause, grounded in the CLI round-trip check. Open issues 20261026T074043Z-firehose-kinesis-s3-a-kinesisstreamassource-delivery-stream-never.md and 20261029T175617Z-firehose-kinesis-s3-4-net-new-root-causes.md describe the same ingestion family from SDK-driven checks and do not carry this check's replay key, so this failure is not covered by either.

Cause 1 of 1: a KinesisStreamAsSource delivery stream created and fed entirely through the AWS CLI never delivers its source records to the ExtendedS3 destination.
AWS: recorded behavior only: after `aws kinesis create-stream`, `aws firehose create-delivery-stream --delivery-stream-type KinesisStreamAsSource --kinesis-stream-source-configuration '{KinesisStreamARN,RoleARN}' --extended-s3-destination-configuration '{RoleARN,BucketARN,Prefix:"records/",BufferingHints:{SizeInMBs:1,IntervalInSeconds:60}}'` and the stream reporting ACTIVE, a payload put with `aws kinesis put-record` is read from the source shard and written verbatim into an object under the destination Prefix (records/...), so a GetObject of the delivered object contains the payload bytes. Firehose's own retry/buffering may delay this, which is why the payload is re-sent periodically, but at least one delivered object must contain it.
Emulator: no delivered object ever contains the payload. The CLI create and put both succeed and the delivery stream reports ACTIVE, but the destination Prefix yields nothing holding the marker for the whole poll budget; the check fails with 'delivered S3 object(s) never contained the expected marker <payload> within the poll budget'. Consistent with the already-filed finding that a KinesisStreamAsSource stream never consumes its source stream.
Why: the entire CLI-level Kinesis→Firehose→S3 pipeline silently loses every record: the producer sees success at both hops and nothing is ever durable, so no consumer of the bucket can observe the data.
Fix: the Firehose source-ingestion path for KinesisStreamSourceConfiguration plus the ExtendedS3 destination writer (services/service-com-amazonaws-firehose-firehose-20150804/src/ — the KinesisStreamAsSource consumer and the S3 delivery step) must poll the source shards once the stream is ACTIVE and write the consumed records under the destination Prefix.
Traces: firehose__kinesis__s3.cli-kinesis-put-record-firehose-delivers-s3-workflow (check line 108).

## Evidence
Cause 1 — firehose__kinesis__s3.cli-kinesis-put-record-firehose-delivers-s3-workflow:
  setup (all via the aws CLI): kinesis create-stream --shard-count 1; firehose create-delivery-stream --delivery-stream-type KinesisStreamAsSource --extended-s3-destination-configuration with Prefix 'records/' and BufferingHints {SizeInMBs:1, IntervalInSeconds:60}; delivery stream observed ACTIVE; kinesis put-record of base64('hello-<marker>-0\n'), re-sent every 30 s
  expected: at least one object under records/ whose body contains the payload text
  actual:   no object under the destination prefix ever contains it — AssertionError: delivered S3 object(s) never contained the expected marker 'payload-<suffix>' within the poll budget (the marker string is a per-run value; the divergence is that nothing is delivered)

## Steps to Reproduce
1. Create an S3 bucket and an IAM role trusted by firehose.amazonaws.com with kinesis (DescribeStream, GetShardIterator, GetRecords, ListShards) on the source stream and the usual s3 delivery permissions on the bucket.
2. `aws kinesis create-stream --stream-name <s> --shard-count 1` and wait for ACTIVE.
3. `aws firehose create-delivery-stream --delivery-stream-name <ds> --delivery-stream-type KinesisStreamAsSource --kinesis-stream-source-configuration '{"KinesisStreamARN":"<arn>","RoleARN":"<role>"}' --extended-s3-destination-configuration '{"RoleARN":"<role>","BucketARN":"arn:aws:s3:::<bucket>","Prefix":"records/","BufferingHints":{"SizeInMBs":1,"IntervalInSeconds":60}}'` and wait for ACTIVE.
4. `aws kinesis put-record --stream-name <s> --partition-key pk1 --data <base64 of 'hello-<marker>-0\n'>`.
5. Poll ListObjectsV2(Bucket=<bucket>, Prefix='records/') and GetObject each key: one of them must contain the payload text.
