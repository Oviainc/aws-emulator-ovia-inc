Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose__logs__s3.cloudwatch-log-processing-stream-rejects-direct-put-record-from-non-cloudwatch-source
Issue-Key: firehose__logs__s3.cloudwatch-log-processing-stream-rejects-direct-put-record-from-non-cloudwatch-source|run#63|AssertionError|assert gated_keys and open_keys, (
Issue-Commit: 86c9f1e4438a61fbe37b19dfcd3d77efce3d0679

# A Firehose delivery stream writes no objects to its S3 destination, so delivery and its absence are both unobservable (1 cause)

> _Note: this issue was identified against the code base at commit `86c9f1e4438a61fbe37b19dfcd3d77efce3d0679`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRecord

## Description
Cause 1 of 1 — records put into a Firehose delivery stream with an S3 destination never land in the bucket.
AWS: Smithy (com.amazonaws.firehose) PutRecord/PutRecordBatch on a delivery stream with an ExtendedS3DestinationConfiguration. Recorded behavior only for the delivery semantics: accepted records are buffered and written as objects under the destination prefix within the stream's buffering hints, so a reader of the bucket sees at least one object per prefix that received data. That delivery is what makes the NEGATIVE claim readable: a stream configured for CloudWatch-Logs sources must not deliver a directly-put, non-CloudWatch record, and the only way to tell that refusal from delivery lag is that a sibling, unprocessed stream's prefix DOES receive its object.
Emulator: the CloudWatchLogProcessing stream's prefix holds 0 objects after the check's 50-second delivery window while the unprocessed stream's prefix holds 1, so the data plane delivers inconsistently and the asserted absence cannot be distinguished from lag.
Why: a client that writes through Firehose to S3 cannot observe its own data arriving, and — because an absence is indistinguishable from a slow delivery — nothing downstream can assert either that a record was delivered or that it was correctly dropped.
Fix: the S3 delivery path of the firehose crate (the buffering/flush that writes objects under the destination prefix), for the CloudWatch-Logs-source stream configuration as well as the plain one.
Traces: firehose__logs__s3.cloudwatch-log-processing-stream-rejects-direct-put-record-from-non-cloudwatch-source

One cause must be closed before submitting.

## Evidence
From the simparity-behavioral-aws-v1 replay against this checkout.

cloudwatch-log-processing-stream-rejects-direct-put-record-from-non-cloudwatch-source:
  AssertionError: DATAPLANE_UNIMPLEMENTED: no delivery in 50s. The CloudWatchLogProcessing stream's prefix holds 0 objects and the unprocessed stream's holds 1. Without delivery the absence asserted below would be unreadable -- it could be lag rather than the rejection

The 50 s window is the check's ceiling, not an AWS latency requirement; the divergence is that one stream's destination prefix never receives an object at all.

## Steps to Reproduce
Boot the emulator from this checkout.

1. aws s3api create-bucket --bucket b
2. Create two Firehose delivery streams with ExtendedS3DestinationConfiguration into bucket b under distinct prefixes — one configured for a CloudWatch Logs source, one plain.
3. aws firehose put-record --delivery-stream-name <each> --record Data=<base64 of a plain json line>
4. aws s3api list-objects-v2 --bucket b --prefix <each prefix>
-> the plain stream's prefix gains an object; the CloudWatch-Logs-source stream's prefix stays empty. AWS delivers for both configurations (and drops only the record content a CloudWatch-Logs-source stream must not process).

Then re-replay: firehose__logs__s3.cloudwatch-log-processing-stream-rejects-direct-put-record-from-non-cloudwatch-source
