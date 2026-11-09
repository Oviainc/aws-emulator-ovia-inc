Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose.buffering-hints-delay-controls-delivery-timing firehose.buffering-hints-delay-delivery-to-s3 firehose.custom-time-zone-shifts-the-timestamp-prefix-expansion firehose.record-de-aggregation-json-splits-one-put-across-jq-derived-prefixes
Issue-Shape-Id: com.amazonaws.firehose#Firehose_20150804
Issue-Key: firehose.buffering-hints-delay-controls-delivery-timing|run#11|AssertionError|assert early_keys == [], (
Issue-Key: firehose.buffering-hints-delay-delivery-to-s3|run#12|AssertionError|assert not early_objs, (
Issue-Key: firehose.custom-time-zone-shifts-the-timestamp-prefix-expansion|_suffix_datetime#34|AssertionError|assert match is not None, (
Issue-Key: firehose.record-de-aggregation-json-splits-one-put-across-jq-derived-prefixes|_poll_until#20|AssertionError|raise AssertionError(
Issue-Commit: 3b4b4e2df28997ae7cbb85154b0c83cf06a43ef8

# firehose: 3 net-new causes — BufferingHints.IntervalInSeconds is ignored, the delivered object name carries an extra numeric segment, and dynamic partitioning with RecordDeAggregation/MetadataExtraction delivers nothing

