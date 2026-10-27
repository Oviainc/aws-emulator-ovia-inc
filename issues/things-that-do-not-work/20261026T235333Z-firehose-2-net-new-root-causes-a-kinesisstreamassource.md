Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose.kinesis-source-preserves-sequence-number-order firehose.put-record-rejected-on-kinesis-source-stream firehose.put-record-on-kinesis-source-stream-rejected
Issue-Shape-Id: com.amazonaws.firehose#Firehose_20150804
Issue-Key: firehose.kinesis-source-preserves-sequence-number-order|_poll_s3_objects#20|Timeout|time.sleep(interval_s)
Issue-Key: firehose.put-record-rejected-on-kinesis-source-stream|_poll_s3_for_marker#26|Timeout|time.sleep(interval_s)
Issue-Key: firehose.put-record-on-kinesis-source-stream-rejected|run#28|AssertionError|assert samples >= 3, f"observation window produced too few samples ({samples}) to trust the absence"
Issue-Commit: da7bc09c8c1c199d36cb9fc39c5c5796d768dc3a

# firehose: 2 net-new root causes — a KinesisStreamAsSource stream never ingests its source Kinesis stream (nothing reaches S3), and S3 delivery flushes before BufferingHints.IntervalInSeconds elapses

> _Note: this issue was identified against the code base at commit `da7bc09c8c1c199d36cb9fc39c5c5796d768dc3a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRecord

## Description
TWO net-new root causes in the Firehose data plane, both in the firehose service. All three grounding checks now get PAST the direct-put source gate: the InvalidArgumentException refusal of PutRecord/PutRecordBatch on a KinesisStreamAsSource stream is asserted by all three and no longer fails (firehose.put-record-on-kinesis-source-stream-rejected fails at a later assertion frame, line 80 of its own file, having already asserted the code and the exact refusal message). What fails now is what happens after the refusal.

=== Cause 1 of 2: a KinesisStreamAsSource delivery stream never consumes its backing Kinesis stream, so records put to Kinesis never become S3 objects ===
Failing checks: firehose.kinesis-source-preserves-sequence-number-order, firehose.put-record-rejected-on-kinesis-source-stream.
AWS: Smithy (model_path services/service-com-amazonaws-firehose-firehose-20150804/smithy-model/com-amazonaws-firehose-firehose-20150804.json) KinesisStreamSourceConfiguration carries @required KinesisStreamARN (length 1..512, pattern ^arn:.*:kinesis:[a-zA-Z0-9\\-]+:\\d{12}:stream/[a-zA-Z0-9_.-]+$) and @required RoleARN, documented as "The stream and role Amazon Resource Names (ARNs) for a Kinesis data stream used as the source for a Firehose stream." Firehose reads from that stream with that role and delivers to the configured destination; direct puts are disabled precisely because the Kinesis stream is the only ingestion path (docs.aws.amazon.com/firehose/latest/dev/writing-with-kinesis-streams.html). Recorded behavior: after kinesis.PutRecord to the source stream, the payload appears as an object under the ExtendedS3/S3 destination's Prefix once the destination's BufferingHints threshold is met, and for a single-shard stream fed on one partition key the per-shard put order is preserved in the delivered bytes.
Emulator: nothing is ever delivered on that path. Both checks consume their whole budget waiting at the destination poll and are cut off by the harness ("Error: Check timed out after 60.0s"); neither reaches an assertion about the delivered content. firehose.put-record-rejected-on-kinesis-source-stream has already passed both refusal assertions (PutRecord and PutRecordBatch, code and message) before it blocks on its positive control — kinesis.put_record to the backing stream followed by a read at the S3 destination. firehose.kinesis-source-preserves-sequence-number-order blocks the same way (its own _poll_s3_objects budget is 240 s with a 60 s buffering hint). The control plane answers immediately in this run (stream ACTIVE, Source.KinesisStreamSourceDescription echoed), so the budget is consumed by the data-plane wait, not by provisioning.
Why: KinesisStreamAsSource is the standard "tee a Kinesis stream into S3" topology. Against the emulator the stream reports ACTIVE, echoes its source, and correctly refuses direct puts — it looks completely healthy — while no data ever arrives, so every consumer built on it (ordering, partitioning, de-aggregation, downstream Glue/Athena) reads an empty destination. A client cannot distinguish "configured wrongly" from "not implemented", and any test that asserts data IS delivered can never fail-closed.
Fix: the ingestion side of the Firehose service's delivery pipeline. A stream created with DeliveryStreamType=KinesisStreamAsSource must register a consumer against the named Kinesis stream in the Kinesis service's state (assuming KinesisStreamSourceConfiguration.RoleARN) and feed the records it reads into the same buffer/flush path that direct puts use, preserving per-shard order, so the destination writer produces objects for Kinesis-originated records exactly as it does for DirectPut ones.
Traces: firehose.kinesis-source-preserves-sequence-number-order, firehose.put-record-rejected-on-kinesis-source-stream.
Relationship to open issues: 20261026T074043Z-firehose-kinesis-s3-a-kinesisstreamassource-delivery-stream-never.md describes this same behavior but is filed under the service token firehose__kinesis__s3 and carries that suite's check ids, not these two; 20261025T002604Z-firehose-putrecord-putrecordbatch-are-accepted-with-200-on.md does list firehose.kinesis-source-preserves-sequence-number-order, but its replay key is that check's _error_code frame (the refusal), which now PASSES — the check has moved on to the destination poll, which is this cause. The replay report marks neither of these two checks as OWNED.

=== Cause 2 of 2: S3 delivery flushes long before the configured BufferingHints.IntervalInSeconds elapses ===
Failing check: firehose.put-record-on-kinesis-source-stream-rejected.
AWS: Smithy BufferingHints.IntervalInSeconds (@range 0..900, default 300) is "Buffer incoming data for the specified period of time, in seconds, before delivering it to the destination"; BufferingHints.SizeInMBs (@range 1..128, default 5) is the size threshold. Firehose delivers when one threshold is reached, so with IntervalInSeconds=60 and a payload of a few dozen bytes (far below SizeInMBs=1) the single record is delivered at roughly the 60 s mark, not within the first seconds.
Emulator: the record is delivered almost immediately. The check puts one small record into a DirectPut stream created with BufferingHints={'IntervalInSeconds': 60, 'SizeInMBs': 1}, then polls the destination every 10 s until the marker appears; the marker was already present on the second poll, so the loop exited with only 2 observations — "AssertionError: observation window produced too few samples (2) to trust the absence" (AWS needs ~7 polls across the 60 s interval to satisfy the same assertion). The hints are accepted and echoed but do not gate the flush.
Why: batching and latency are what Firehose exists for. A pipeline tuned on the emulator sees one object per record at zero latency, so object counts, object sizes, small-file costs downstream and any test asserting "not yet delivered" invert in production; conversely a test asserting "delivered only after the interval" can never catch a regression. Here it also destroys a negative control: the window the check uses to prove the REFUSED record never lands is sized from the accepted record's measured latency, so an immediate flush collapses the window and the absence proof with it.
Fix: the flush scheduler in the Firehose service crate must hold accepted records until the destination's BufferingHints.SizeInMBs or IntervalInSeconds threshold is met (applying the modeled defaults 5 MiB / 300 s when absent) and only then write one object per flushed batch.
Traces: firehose.put-record-on-kinesis-source-stream-rejected.
Relationship to open issues: this is the same root cause as Cause 2 of open issue 20261025T083505Z-firehose-s3-delivery-ignores-bufferinghints-the-destination-role.md ("BufferingHints are ignored — every put is flushed to S3 immediately"). That issue does not carry this check id and the replay report does not mark this check OWNED by it, so the replay key is filed here; a fixer closing that issue's Cause 2 should re-replay this check id as well and may close the two together.

## Evidence
Replayed suite: simparity-behavioral-ga-prod-v1, firehose 109/139 (78%), 30 failures — 27 marked OWNED by open issues, these 3 not.

Cause 1 — Kinesis-sourced ingestion never reaches the destination
Expected (AWS): create_delivery_stream(DeliveryStreamType='KinesisStreamAsSource', KinesisStreamSourceConfiguration={'KinesisStreamARN': KA, 'RoleARN': R}, S3DestinationConfiguration={'RoleARN': R, 'BucketARN': 'arn:aws:s3:::B', 'BufferingHints': {'SizeInMBs': 1, 'IntervalInSeconds': 60}}); kinesis.put_record(StreamName=K, Data=X, PartitionKey='k') -> within roughly one buffer interval an object under B contains X; with 8 records on one partition key the delivered lines are in put order.
Actual (emulator): no object ever appears; the check is cut off by the harness budget —
  firehose.kinesis-source-preserves-sequence-number-order: "Error: Check timed out after 60.0s" (its destination poll _poll_s3_objects(min_count=1, timeout_s=240.0) never satisfies; the preceding assertions on Source.KinesisStreamSourceDescription.KinesisStreamARN/RoleARN and on the InvalidArgumentException refusal of PutRecord all passed).
  firehose.put-record-rejected-on-kinesis-source-stream: "Error: Check timed out after 60.0s" (both refusal assertions — PutRecord and PutRecordBatch, code InvalidArgumentException and message "This operation is not permitted on KinesisStreamAsSource delivery stream type." — passed; it blocks on its positive control, a kinesis.put_record read back at the S3 destination).
Corroboration that the wait is the emulator's and not a provisioning artifact: control-plane calls in this run answer immediately (both checks reached and passed their post-ACTIVE assertions), and the sibling suite's checks for the same topology fail with an explicit "no object" assertion (open issue 20261026T074043Z).

Cause 2 — buffer interval not honoured
Expected (AWS): with BufferingHints={'IntervalInSeconds': 60, 'SizeInMBs': 1} and a ~50-byte record, the destination has no object for most of the first minute; polling every 10 s until it appears yields about 7 observations.
Actual (emulator): firehose.put-record-on-kinesis-source-stream-rejected — "AssertionError: observation window produced too few samples (2) to trust the absence" at check line 80, i.e. the accepted record was readable at the destination on the second 10 s poll (between 10 s and 20 s after the put) instead of at ~60 s. The two assertions before it (code InvalidArgumentException and the exact message "This operation is not permitted on KinesisStreamAsSource delivery stream type." for PutRecord on the Kinesis-sourced stream) passed, and the positive control's delivery itself succeeded — only its timing is wrong.

## Steps to Reproduce
Common setup: boot the emulator, point botocore at it, s3.create_bucket(Bucket=B), iam.create_role(R) trusted by firehose.amazonaws.com with s3:PutObject/s3:AbortMultipartUpload on arn:aws:s3:::B and B/* plus kinesis:Get*/DescribeStream on the source stream.

Cause 1 (Kinesis-sourced ingestion):
1. kinesis.create_stream(StreamName=K, ShardCount=1); wait for StreamStatus=ACTIVE; note StreamARN=KA.
2. firehose.create_delivery_stream(DeliveryStreamName=S, DeliveryStreamType='KinesisStreamAsSource', KinesisStreamSourceConfiguration={'KinesisStreamARN': KA, 'RoleARN': R}, S3DestinationConfiguration={'RoleARN': R, 'BucketARN': 'arn:aws:s3:::B', 'BufferingHints': {'SizeInMBs': 1, 'IntervalInSeconds': 60}}); wait for DeliveryStreamStatus=ACTIVE.
3. Confirm the refusal still holds (it does today): firehose.put_record(DeliveryStreamName=S, Record={'Data': b'x'}) -> 400 InvalidArgumentException, message 'This operation is not permitted on KinesisStreamAsSource delivery stream type.'; same for put_record_batch.
4. kinesis.put_record(StreamName=K, Data=b'kseq-000\n', PartitionKey='fixed-key'); repeat for kseq-001..kseq-007 on the same partition key.
5. Poll s3.list_objects_v2(Bucket=B) for up to 240 s. Expect at least one object whose concatenated body is the 8 lines in put order; observe no object at all.

Cause 2 (buffer interval):
1. firehose.create_delivery_stream(DeliveryStreamName=D, DeliveryStreamType='DirectPut', S3DestinationConfiguration={'RoleARN': R, 'BucketARN': 'arn:aws:s3:::B', 'BufferingHints': {'IntervalInSeconds': 60, 'SizeInMBs': 1}}); wait for ACTIVE.
2. firehose.put_record(DeliveryStreamName=D, Record={'Data': json.dumps({'marker': M}).encode()}) and record t0.
3. Poll s3.list_objects_v2(Bucket=B) every 10 s, counting observations, until M is found.
4. Expect M to appear at roughly t0+60 s (about 7 observations); observe it on the second observation, 10-20 s after t0.

Finally re-replay: firehose.kinesis-source-preserves-sequence-number-order, firehose.put-record-rejected-on-kinesis-source-stream, firehose.put-record-on-kinesis-source-stream-rejected.
