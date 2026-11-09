Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__firehose__s3.eventbridge-rule-invokes-firehose-target events__firehose__s3.eventbridge-pattern-mismatch-suppresses-firehose-delivery events__firehose__s3.eventbridge-firehose-target-role-missing-trust-rejected
Issue-Key: events__firehose__s3.eventbridge-rule-invokes-firehose-target|run#14|AssertionError|raise AssertionError(
Issue-Key: events__firehose__s3.eventbridge-pattern-mismatch-suppresses-firehose-delivery|_poll_until#13|AssertionError|raise AssertionError(
Issue-Key: events__firehose__s3.eventbridge-firehose-target-role-missing-trust-rejected|_poll_until#16|AssertionError|raise AssertionError(
Issue-Commit: 3b4b4e2df28997ae7cbb85154b0c83cf06a43ef8

# events__firehose__s3: an EventBridge rule with a Firehose delivery-stream target still delivers no record, so the pattern-match and role-trust differentials never reach a verdict

> _Note: this issue was identified against the code base at commit `3b4b4e2df28997ae7cbb85154b0c83cf06a43ef8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
=== Cause 1 of 1: a rule whose Target is a Firehose delivery stream never puts a record into that stream ===
Failing checks: events__firehose__s3.eventbridge-rule-invokes-firehose-target, events__firehose__s3.eventbridge-pattern-mismatch-suppresses-firehose-delivery, events__firehose__s3.eventbridge-firehose-target-role-missing-trust-rejected.
AWS: PutEvents on a bus with a rule whose EventPattern matches delivers the event to each of the rule's Targets; for a Target whose Arn is a Firehose delivery stream and whose RoleArn is a role EventBridge can assume with firehose:PutRecord, EventBridge calls PutRecord with the (optionally input-transformed) event as the record payload. The record then follows the stream's own delivery contract and becomes an object under the ExtendedS3 destination's Prefix, so after the buffering interval the matching event's marker is readable out of the bucket — and, with CloudWatchLoggingOptions enabled, the delivery is visible in the configured log stream. The negative legs are only meaningful against that working positive leg: a non-matching sibling event in the same PutEvents batch must NOT appear, and a target role that does not trust events.amazonaws.com must deliver nothing while the correctly-trusted rule does.
Emulator: no record is produced for any of the three checks. The pattern-match check never sees the MATCHING event: "matching event delivered to S3 via Firehose not met within 360.0s. Last result: []". The trust differential never sees its positive control: "correctly-trusted rule's event delivered to S3 via Firehose not met within 240.0s. Last result: []". The diagnostic leg finds no delivery log events either: "DIAGNOSTIC firehose delivery log events: []". Rule creation, PutTargets and PutEvents all succeed, so the rule is accepted and then the Firehose target is never invoked.
Why: EventBridge→Firehose→S3 is a standard ingestion path; on the emulator it silently drops every event, so a client sees a healthy rule with an empty destination. Worse for testing, the negative contracts (pattern mismatch suppresses delivery; an untrusted target role delivers nothing) are vacuously "satisfied" because nothing is ever delivered — a target that drops everything passes the same assertions a correct one does, so these boundaries are ungradeable.
Fix: the EventBridge rule-dispatch component must support a Firehose delivery-stream target: assume the target's RoleArn, call the firehose service's PutRecord with the matched (and InputTransformer-applied) event, and surface a delivery failure rather than silently dropping it. Related open issues 20261029T175617Z-events-firehose-s3-an-eventbridge-rule-s-firehose.md, 20261107T185637Z-events-firehose-s3-an-eventbridge-rule-s-firehose-target-never-delivers.md and 20261108T135629Z-events-firehose-s3-the-eventbridge-firehose-s3-delivery.md describe the same family of symptoms but the replay marks none of these three checks as owned by them, so they carry no replay key for this evidence.
Traces: the three check ids above.

## Evidence
Replayed suite: simparity-behavioral-aws-v1 — events__firehose__s3 12/26 (46%), 14 failing checks of which these three are marked neither ALREADY FILED nor OWNED.

Expected (AWS): after put_events of a matching event on a bus whose rule targets a Firehose delivery stream (with an EventBridge-assumable role carrying firehose:PutRecord), the event's marker is readable from the stream's ExtendedS3 destination prefix within the buffering interval, and the delivery is visible in the stream's CloudWatch log stream.
Actual (emulator):
- events__firehose__s3.eventbridge-pattern-mismatch-suppresses-firehose-delivery — "AssertionError: matching event delivered to S3 via Firehose not met within 360.0s. Last result: []" (the positive leg of the differential).
- events__firehose__s3.eventbridge-firehose-target-role-missing-trust-rejected — "AssertionError: correctly-trusted rule's event delivered to S3 via Firehose not met within 240.0s. Last result: []".
- events__firehose__s3.eventbridge-rule-invokes-firehose-target — "AssertionError: DIAGNOSTIC firehose delivery log events: []".
In all three the control plane succeeded (bucket, role, delivery stream ACTIVE, rule + target created, put_events accepted); only the record is missing.

## Steps to Reproduce
1. Boot the emulator and point botocore at it. s3.create_bucket(Bucket=B).
2. iam.create_role(R_fh) trusted by firehose.amazonaws.com with s3:PutObject/s3:AbortMultipartUpload on arn:aws:s3:::B and B/*; firehose.create_delivery_stream(DeliveryStreamName=S, DeliveryStreamType='DirectPut', ExtendedS3DestinationConfiguration={'RoleARN': R_fh, 'BucketARN': 'arn:aws:s3:::B', 'Prefix': 'evt/', 'BufferingHints': {'SizeInMBs': 1, 'IntervalInSeconds': 60}, 'CloudWatchLoggingOptions': {...}}); wait ACTIVE.
3. iam.create_role(R_evt) trusted by events.amazonaws.com with firehose:PutRecord on S's ARN.
4. events.put_rule(Name=RU, EventPattern='{"source":["sim2real.test"]}'); events.put_targets(Rule=RU, Targets=[{'Id':'1','Arn':<S arn>,'RoleArn':R_evt}]).
5. events.put_events(Entries=[{'Source':'sim2real.test','DetailType':'t','Detail':'{"marker":"<unique>"}'}, {'Source':'sim2real.other', ...}]).
6. Poll s3.list_objects_v2(Bucket=B, Prefix='evt/') and read the bodies for up to the buffering interval plus slack.
7. Expect the matching event's marker to be present (and only it); observe an empty prefix and no delivery log events. Repeat with a target role that does NOT trust events.amazonaws.com alongside a correctly-trusted rule: expect only the trusted rule's event; observe neither delivers.

Finally re-replay the three check ids in trace_ids.