> _Note: this issue was identified against the code base at commit `3b4b4e2df28997ae7cbb85154b0c83cf06a43ef8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRecord

## Description
THREE net-new root causes in the Firehose ExtendedS3 delivery path, grounded in firehose.* checks that the suite marks neither ALREADY FILED nor OWNED.

Relationship to open issues: 20261025T083505Z-firehose-s3-delivery-ignores-bufferinghints-the-destination-role.md and 20261029T175617Z-firehose-6-net-new-root-causes-bufferinghints-are.md already carry OTHER buffering/object-name checks; the replay does not mark the three checks below as owned by them, and cause 3 (dynamic partitioning) appears in neither.

=== Cause 1 of 3: BufferingHints.IntervalInSeconds does not gate the flush — every put is written to S3 immediately ===
Failing checks: firehose.buffering-hints-delay-controls-delivery-timing, firehose.buffering-hints-delay-delivery-to-s3.
AWS: Smithy BufferingHints.IntervalInSeconds (@range 0..900, default 300) is "Buffer incoming data for the specified period of time, in seconds, before delivering it to the destination"; BufferingHints.SizeInMBs (@range 1..128, default 5) is the size threshold. Firehose delivers when one of the two thresholds is reached, so a small single record put into a stream configured with IntervalInSeconds=60 (or 120) and SizeInMBs well above the payload is NOT present in the destination bucket while the interval is still running: list_objects_v2 under the prefix returns no Contents until (roughly) the interval has elapsed.
Emulator: the object is present seconds after the put. "record appeared in S3 before the 60s buffering interval elapsed (at t=5.0s): ['2026/11/09/13/<stream>-1-...']" and "record delivered to S3 before the 120s buffering interval elapsed (at t=2s): [{'Key': '2026/11/09/13/<stream>-1-...', 'Size': 15, ...}]". The hints are accepted and echoed by DescribeDeliveryStream but do not delay the flush.
Why: batching and latency are the properties Firehose exists for. A pipeline tuned against the emulator sees one object per record at zero latency, so object counts, object sizes, small-file cost and any test asserting "not yet delivered" invert in production, and a regression in the buffering scheduler can never be caught.
Fix: the flush scheduler in the firehose service crate must hold accepted records until the destination's BufferingHints.SizeInMBs or IntervalInSeconds threshold is reached (applying the modeled defaults 5 MiB / 300 s when absent) and write one object per flushed batch.
Traces: the two check ids above.

=== Cause 2 of 3: the delivered S3 object name inserts an extra numeric segment between the timestamp and the UUID ===
Failing check: firehose.custom-time-zone-shifts-the-timestamp-prefix-expansion.
AWS: the documented default object name is <evaluated prefix><delivery stream name>-<delivery stream version>-yyyy-MM-dd-HH-mm-ss-<uuid><file extension> (docs.aws.amazon.com/firehose/latest/dev/s3-object-name.html): after the evaluated prefix the tail is exactly the stream name, the decimal stream version DescribeDeliveryStream reports as VersionId, ONE second-resolution timestamp and the UUID — nothing between the timestamp and the UUID. The fixed text here is the separator layout and field order; the stream name, version, timestamp and uuid are values taken from the stream and the flush.
Emulator: an additional numeric segment ('-0', apparently a batch/sequence counter) is written between the timestamp and the UUID, so the name does not parse against that format: "delivered key 'tz-utc/d=2026-11-09/h=12/<stream>-1-2026-11-09-12-29-38-0-<uuid>' does not carry the documented object-name suffix '<stream>-<version>-yyyy-MM-dd-HH-mm-ss-<uuid>'". The same shape appears in every delivered key in this run (including the metric-stream exports of other services), so it is systematic rather than per-check, and it blocks this check before its CustomTimeZone assertion can even be evaluated.
Why: consumers parse these names. Glue/Athena partition tooling, log shippers, de-duplication by object name and anything deriving stream/version/time from the key break on the extra segment, and a client cannot tell which configuration version produced an object.
Fix: the object-key builder in the Firehose delivery path must emit <evaluated prefix><stream name>-<version>-yyyy-MM-dd-HH-mm-ss-<uuid><file extension> with no additional segment; per-object uniqueness belongs in the UUID.
Traces: the check id above.

=== Cause 3 of 3: a dynamic-partitioning stream with RecordDeAggregation + MetadataExtraction delivers nothing at all ===
Failing check: firehose.record-de-aggregation-json-splits-one-put-across-jq-derived-prefixes.
AWS: Smithy ProcessorType is an enum including RecordDeAggregation and MetadataExtraction; ProcessorParameterName includes SubRecordType, MetadataExtractionQuery and JsonParsingEngine; DynamicPartitioningConfiguration is "the configuration of the dynamic partitioning mechanism that creates smaller data sets from the streaming data by partitioning it based on partition keys ... only supported for Amazon S3 destinations". With DynamicPartitioningConfiguration.Enabled=true and Processors [RecordDeAggregation(SubRecordType=JSON), MetadataExtraction(MetadataExtractionQuery='{tenant:.tenant}', JsonParsingEngine=JQ-1.6)], one PutRecord carrying four concatenated JSON sub-records is split into four records, the jq query derives each record's tenant partition key, and the Prefix expression '!{partitionKeyFromQuery:tenant}' expands into a directory level — so the four sub-records land under four distinct prefixes <leg>/alpha/, <leg>/bravo/, <leg>/charlie/, <leg>/delta/, with nothing under the ErrorOutputPrefix because every sub-record carries a `tenant` field.
Emulator: nothing is ever written. "DATAPLANE_UNIMPLEMENTED?: all 4 deaggregated prefixes ['deagg/alpha/', 'deagg/bravo/', 'deagg/charlie/', 'deagg/delta/'] appear under s3://<bucket>/deagg/ did not happen within 480.0s. Last observation: []" — the configuration is accepted and echoed (the processor list and its parameters read back correctly) but neither a partitioned object nor an error-prefix object appears.
Why: dynamic partitioning is how Firehose feeds partitioned query layers. On the emulator a partitioned stream is a black hole: no data, no error output, so a client cannot validate partition keys, jq extraction queries, or the error path for a record missing the key.
Fix: the Firehose processing/delivery pipeline must implement the dynamic-partitioning path — apply the declared processors in order (RecordDeAggregation before MetadataExtraction), evaluate the jq MetadataExtractionQuery to partition keys, expand !{partitionKeyFromQuery:<key>} in the Prefix, and route records that fail extraction to the ErrorOutputPrefix.
Traces: the check id above.

## Evidence
Replayed suite: simparity-behavioral-aws-v1 — firehose 156/175 (89%), 19 failing checks of which these three are marked neither ALREADY FILED nor OWNED.

Cause 1 — BufferingHints ignored
Expected (AWS): with IntervalInSeconds=60 (resp. 120) and a payload far below SizeInMBs, the destination bucket has no object while the interval is still running.
Actual (emulator): firehose.buffering-hints-delay-controls-delivery-timing — "AssertionError: record appeared in S3 before the 60s buffering interval elapsed (at t=5.0s): ['2026/11/09/13/sim2real-fh-buf-…-1-2026-11-09-13-07-37-0-…']". firehose.buffering-hints-delay-delivery-to-s3 — "AssertionError: record delivered to S3 before the 120s buffering interval elapsed (at t=2s): [{'Key': '2026/11/09/13/sim2real-behav-frh-…-1-2026-11-09-13-07-47-0-…', 'Size': 15, 'ETag': '\"78a5c91de6cbb826e390186c539d22ce\"'}]".

Cause 2 — object name has an extra segment
Expected (AWS): the key tail matches <stream>-<version>-\d{4}-\d{2}-\d{2}-\d{2}-\d{2}-\d{2}-<uuid>.
Actual (emulator): firehose.custom-time-zone-shifts-the-timestamp-prefix-expansion — "AssertionError: delivered key 'tz-utc/d=2026-11-09/h=12/sim2real-fh-tz-utc-…-1-2026-11-09-12-29-38-0-3211e386-9bcb-4c72-8cd3-a4668c96a108' does not carry the documented object-name suffix '<stream>-<version>-yyyy-MM-dd-HH-mm-ss-<uuid>'; got the tail 'tz-utc-…-1-2026-11-09-12-29-38-0-3211e386-…'" — a '-0' sits between the seconds and the uuid.

Cause 3 — dynamic partitioning delivers nothing
Expected (AWS): the four jq-derived prefixes deagg/alpha/, deagg/bravo/, deagg/charlie/, deagg/delta/ all contain objects, and deagg-err/ is empty.
Actual (emulator): firehose.record-de-aggregation-json-splits-one-put-across-jq-derived-prefixes — "AssertionError: DATAPLANE_UNIMPLEMENTED?: all 4 deaggregated prefixes ['deagg/alpha/', 'deagg/bravo/', 'deagg/charlie/', 'deagg/delta/'] appear under s3://sim2real-fh-deagg-…/deagg/ did not happen within 480.0s. Last observation: []".

## Steps to Reproduce
Common setup: boot the emulator, point botocore at it, s3.create_bucket(Bucket=B), create an IAM role that can write B and is trusted by firehose.amazonaws.com.

Cause 1:
1. firehose.create_delivery_stream(DeliveryStreamType='DirectPut', ExtendedS3DestinationConfiguration={'RoleARN': R, 'BucketARN': 'arn:aws:s3:::B', 'BufferingHints': {'SizeInMBs': 1, 'IntervalInSeconds': 60}, 'CompressionFormat': 'UNCOMPRESSED'}); wait ACTIVE.
2. put_record one small record; poll s3.list_objects_v2(Bucket=B) every second for the first 10 s.
3. Expect no Contents yet; observe the object within ~2–5 s. Repeat with IntervalInSeconds=120.

Cause 2:
1. Create a DirectPut stream with Prefix 'tz-utc/d=!{timestamp:yyyy-MM-dd}/h=!{timestamp:HH}/', put one record and read back the delivered key.
2. Expect the tail '<stream>-<VersionId>-yyyy-MM-dd-HH-mm-ss-<uuid>'; observe an extra '-<n>' between the seconds and the uuid.

Cause 3:
1. Create a DirectPut stream with ExtendedS3DestinationConfiguration Prefix 'deagg/!{partitionKeyFromQuery:tenant}/', ErrorOutputPrefix 'deagg-err/!{firehose:error-output-type}/', BufferingHints {'SizeInMBs': 64, 'IntervalInSeconds': 60}, DynamicPartitioningConfiguration={'Enabled': True} and ProcessingConfiguration={'Enabled': True, 'Processors': [{'Type':'RecordDeAggregation','Parameters':[{'ParameterName':'SubRecordType','ParameterValue':'JSON'}]}, {'Type':'MetadataExtraction','Parameters':[{'ParameterName':'MetadataExtractionQuery','ParameterValue':'{tenant:.tenant}'},{'ParameterName':'JsonParsingEngine','ParameterValue':'JQ-1.6'}]}]}; wait ACTIVE.
2. put_record one blob concatenating four JSON objects whose `tenant` values are alpha/bravo/charlie/delta.
3. Poll s3.list_objects_v2(Bucket=B, Prefix='deagg/') for up to 480 s.
4. Expect objects under all four tenant prefixes and none under 'deagg-err/'; observe an empty bucket.

Finally re-replay the four check ids in trace_ids.
