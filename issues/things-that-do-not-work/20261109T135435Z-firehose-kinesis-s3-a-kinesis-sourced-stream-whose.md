Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose__kinesis__s3.cfn-deploy-firehose-missing-kinesis-permission-negative
Issue-Key: firehose__kinesis__s3.cfn-deploy-firehose-missing-kinesis-permission-negative|_poll_until#16|AssertionError|raise AssertionError(
Issue-Commit: 3b4b4e2df28997ae7cbb85154b0c83cf06a43ef8

# firehose__kinesis__s3: a Kinesis-sourced stream whose role lacks kinesis:GetRecords logs no Kinesis.AccessDenied record to its CloudWatchLoggingOptions log stream

> _Note: this issue was identified against the code base at commit `3b4b4e2df28997ae7cbb85154b0c83cf06a43ef8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeliveryStream

## Description
=== Cause 1 of 1: a failed source read is never reported as a Kinesis.AccessDenied log record ===
Failing check: firehose__kinesis__s3.cfn-deploy-firehose-missing-kinesis-permission-negative.
AWS: for a KinesisStreamAsSource delivery stream, Firehose assumes the configured RoleARN to call kinesis:GetRecords/GetShardIterator on the source stream. When that permission is missing the read fails and, with CloudWatchLoggingOptions.Enabled=true, Firehose writes a JSON log event into the configured LogGroupName/LogStreamName whose "errorCode" field is "Kinesis.AccessDenied" (alongside an errorMessage naming the denied operation and the role) — that log record is the only way the failure is observable, since nothing is delivered to the destination. The fixed part of the record is the errorCode token "Kinesis.AccessDenied"; the stream/role/ARN values in the message come from the stream's configuration.
Emulator: no log event is ever written. The check polls the stack's log group/log stream for any event carrying an "errorCode" and finds none: "Kinesis read error logged for <stack>-LogGroup-…/<stack>-BadLogStream-…: not met within 180.0s. Last result: None". The CloudFormation stack deploys and the streams are created, so the configuration is accepted and the error surface is simply absent.
Why: this log record is the documented diagnosis path for a Firehose source-permission problem. On the emulator a stream whose role cannot read its Kinesis source looks indistinguishable from an idle healthy stream: no delivery, no error, nothing to alarm on — so the "missing kinesis:GetRecords" boundary (and the subsequent repair, which should restore delivery) cannot be observed at all.
Fix: the Firehose Kinesis-source reader must evaluate the stream's RoleARN against the IAM service's state before reading, and on an authorization failure emit a delivery-error record to the destination's CloudWatchLoggingOptions log stream with errorCode "Kinesis.AccessDenied" and an errorMessage naming the refused call, the source stream ARN and the role. Related open issue 20261104T055008Z-firehose-logs-s3-cloudwatchloggingoptions-never-emits-a-delivery.md reports the same missing surface for the S3 destination error under a different service token and does not carry this check's replay key.
Traces: the check id above.

## Evidence
Replayed suite: simparity-behavioral-aws-v1 — firehose__kinesis__s3 18/23 (78%), 5 failing checks of which this one is marked neither ALREADY FILED nor OWNED.

Expected (AWS): within 180 s of records arriving on the source Kinesis stream, the bad stream's CloudWatch log stream contains an event whose message carries "errorCode": "Kinesis.AccessDenied"; the same-stack sibling whose role has kinesis:GetRecords delivers to S3, and granting the permission in place restores delivery for the bad stream.
Actual (emulator): "AssertionError: Kinesis read error logged for sim2real-xsvc-stk-…-LogGroup-…/sim2real-xsvc-stk-…-BadLogStream-…: not met within 180.0s. Last result: None" — the log-event scan (regex '"errorCode"\s*:\s*"([^"]+)"' over get_log_events) never matched anything, so the expected code 'Kinesis.AccessDenied' could not even be compared.

## Steps to Reproduce
1. Boot the emulator and point botocore at it.
2. Deploy a CloudFormation stack containing: an S3 bucket; a Kinesis stream; a CloudWatch log group with two log streams; two AWS::KinesisFirehose::DeliveryStream resources of type KinesisStreamAsSource over the same Kinesis stream, each with S3DestinationConfiguration {BufferingHints {IntervalInSeconds: 0, SizeInMBs: 1}, CloudWatchLoggingOptions {Enabled: true, LogGroupName: <group>, LogStreamName: <its own stream>}} — one role with kinesis:GetRecords/GetShardIterator/DescribeStream and one role with everything EXCEPT kinesis:GetRecords.
3. kinesis.put_records of a few records into the source stream.
4. Poll logs.get_log_events(logGroupName=<group>, logStreamName=<bad stream>, startFromHead=True) for up to 180 s and extract "errorCode" from each message.
5. Expect an event with errorCode == 'Kinesis.AccessDenied'; observe no events at all (None).
6. Also check that the permitted sibling delivered to S3 while the denied one wrote nothing, then grant kinesis:GetRecords in place and expect delivery to start.
