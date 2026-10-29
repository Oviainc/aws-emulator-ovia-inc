Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__firehose__s3.eventbridge-put-events-triggers-firehose-to-s3 events__firehose__s3.eventbridge-fanout-firehose-and-sqs-targets events__firehose__s3.eventbridge-input-transformer-firehose-record events__firehose__s3.eventbridge-firehose-delivery-stream-arn-resolution events__firehose__s3.eventbridge-firehose-target-missing-permission-not-invoked events__firehose__s3.events-rule-role-trusts-events-service events__firehose__s3.cfn-deploy-eventbridge-firehose-s3-pipeline events__firehose__s3.cfn-deploy-firehose-role-missing-trust-fails-delivery events__firehose__s3.firehose-role-missing-s3-permission-denied events__firehose__s3.firehose-role-scoped-bucket-denied-on-other-bucket
Issue-Key: events__firehose__s3.eventbridge-put-events-triggers-firehose-to-s3|_warm_up_delivery#9|Timeout|time.sleep(2.0)
Issue-Key: events__firehose__s3.eventbridge-fanout-firehose-and-sqs-targets|_deliver_until_firehose_receives#11|Timeout|time.sleep(15.0)
Issue-Key: events__firehose__s3.eventbridge-input-transformer-firehose-record|_records_within#13|Timeout|time.sleep(interval_s)
Issue-Key: events__firehose__s3.eventbridge-firehose-delivery-stream-arn-resolution|_poll_until#14|Timeout|time.sleep(interval_s)
Issue-Key: events__firehose__s3.eventbridge-firehose-target-missing-permission-not-invoked|_poll_until#16|Timeout|time.sleep(interval_s)
Issue-Key: events__firehose__s3.events-rule-role-trusts-events-service|_s3_body_with_marker#23|Timeout|time.sleep(5)
Issue-Key: events__firehose__s3.cfn-deploy-eventbridge-firehose-s3-pipeline|_poll_until#19|Timeout|time.sleep(interval_s)
Issue-Key: events__firehose__s3.cfn-deploy-firehose-role-missing-trust-fails-delivery|_poll_until#24|Timeout|time.sleep(interval_s)
Issue-Key: events__firehose__s3.firehose-role-missing-s3-permission-denied|_poll_until#17|Timeout|time.sleep(interval_s)
Issue-Key: events__firehose__s3.firehose-role-scoped-bucket-denied-on-other-bucket|_poll_until#18|Timeout|time.sleep(interval_s)
Issue-Commit: f4e5251e6df88363d8e23ace2ff5bff3be31b423

# events__firehose__s3: an EventBridge rule's Firehose target never produces a record (3 causes — direct rule delivery, CFN-deployed pipeline, and the delivery role's S3 denial never logged)

> _Note: this issue was identified against the code base at commit `f4e5251e6df88363d8e23ace2ff5bff3be31b423`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
Three net-new root causes in the EventBridge -> Firehose -> S3 path. All ten surviving failures in this service are end-to-end observations that never materialise, so every one of them burns the whole 60 s check budget and is reported as `Check timed out after 60.0s`.

NOTE: the two other failures in this service (events-target-role-without-trust-rejected, s3-role-without-trust-policy-rejected) are already OWNED by 20261022T194351Z-events-firehose-s3-eventbridge-firehose-s3-eventing-eventbridge-tagresource-and-s3.md and 20261028T105230Z-events-firehose-s3-s3-replication-accepts-a-role.md and are NOT part of this issue. The resolved issue 20261024T005347Z-events-firehose-s3-eventbridge-to-firehose-to-s3-records-put-into.md covered PutRecord-to-S3 materialisation only; the EventBridge *target invocation* leg below is a different cause and is not a refusal, so this is not a reopen.

