Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose__logs__s3.cloudwatch-log-processing-stream-rejects-direct-put-record-from-non-cloudwatch-source
Issue-Key: firehose__logs__s3.cloudwatch-log-processing-stream-rejects-direct-put-record-from-non-cloudwatch-source|run#58|Timeout|time.sleep(_POLL_INTERVAL_S)
Issue-Commit: f4e5251e6df88363d8e23ace2ff5bff3be31b423

# firehose__logs__s3: a CloudWatch Logs subscription filter to a Firehose stream still delivers nothing to S3, so the CloudWatchLogProcessing source gate (InvalidSourceException on a direct PutRecord) can never be graded

> _Note: this issue was identified against the code base at commit `f4e5251e6df88363d8e23ace2ff5bff3be31b423`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutSubscriptionFilter

## Description
One net-new root cause.

=== Cause 1 of 1: a CloudWatch Logs subscription filter whose destination is a Firehose delivery stream never delivers the log events, so the stream's CloudWatchLogProcessing source gate is unobservable ===
AWS: recorded behavior only for the delivery hop (PutSubscriptionFilter is modelled, the CloudWatch-Logs-to-Firehose delivery is not). The refusal arm IS modelled: Smithy: PutRecord errors include InvalidSourceException (client, HTTP 400), documented "Only requests from CloudWatch Logs are supported when CloudWatch Logs decompression is enabled." Recorded message for that refusal: `Put to Firehose failed for AccountId: <account-id>, FirehoseName: <stream-name> because the request is not originating from CloudWatch.` — fixed text `Put to Firehose failed for AccountId: `, `, FirehoseName: `, ` because the request is not originating from CloudWatch.` with the caller's account id and the stream name taken from the request. AWS behavior for the whole claim: PutLogEvents into a subscribed log group causes Firehose to receive the gzip'd CloudWatch-Logs DATA_MESSAGE envelope and, after the buffering flush, write it to S3 — on the stream that declares Decompression(GZIP) + CloudWatchLogProcessing(DataMessageExtraction) as well as on the twin that declares no processing — while a DIRECT firehose:PutRecord of the same bytes at the processing stream is refused with InvalidSourceException and never appears in S3.
Emulator: the subscription-filter hop produces nothing at either destination, so the check never gets past proving the refusing stream is alive and is killed at the 60 s cap. Neither the refusal nor the absence-of-the-refused-record claim is ever exercised, so the InvalidSourceException gate remains ungraded even though the resolved issue 20261022T194351Z-firehose-logs-s3-firehose-putrecord-is-accepted-on-a-cloudwatch-logs.md claimed it fixed. This failure is a delivery timeout, not a refusal, so it is reported as a new defect in the delivery hop rather than a reopen of that issue.
Why: a client building the standard CloudWatch Logs -> Firehose -> S3 log-archival pipeline gets an accepted subscription filter and a permanently empty bucket, with no error anywhere to point at. It also cannot rely on the source gate: there is no way to confirm that only CloudWatch-sourced records reach a CloudWatchLogProcessing stream.
Fix: the CloudWatch Logs subscription-filter delivery worker for a `kinesis-firehose` destinationArn — it must assume the filter's roleArn and hand the gzip'd DATA_MESSAGE envelope to the Firehose delivery stream so the stream's processing/decompression stage and its S3 destination can be exercised.
Traces: firehose__logs__s3.cloudwatch-log-processing-stream-rejects-direct-put-record-from-non-cloudwatch-source.

Related open issues: 20261029T072224Z-firehose-logs-s3-a-cloudwatch-logs-subscription-filter.md owns the sibling decompression check and 20261024T005347Z-firehose-logs-s3-firehose-cloudwatch-logs-integration-produces-nothing-subscription-filter.md describes the same family; neither carries this check's replay key.

## Evidence
firehose__logs__s3.cloudwatch-log-processing-stream-rejects-direct-put-record-from-non-cloudwatch-source: Error: Check timed out after 60.0s.

The check creates two delivery streams differing in exactly one thing (one declares ProcessingConfiguration with Decompression GZIP + CloudWatchLogProcessing DataMessageExtraction, the other declares no processing), puts a subscription filter from a log group to each, writes 12 log events, and only AFTER proving delivery is flowing into both destinations asserts that a direct firehose:PutRecord of the same bytes at the processing stream is refused with Error.Code InvalidSourceException and never appears in S3.

Expected: both prefixes receive the subscription-filter payload; the direct PutRecord at the processing stream is refused HTTP 400 / InvalidSourceException with message 'Put to Firehose failed for AccountId: <account-id>, FirehoseName: <stream-name> because the request is not originating from CloudWatch.'
Actual: neither prefix ever receives an object, so the run budget ends before the refusal is ever attempted.

## Steps to Reproduce
1. Clone the repo and boot the emulator (run_behavioral_suite).
2. Create an S3 bucket, a role trusting firehose.amazonaws.com with s3:PutObject, and two DirectPut delivery streams with ExtendedS3DestinationConfiguration into different prefixes — one with ProcessingConfiguration {Enabled: true, Processors: [Decompression(GZIP), CloudWatchLogProcessing(DataMessageExtraction)]}, one with no ProcessingConfiguration. Wait ACTIVE.
3. Create a log group and a role trusting logs.amazonaws.com with firehose:PutRecord on both streams; PutSubscriptionFilter from the group to each stream ARN with filterPattern "".
4. PutLogEvents 12 events into the group and poll both S3 prefixes: neither ever receives an object.
5. Separately, call firehose PutRecord directly against the processing stream with the gzip'd DATA_MESSAGE bytes and read Error.Code/Error.Message — this arm is never reached in the suite because step 4 never completes.
