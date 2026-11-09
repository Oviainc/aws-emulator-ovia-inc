Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose__s3__sesv2.firehose-destination-delivers-two-send-events-within-the-buffering-interval
Issue-Key: firehose__s3__sesv2.firehose-destination-delivers-two-send-events-within-the-buffering-interval|_poll_until#16|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Commit: 3b4b4e2df28997ae7cbb85154b0c83cf06a43ef8

# firehose__s3__sesv2: a configuration set's KinesisFirehoseDestination receives no SEND event, so SES email events never reach the Firehose stream's S3 destination

> _Note: this issue was identified against the code base at commit `3b4b4e2df28997ae7cbb85154b0c83cf06a43ef8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendEmail

## Description
=== Cause 1 of 1: SES event publishing to a KinesisFirehoseDestination never puts a record ===
Failing check: firehose__s3__sesv2.firehose-destination-delivers-two-send-events-within-the-buffering-interval.
AWS: Smithy KinesisFirehoseDestination requires IamRoleArn ("the IAM role that the Amazon SES API v2 uses to send email events to the Amazon Kinesis Data Firehose stream") and DeliveryStreamArn ("the Amazon Kinesis Data Firehose stream that the Amazon SES API v2 sends email events to"), and EventType includes SEND. So when a configuration set has an event destination with Enabled=true and MatchingEventTypes including SEND, every SendEmail using that configuration set causes SES to assume IamRoleArn and PutRecord one JSON event per send into the named delivery stream; the stream then delivers those records to its own destination, so within the stream's buffering interval both sends' event records are readable out of the ExtendedS3 destination prefix.
Emulator: no event record is ever produced. After two SendEmail calls on the configuration set, the destination prefix stays empty for the whole budget: "Firehose delivering both Send events (['sim2real-fh-evt-0-…', 'sim2real-fh-evt-1-…']) to S3 not met within 420s. Last result: []". CreateConfigurationSetEventDestination and SendEmail both succeed, so the wiring is accepted and the publish step is missing.
Why: the Firehose event destination is how senders archive and analyse SES event data. On the emulator the pipeline is silently empty: a client cannot verify that sends are recorded, cannot test the event JSON shape, and any downstream analytics built on it has no input — while the control plane reports a healthy, enabled event destination.
Fix: the sesv2 send path must publish an email-event record per matching EventType to each enabled event destination, assuming the destination's IamRoleArn and calling the firehose service's PutRecord on DeliveryStreamArn, so the record flows through the delivery stream's normal buffering/delivery to its destination. Open issue 20261026T063851Z-firehose-s3-sesv2-createconfigurationseteventdestination-accepts-a-kinesisfirehosedestination-whose.md concerns the CONTROL-plane validation of that destination's role and does not cover the missing publish.
Traces: the check id above.

## Evidence
Replayed suite: simparity-behavioral-aws-v1 — firehose__s3__sesv2 0/2 (0%), 2 failing checks of which this one is marked neither ALREADY FILED nor OWNED.

Expected (AWS): two SendEmail calls on a configuration set whose event destination is {Enabled: true, MatchingEventTypes: ['SEND'], KinesisFirehoseDestination: {IamRoleArn, DeliveryStreamArn}} put two SEND event records into the delivery stream, and both markers are readable from the stream's S3 destination prefix within the buffering interval.
Actual (emulator): "AssertionError: Firehose delivering both Send events (['sim2real-fh-evt-0-cfa907958fcc', 'sim2real-fh-evt-1-e532ab14cc02']) to S3 not met within 420s. Last result: []" — the destination prefix never contained any object.

## Steps to Reproduce
1. Boot the emulator and point botocore at it. s3.create_bucket(Bucket=B).
2. iam.create_role(R_fh) trusted by firehose.amazonaws.com able to write B; firehose.create_delivery_stream(DeliveryStreamName=S, DeliveryStreamType='DirectPut', ExtendedS3DestinationConfiguration={'RoleARN': R_fh, 'BucketARN': 'arn:aws:s3:::B', 'Prefix': 'ses/', 'BufferingHints': {'SizeInMBs': 1, 'IntervalInSeconds': 60}}); wait ACTIVE.
3. iam.create_role(R_ses) trusted by ses.amazonaws.com with firehose:PutRecord/PutRecordBatch on S's ARN.
4. sesv2.create_email_identity(...) for a verified sending domain; sesv2.create_configuration_set(ConfigurationSetName=CS); sesv2.create_configuration_set_event_destination(ConfigurationSetName=CS, EventDestinationName='fh', EventDestination={'Enabled': True, 'MatchingEventTypes': ['SEND'], 'KinesisFirehoseDestination': {'IamRoleArn': R_ses, 'DeliveryStreamArn': <S arn>}}).
5. sesv2.send_email(ConfigurationSetName=CS, ...) twice with distinct markers in the message.
6. Poll s3.list_objects_v2(Bucket=B, Prefix='ses/') and read the bodies for up to ~420 s.
7. Expect two SEND event records carrying the two markers; observe an empty prefix.
