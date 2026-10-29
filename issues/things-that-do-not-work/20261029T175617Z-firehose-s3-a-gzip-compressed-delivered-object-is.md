Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose__s3.firehose-gzip-compressed-delivery-decompresses-to-the-exact-records
Issue-Key: firehose__s3.firehose-gzip-compressed-delivery-decompresses-to-the-exact-records|run#15|AssertionError|assert head["ContentType"] == "application/octet-stream", (
Issue-Commit: f4e5251e6df88363d8e23ace2ff5bff3be31b423

# firehose__s3: a GZIP-compressed delivered object is written with ContentType application/xml instead of application/octet-stream

> _Note: this issue was identified against the code base at commit `f4e5251e6df88363d8e23ace2ff5bff3be31b423`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRecordBatch

## Description
One net-new root cause.

=== Cause 1 of 1: the Firehose-written S3 object carries the wrong Content-Type ===
AWS: recorded behavior only (CompressionFormat is modelled; the delivered object's HTTP metadata is not). AWS behavior: Firehose writes its destination objects with Content-Type `application/octet-stream` regardless of CompressionFormat — for a GZIP stream the object additionally carries Content-Encoding `gzip` and a `.gz` key suffix, and its bytes decompress to exactly the records that were put.
Emulator: the delivered object's ContentType is `application/xml`. The key suffix is `.gz` as expected, so the compression path itself ran; only the object's declared media type is wrong, and `application/xml` is a default that has nothing to do with Firehose's payload.
Why: a client that reads the object with a content-type-driven reader (or serves it onward, or routes on Content-Type in an analytics/ETL step) is told the gzip bytes are XML and mis-parses or rejects them; HeadObject-based assertions on delivered objects do not replay.
Fix: the Firehose S3 delivery worker's PutObject call — it must set ContentType application/octet-stream (alongside Content-Encoding for compressed formats) instead of letting the S3 default content type apply.
Traces: firehose__s3.firehose-gzip-compressed-delivery-decompresses-to-the-exact-records.

Related open issue 20261025T083505Z-firehose-s3-delivery-ignores-bufferinghints-the-destination-role.md covers the missing Content-Encoding for a declared CompressionFormat; the wrong ContentType on the delivered object is a different member of the same PutObject call and is not carried by that issue's replay keys.

## Evidence
firehose__s3.firehose-gzip-compressed-delivery-decompresses-to-the-exact-records:
  assert head["ContentType"] == "application/octet-stream"
  AssertionError: delivered object 'gz/<YYYY/MM/DD/HH>/<stream>-1-<ts>-0-<uuid>.gz' ContentType was 'application/xml'

Expected: HeadObject on the delivered object reports ContentType 'application/octet-stream'.
Actual: 'application/xml'. (The '.gz' key suffix is correct; the key's date path, stream name, version, timestamp and uuid are per-run values.)

## Steps to Reproduce
1. Clone the repo and boot the emulator (run_behavioral_suite).
2. Create an S3 bucket and a role trusting firehose.amazonaws.com with s3:PutObject on it.
3. CreateDeliveryStream (DirectPut) with ExtendedS3DestinationConfiguration {BucketARN, RoleARN, Prefix: 'gz/', CompressionFormat: 'GZIP', BufferingHints {SizeInMBs: 1, IntervalInSeconds: 60}}; wait ACTIVE.
4. PutRecordBatch a few records and wait for the delivered object under gz/.
5. HeadObject the delivered key and read ContentType — it is 'application/xml' instead of 'application/octet-stream'.
