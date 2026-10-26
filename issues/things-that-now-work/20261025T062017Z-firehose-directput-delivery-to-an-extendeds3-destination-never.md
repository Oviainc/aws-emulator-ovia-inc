Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena__firehose__glue__s3.firehose-raw-json-delivery-queryable-via-athena-recursive-prefix
Issue-Key: athena__firehose__glue__s3.firehose-raw-json-delivery-queryable-via-athena-recursive-prefix|_poll_until#14|Timeout|time.sleep(interval_s)
Issue-Commit: 2b5b554775f720de5c9510c8dde5c1ff761ca735

# Firehose DirectPut delivery to an ExtendedS3 destination never materialises an object, so the recursive-prefix read-back stalls

> _Note: this issue was identified against the code base at commit `2b5b554775f720de5c9510c8dde5c1ff761ca735`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRecord

## Description
This issue carries 1 net-new root cause. It must be closed before the issue is done.

Cause 1 of 1: a DirectPut delivery stream with an ExtendedS3DestinationConfiguration accepts PutRecord but never completes its buffered delivery, so no S3 object ever appears under the configured Prefix and the service's own date segments are never produced.

AWS: Smithy PutRecord takes DeliveryStreamName (required, length 1..64, pattern ^[a-zA-Z0-9_.-]+$) and Record, returning RecordId; its modeled refusals are InvalidArgumentException / ResourceNotFoundException / InvalidSourceException / InvalidKMSResourceException (400) and ServiceUnavailableException (503). Smithy ExtendedS3DestinationConfiguration.Prefix documents that "the \"YYYY/MM/DD/HH\" time format prefix is automatically used for delivered Amazon S3 files" on top of any custom prefix. Recorded behavior: after PutRecord into an ACTIVE DirectPut stream whose destination carries Prefix='<stream>/' and BufferingHints {SizeInMBs: 1, IntervalInSeconds: 60}, objects appear in S3 under '<stream>/yyyy/MM/dd/HH/...' within the buffering interval, with nothing diverted to ErrorOutputPrefix.

Emulator: the check gets past the S3 bucket, the Glue database/table, the Athena workgroup, the IAM role and the delivery-stream create, and then stalls — the harness killed it at its 60s budget. Every poll it can be sitting in belongs to Firehose and all of them precede the first Athena call: the describe_delivery_stream poll for ACTIVE, the PutRecord retry loop, and the list_objects_v2 poll for at least one delivered object under the prefix. No object is ever listed, so the delivery never lands. Hypothesis: the buffered records are retained in memory and nothing flushes them to S3 — the same family the open issue 20261024T005347Z-firehose-never-delivers-buffered-records-to-the-s3.md describes for other check ids; that issue does not carry this check's replay key, and the neighbouring 20261024T005347Z-firehose-s3-firehose-to-s3-data-plane-never-materialises-objects.md covers dynamic partitioning / repointing rather than the static-Prefix recursive read path asserted here.

Why: a client that writes through Firehose and then reads the destination (directly, or through a non-partitioned Hive/Glue EXTERNAL_TABLE pointed at the static prefix, which must read the nested date segments recursively) sees an empty prefix forever. PutRecord's 200 and its RecordId promise a delivery that never happens, and nothing lands under ErrorOutputPrefix either, so the loss is silent: the caller cannot distinguish "not yet buffered" from "never delivered".

Fix: the delivery/flush path of the Firehose delivery-stream state machine (the ExtendedS3 destination writer behind the PutRecord/PutRecordBatch handlers) must flush buffered records to the destination bucket when a BufferingHints threshold is met, writing the object key as the configured Prefix followed by the service-appended yyyy/MM/dd/HH/ segments, and must keep the stream reaching ACTIVE so the data plane is reachable.

Traces: athena__firehose__glue__s3.firehose-raw-json-delivery-queryable-via-athena-recursive-prefix. The check's later Athena assertions were never reached, so this issue claims nothing about the query engine; only the Firehose-to-S3 delivery is filed here.

## Evidence
Replay of simparity-behavioral-ga-prod-v1 against this checkout (23/223 checks passed).

athena__firehose__glue__s3.firehose-raw-json-delivery-queryable-via-athena-recursive-prefix [mismatch]: "Error: Check timed out after 60.0s".

The check's sequence before the timeout: create bucket -> glue:CreateDatabase/CreateTable at s3://<bucket>/<stream>/ -> athena:CreateWorkGroup -> iam role -> firehose:CreateDeliveryStream(DeliveryStreamType='DirectPut', ExtendedS3DestinationConfiguration={Prefix:'<stream>/', ErrorOutputPrefix:'err/!{firehose:error-output-type}/', BufferingHints:{SizeInMBs:1,IntervalInSeconds:60}, CompressionFormat:'UNCOMPRESSED'}) -> poll describe_delivery_stream for ACTIVE (300s budget) -> 5x firehose:PutRecord of one JSON line each (180s retry budget) -> poll s3:ListObjectsV2(Prefix='<stream>/') for len(keys) > 0 (420s budget). No non-poll call raised, so the run was inside one of those three Firehose polls when the 60s harness budget expired; expected per AWS is at least one key matching '^[^/]+/\d{4}/\d{2}/\d{2}/\d{2}/' and an empty 'err/' prefix.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay simparity-behavioral-ga-prod-v1, re-running the check id in trace_ids.
2. Create an S3 bucket and an IAM role trusted by firehose.amazonaws.com with read/write on it.
3. firehose:CreateDeliveryStream(DeliveryStreamName=<name>, DeliveryStreamType='DirectPut', ExtendedS3DestinationConfiguration={'RoleARN':<role>, 'BucketARN':'arn:aws:s3:::<bucket>', 'Prefix':'<name>/', 'ErrorOutputPrefix':'err/!{firehose:error-output-type}/', 'BufferingHints':{'SizeInMBs':1,'IntervalInSeconds':60}, 'CompressionFormat':'UNCOMPRESSED'}).
4. Poll firehose:DescribeDeliveryStream until DeliveryStreamStatus == 'ACTIVE'.
5. firehose:PutRecord five times with one JSON object per record (newline terminated).
6. Poll s3:ListObjectsV2(Bucket=<bucket>, Prefix='<name>/') — expect, within the buffering interval, at least one key of the form '<name>/yyyy/MM/dd/HH/<object>' containing the records, and s3:ListObjectsV2(Prefix='err/') to stay empty.
7. Observed: the prefix stays empty for the whole budget; the check never progresses to its Athena read-back.
