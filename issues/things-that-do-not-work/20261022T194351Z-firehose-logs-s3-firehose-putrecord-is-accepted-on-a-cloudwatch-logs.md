Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose__logs__s3.cloudwatch-log-processing-stream-rejects-direct-put-record-from-non-cloudwatch-source
Issue-Key: firehose__logs__s3.cloudwatch-log-processing-stream-rejects-direct-put-record-from-non-cloudwatch-source|_error_code#43|AssertionError|raise AssertionError(
Issue-Commit: 17f16131651e3a76635c083c093a8581359ec045

# Firehose PutRecord is accepted on a CloudWatch-Logs-processing stream instead of being refused with InvalidSourceException

> _Note: this issue was identified against the code base at commit `17f16131651e3a76635c083c093a8581359ec045`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRecord

## Description
Cause 1 of 1: a direct PutRecord to a stream whose ProcessingConfiguration declares the CloudWatchLogProcessing processor is accepted.
AWS: Smithy models InvalidSourceException (client, HTTP 400) on PutRecord, and AWS raises it for a put that does not originate from CloudWatch when the stream's ProcessingConfiguration declares CloudWatchLogProcessing. Recorded in us-west-2: 400 InvalidSourceException with 'Put to Firehose failed for AccountId: <acct>, FirehoseName: <name> because the request is not originating from CloudWatch.' The gate is a property of the stream's processing configuration, not of the payload.
Emulator: the call returns HTTP 200 with a RecordId and Encrypted=false, i.e. the source gate does not exist and the record is accepted into the stream.
Why: a client cannot distinguish a log-processing stream from a general-purpose one — code that relies on the refusal to detect mis-targeted producers never sees it, and arbitrary non-CloudWatch payloads enter a stream whose decompression/envelope-stripping pipeline is only valid for CloudWatch Logs records, so the delivered objects differ from AWS's.
Fix: a source check in the PutRecord (and PutRecordBatch) handler of the Firehose service crate: when the stream's ProcessingConfiguration contains the CloudWatchLogProcessing processor, a direct API put must be refused with 400 InvalidSourceException.
Traces: firehose__logs__s3.cloudwatch-log-processing-stream-rejects-direct-put-record-from-non-cloudwatch-source.

## Evidence
Expected: 400 InvalidSourceException. Actual: AssertionError: the call was expected to be rejected with InvalidSourceException and instead SUCCEEDED, returning {'RecordId': '000000000000000018e0f06b42cf40b70000000000000001', 'Encrypted': False, 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}.

## Steps to Reproduce
1. Boot the emulator and point botocore at it.
2. Create an S3-destination delivery stream whose ExtendedS3DestinationConfiguration.ProcessingConfiguration is Enabled with a Processors entry of Type 'CloudWatchLogProcessing' (DataMessageExtraction).
3. Wait for ACTIVE, then firehose.put_record(DeliveryStreamName=<stream>, Record={'Data': b'<any payload>'}) -> expect 400 InvalidSourceException; observe 200 with a RecordId.
4. Re-replay the check id in trace_ids.