=== Cause 1 of 4: an EventBridge rule whose target is a Firehose delivery-stream ARN never delivers the event to the stream ===
AWS: recorded behavior only (the EventBridge Smithy model describes PutTargets/PutEvents but not the asynchronous target invocation). AWS behavior: PutEvents returns 200 with FailedEntryCount 0, EventBridge then assumes the target's RoleArn and calls firehose:PutRecord on the target delivery stream; after the stream's BufferingHints window the event body appears as an object under the destination bucket/prefix. A target referenced by ARN receives the event and a sibling, unreferenced stream does not. With an InputTransformer on the target, the bytes Firehose receives are the transformer's rendered output, not the raw event envelope.
Emulator: PutEvents is accepted, but no record ever reaches the delivery stream and no object is ever written to the destination bucket. The checks poll the destination for the whole budget and are killed at 60 s with zero objects. The negative arms are equally ungradeable: the emulator cannot show that a target role without firehose:PutRecord is NOT invoked, because the positively-permissioned control never delivers either.
Why: any client building an eventing pipeline (rule -> Firehose -> S3/analytics) observes a silently empty destination. Nothing distinguishes a correct pipeline from a broken one, and an IAM misconfiguration in a target role is indistinguishable from a working rule.
Fix: the EventBridge target-invocation dispatcher — the component that, after a rule matches, assumes Target.RoleArn and issues the destination call for a `kinesis-firehose` target ARN (and applies Target.InputTransformer before handing the bytes over).
Traces: events__firehose__s3.eventbridge-put-events-triggers-firehose-to-s3, events__firehose__s3.eventbridge-fanout-firehose-and-sqs-targets, events__firehose__s3.eventbridge-input-transformer-firehose-record, events__firehose__s3.eventbridge-firehose-delivery-stream-arn-resolution, events__firehose__s3.eventbridge-firehose-target-missing-permission-not-invoked, events__firehose__s3.events-rule-role-trusts-events-service.

=== Cause 2 of 4: a CloudFormation-deployed EventBridge rule -> Firehose -> S3 stack reaches CREATE_COMPLETE but the pipeline is inert ===
AWS: recorded behavior only. A stack declaring AWS::S3::Bucket, AWS::IAM::Role, AWS::KinesisFirehose::DeliveryStream and AWS::Events::Rule (with a Firehose Target whose Arn is Fn::GetAtt DeliveryStream.Arn) converges to CREATE_COMPLETE with every resource usable: a single PutEvents matching the rule's pattern becomes an object under the stream's destination prefix.
Emulator: the stack is accepted, but the deployed pipeline delivers nothing — the destination prefix stays empty for the whole budget. The negative arm (one rule, two streams whose roles differ only in their trust policy: one trusting firehose.amazonaws.com, one trusting only ec2.amazonaws.com) cannot be graded either, because the GOOD stream never delivers, so the BAD stream's empty prefix proves nothing.
Why: a template that works on AWS silently produces a dead pipeline, and trust-policy mistakes in a deployed template are undetectable.
Fix: the CloudFormation resource providers for AWS::KinesisFirehose::DeliveryStream and AWS::Events::Rule/Target (so the Ref'd/GetAtt'd stream ARN resolves to a stream the EventBridge dispatcher of Cause 1 can actually invoke), plus the dispatcher itself.
Traces: events__firehose__s3.cfn-deploy-eventbridge-firehose-s3-pipeline, events__firehose__s3.cfn-deploy-firehose-role-missing-trust-fails-delivery.

=== Cause 3 of 4: the Firehose delivery role's S3 permissions are never evaluated, and no S3.AccessDenied delivery-error record is ever written to CloudWatchLoggingOptions ===
AWS: recorded behavior only (CloudWatchLoggingOptions is modelled, its per-delivery error records are not). AWS behavior: a delivery stream whose RoleARN trusts firehose.amazonaws.com but grants no s3:PutObject (or grants it only for a different bucket) never writes to the destination, and the stream's configured CloudWatch Logs group/stream (LogStreamName "DestinationDelivery") receives a JSON delivery-error record whose errorCode is exactly `S3.AccessDenied`. Widening the role's policy to cover the bucket makes the identical delivery succeed.
Emulator: no delivery-error record with any errorCode is ever emitted to the configured log group, and the role's S3 policy is not consulted at all. Both checks also show the positive control never completing, so neither the denial nor the restore is observable; the checks are killed at 60 s.
Why: a client debugging a delivery failure on AWS reads the DestinationDelivery log stream to get `S3.AccessDenied`; against the emulator that log stream is permanently empty, and an under-permissioned role looks identical to a correct one.
Fix: the Firehose S3 delivery worker — it must authorize the PutObject against the stream's RoleARN before writing, and its error path must emit the delivery-error record (errorCode/errorMessage) into the stream's CloudWatchLoggingOptions log group.
Traces: events__firehose__s3.firehose-role-missing-s3-permission-denied, events__firehose__s3.firehose-role-scoped-bucket-denied-on-other-bucket.

