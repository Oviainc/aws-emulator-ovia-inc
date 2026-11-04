Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose__logs__s3.cloudwatch-logging-options-emits-the-s3-access-denied-error-while-the-healthy-twin-logs-nothing
Issue-Key: firehose__logs__s3.cloudwatch-logging-options-emits-the-s3-access-denied-error-while-the-healthy-twin-logs-nothing|_poll_until#24|AssertionError|raise AssertionError(
Issue-Commit: 04fa8901c0470ed069cb581dfd2246db1b5bced9

# firehose__logs__s3: CloudWatchLoggingOptions never emits a delivery-error event, so an S3.AccessDenied delivery failure is invisible

> _Note: this issue was identified against the code base at commit `04fa8901c0470ed069cb581dfd2246db1b5bced9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeliveryStream

## Description
Cause 1 of 1: a delivery stream configured with CloudWatchLoggingOptions publishes no log event when its S3 delivery is refused.
AWS: recorded behavior only — when an ExtendedS3DestinationConfiguration carries CloudWatchLoggingOptions with Enabled=true, a LogGroupName and a LogStreamName, and delivery to the destination bucket is denied, Firehose puts a delivery-error event into that log stream whose JSON carries errorCode 'S3.AccessDenied' and a message naming the refused destination; a healthy twin stream writes nothing to its log group.
Emulator: the configured log group receives nothing at all — polling it for 300 s yields an empty event list, so the error path is indistinguishable from the healthy path.
Why: the only channel through which a client learns that Firehose deliveries are being refused is silent, so a broken delivery stream looks identical to a working one and the diagnosis (errorCode S3.AccessDenied) is unavailable.
Fix: the Firehose delivery-error reporting path in the firehose service crate — the CloudWatchLoggingOptions emitter that must write delivery-failure events (with errorCode and message) into the configured CloudWatch Logs stream.
Traces: firehose__logs__s3.cloudwatch-logging-options-emits-the-s3-access-denied-error-while-the-healthy-twin-logs-nothing

## Evidence
Replay detail, verbatim: "AssertionError: DATAPLANE_UNIMPLEMENTED?: CloudWatch Logs group '<log group>' receives a delivery-error event with errorCode 'S3.AccessDenied' did not happen within 300.0s. Last observation: []". Expected: at least one event in the configured log stream whose errorCode is 'S3.AccessDenied'. Actual: no events (empty list) after 300 s.

## Steps to Reproduce
1. Clone the repo and run the behavioral suite. 2. Replay firehose__logs__s3.cloudwatch-logging-options-emits-the-s3-access-denied-error-while-the-healthy-twin-logs-nothing. 3. Directly: create a delivery stream with an ExtendedS3 destination whose bucket policy denies the stream's role, enable CloudWatchLoggingOptions, PutRecord, then poll the log group for events.
