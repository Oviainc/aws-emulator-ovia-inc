Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose__logs__s3.subscription-filter-to-firehose-decompression-strips-cwl-envelope
Issue-Key: firehose__logs__s3.subscription-filter-to-firehose-decompression-strips-cwl-envelope|run#53|TimeoutError|raise TimeoutError("both streams' payloads landing in S3: the run budget ended first; not graded")
Issue-Commit: 8fe1b88606c77dfa59d771b1a110885b6dfaed56

# firehose__logs__s3: a CloudWatch Logs subscription filter to a Firehose delivery stream delivers nothing to S3 (1 cause)

> _Note: this issue was identified against the code base at commit `8fe1b88606c77dfa59d771b1a110885b6dfaed56`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutSubscriptionFilter

## Description
ONE root cause. 1 CAUSE must be closed before this issue is submitted.

Cause 1 of 1: log events put into a log group that has a subscription filter whose destination is a Firehose delivery stream never reach the delivery stream's S3 destination — neither the stream with Decompression enabled nor the untouched twin stream ever produces an object under its S3 prefix. Failing check: firehose__logs__s3.subscription-filter-to-firehose-decompression-strips-cwl-envelope.

AWS: recorded behavior only for the delivery path (the hop is a side effect no response field reports). Recorded: PutSubscriptionFilter(logGroupName, filterPattern, destinationArn=<firehose stream ARN>, roleArn=<role Firehose/logs can assume>) is accepted; PutLogEvents into that group then causes CloudWatch Logs to forward the matching events to the delivery stream as a gzip-framed CloudWatch Logs envelope (a JSON document with owner/logGroup/logStream/messageType/logEvents, gzip magic 0x1f 0x8b); with zero buffering hints the stream writes an object under its ExtendedS3DestinationConfiguration prefix within the delivery window. The two streams differ only in ProcessingConfiguration/Decompression: on the stream with decompression enabled the delivered object holds the DECOMPRESSED records with the CloudWatch Logs envelope stripped (the bare `message` payloads), while the untouched stream's object holds the gzip-framed envelopes concatenated back to back. The requirement is that BOTH streams produce objects, and the difference between their payloads; bucket names, prefixes, stream names and object keys are per-run values.

Emulator: no object of any kind appears under either stream's S3 prefix. The replay dies in run() at line 326 with `TimeoutError: both streams' payloads landing in S3: the run budget ended first; not graded` — i.e. the poll for the first delivered object never succeeds inside its 360 s budget. Every control-plane call upstream is accepted (CreateDeliveryStream for both streams, the streams reaching ACTIVE, PutSubscriptionFilter, PutLogEvents), so the missing piece is the CloudWatch Logs -> Firehose forwarding side effect and/or the Firehose -> S3 delivery side effect, not request validation. Note the sibling check in this group is separately blocked by the unimplemented FilterLogEvents stub, which is already filed; this cause is the delivery pipeline, not that stub.

Why: the single most common log-shipping topology in AWS (log group -> subscription filter -> Firehose -> S3) is accepted at every configuration call and silently delivers nothing. A client sees a healthy ACTIVE delivery stream and an installed subscription filter and an empty bucket, with no error and no metric to distinguish "still buffering" from "never". Anything that reads the delivered payload — and in particular anything that depends on Firehose's Decompression feature stripping the CloudWatch Logs envelope — cannot be exercised at all.

Fix: the forwarding side effect of the CloudWatch Logs subscription-filter state (the PutSubscriptionFilter handler's destination wiring plus the PutLogEvents path that must publish matching events to a Firehose destination as a gzip-framed CWL envelope) and the Firehose delivery task behind ExtendedS3DestinationConfiguration that must write buffered records to S3, including the ProcessingConfiguration/Decompression transform that strips the envelope.

Traces: firehose__logs__s3.subscription-filter-to-firehose-decompression-strips-cwl-envelope (frame run() line 326).

## Evidence
Expected (AWS): after PutLogEvents of 12 events into the subscribed log group, ListObjectsV2 on each delivery stream's prefix eventually returns at least one key; GetObject on the untouched stream's object returns gzip-framed bytes (first two bytes 0x1f 0x8b) that gunzip to CloudWatch Logs envelope JSON documents (owner/logGroup/logStream/messageType/logEvents), while the decompression-enabled stream's object holds the decompressed records with the envelope stripped.

Actual (emulator):
  [error] firehose__logs__s3.subscription-filter-to-firehose-decompression-strips-cwl-envelope
  File "check_firehose__logs__s3_subscription_filter_to_firehose_decompression_strips_cwl_envelope.py", line 326, in run
    raise TimeoutError("both streams' payloads landing in S3: the run budget ended first; not graded")
  TimeoutError: both streams' payloads landing in S3: the run budget ended first; not graded
Zero objects under either prefix. Both delivery streams were created with zero buffering hints and reached ACTIVE, and the check allows a 360 s delivery budget (_DELIVERY_BUDGET_S), so this is a pipeline that never delivers rather than one that is slow.

## Steps to Reproduce
1. Boot the emulator from mainline.
2. Create an S3 bucket and a role Firehose and CloudWatch Logs can assume with write access to it.
3. CreateDeliveryStream twice with DeliveryStreamType=DirectPut and ExtendedS3DestinationConfiguration pointing at distinct prefixes in that bucket, one with ProcessingConfiguration/Decompression enabled and one untouched; wait for both to reach ACTIVE.
4. CreateLogGroup/CreateLogStream, then PutSubscriptionFilter for each stream with destinationArn=<delivery stream ARN> and the delivery roleArn.
5. PutLogEvents 12 events matching the filter pattern.
6. Poll ListObjectsV2 under both prefixes: no object ever appears (expected at least one per stream, gzip-framed CWL envelopes on the untouched stream and envelope-stripped records on the decompression stream).
7. Replay: firehose__logs__s3.subscription-filter-to-firehose-decompression-strips-cwl-envelope.