## Evidence
All ten surviving checks in this service report the same terminal verdict, `Error: Check timed out after 60.0s`, each while polling for an effect that never arrives:

- eventbridge-put-events-triggers-firehose-to-s3: polls the destination bucket for the batched PutEvents records in Firehose's default YYYY/MM/DD/HH key layout; zero objects.
- eventbridge-fanout-firehose-and-sqs-targets: one rule, two targets; expects the record in S3 via Firehose AND the same envelope as an SQS message.
- eventbridge-input-transformer-firehose-record: expects the delivered bytes to be the InputTransformer's rendered output, not the raw envelope.
- eventbridge-firehose-delivery-stream-arn-resolution: expects delivery into the targeted stream's bucket only, never into the sibling stream's bucket.
- eventbridge-firehose-target-missing-permission-not-invoked: expects the properly-permissioned sibling rule to deliver while the role with no firehose:PutRecord does not.
- events-rule-role-trusts-events-service: expects the event payload to land in S3 when the target role trusts events.amazonaws.com.
- cfn-deploy-eventbridge-firehose-s3-pipeline: stack of Bucket + FirehoseRole + DeliveryStream + EventsRole + Rule; expects a matching PutEvents to be observable as an object in the destination bucket, "not merely CREATE_COMPLETE on every resource".
- cfn-deploy-firehose-role-missing-trust-fails-delivery: GoodStream role trusts firehose.amazonaws.com, BadStream role trusts only ec2.amazonaws.com; expects GoodStream's prefix to receive the record and BadStream's never to.
- firehose-role-missing-s3-permission-denied: expects `logged == "S3.AccessDenied"` from the broken stream's DestinationDelivery log stream and a delivered object under good/ for the positive control; the positive control never delivers and the log stream is empty.
- firehose-role-scoped-bucket-denied-on-other-bucket: role scoped to bucket A only; expects S3.AccessDenied for the B-targeting stream, successful delivery to A, and success to B after the policy is widened.

Expected: a matching PutEvents yields a Firehose record and, after the buffering window, an object under the destination prefix; an unassumable/unpermissioned role yields no object; a denied S3 write yields an `S3.AccessDenied` delivery-error record in the stream's CloudWatch Logs stream.
Actual: nothing is ever delivered on any leg, and no delivery-error record is ever logged, so each check exhausts its 60 s budget with an empty destination.

## Steps to Reproduce
1. Clone the repo and boot the emulator (run_behavioral_suite).
2. Cause 1: create an S3 bucket, an IAM role trusting firehose.amazonaws.com with s3:PutObject on it, a DirectPut delivery stream with ExtendedS3DestinationConfiguration (BufferingHints SizeInMBs=1, IntervalInSeconds=60) pointing at that bucket. Create an IAM role trusting events.amazonaws.com with firehose:PutRecord/PutRecordBatch on the stream ARN. PutRule with an EventPattern on a unique source, PutTargets with {Id, Arn: <stream ARN>, RoleArn: <events role>}. Call PutEvents with a matching event. List the destination bucket: it stays empty indefinitely. Repeat with an InputTransformer on the target, and with two streams where only one is targeted.
3. Cause 2: deploy the same topology as a CloudFormation stack (AWS::S3::Bucket, AWS::IAM::Role, AWS::KinesisFirehose::DeliveryStream with BucketARN/RoleARN via Fn::GetAtt, AWS::Events::Rule with the Firehose target). Wait for CREATE_COMPLETE, PutEvents a matching event, then list the destination bucket: empty.
4. Cause 3: create a delivery stream whose RoleARN has ONLY logs:PutLogEvents (no s3:*) and whose ExtendedS3DestinationConfiguration sets CloudWatchLoggingOptions {Enabled: true, LogGroupName: <group>, LogStreamName: "DestinationDelivery"}. PutRecord into it, then read the log group: no delivery-error record with errorCode S3.AccessDenied is ever written. Repeat with a role scoped to bucket A while the stream targets bucket B.
