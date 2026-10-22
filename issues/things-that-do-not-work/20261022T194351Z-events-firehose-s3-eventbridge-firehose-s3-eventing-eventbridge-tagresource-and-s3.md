Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__firehose__s3.eventbridge-firehose-delivery-stream-arn-resolution events__firehose__s3.eventbridge-firehose-target-role-missing-trust-rejected events__firehose__s3.s3-replication-role-trusts-s3-service events__firehose__s3.s3-role-without-trust-policy-rejected events__firehose__s3.events-target-role-without-trust-rejected events__firehose__s3.firehose-role-without-trust-policy-rejected
Issue-Key: events__firehose__s3|TagResource|NotImplemented
Issue-Key: events__firehose__s3|PutBucketReplication|NotImplemented
Issue-Key: events__firehose__s3.events-target-role-without-trust-rejected|_fire_until_settled#26|AssertionError|raise AssertionError(f"target invocation never settled after {attempts} attempts (last: {last_reason})")
Issue-Key: events__firehose__s3.firehose-role-without-trust-policy-rejected|_assert_rejected_for_missing_trust#5|AssertionError|raise AssertionError(
Issue-Commit: 17f16131651e3a76635c083c093a8581359ec045

# EventBridge/Firehose/S3 eventing: EventBridge TagResource and S3 PutBucketReplication are 501 stubs, failed target invocations never reach the DLQ, and an untrusted Firehose role is accepted (4 root causes)

> _Note: this issue was identified against the code base at commit `17f16131651e3a76635c083c093a8581359ec045`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Four net-new root causes behind the events__firehose__s3 failures that are not OWNED by an open issue (the remaining checks in this group fail on S3 ListObjectsV2(encoding-type) or CFN provisioning, owned by 20261022T154846Z-emr-glue-s3-cross-service-suite-cfn-declared, 20261022T064119Z-firehose-cloudformation-deployed-firehose-delivery-streams-are-never-provisioned and 20261022T172625Z-codebuild-codepipeline-ecr-ci-chain-pipeline-executions-never).

Cause 1 of 4: EventBridge TagResource answers 501 NotImplemented.
AWS: events.TagResource(ResourceARN=<rule arn>, Tags=[...]) returns 200 with an empty body for an existing rule and refuses a missing one with ResourceNotFoundException (400). It is an ordinary control-plane call that any rule-provisioning helper makes.
Emulator: 501, surfacing as botocore ClientError 'NotImplemented' — 'operation TagResource is scaffolded but not implemented in this emulator'.
Why: every workflow that tags the rule it just created aborts before it can even put targets or events, so the whole rule-plus-target path behind it is unreachable.
Fix: the scaffold stub for TagResource in the EventBridge service crate (services/service-com-amazonaws-eventbridge-awsevents/src/ops/tag_resource.rs).
Traces: events__firehose__s3.eventbridge-firehose-delivery-stream-arn-resolution, events__firehose__s3.eventbridge-firehose-target-role-missing-trust-rejected.

Cause 2 of 4: S3 PutBucketReplication answers 501 NotImplemented.
AWS: s3.PutBucketReplication(Bucket=b, ReplicationConfiguration={'Role': <role arn>, 'Rules': [...]}) returns 200 on a versioning-enabled bucket and refuses a role S3 cannot assume with 400 InvalidRequest.
Emulator: 501 'operation PutBucketReplication is scaffolded but not implemented in this emulator'.
Why: replication cannot be configured at all, so neither the positive path (a role trusting s3.amazonaws.com is accepted) nor the negative path (a role without that trust is refused) exists for a client.
Fix: the scaffold stub for PutBucketReplication in the S3 service crate (services/service-com-amazonaws-s3-amazons3/src/ops/put_bucket_replication.rs).
Traces: events__firehose__s3.s3-replication-role-trusts-s3-service, events__firehose__s3.s3-role-without-trust-policy-rejected.

Cause 3 of 4: a target invocation that cannot be performed produces no dead-letter message.
AWS: when a rule's target carries a RoleArn whose trust policy does not allow events.amazonaws.com, EventBridge cannot assume the role, the invocation fails, and with a DeadLetterConfig on the target the event is written to the configured DLQ (an SQS message carrying the original event plus the failure metadata); the target itself is never invoked.
Emulator: nothing ever arrives on the DLQ — the check retried the whole put_events/settle cycle twice and observed 'no bad-role DLQ message within 15.0s' — so a failed invocation is silently dropped rather than dead-lettered.
Why: a client cannot observe or recover failed deliveries; the emulator makes an undeliverable event indistinguishable from an unmatched one, and DLQ-based alerting is untestable.
Fix: target-invocation failure handling in the EventBridge rule dispatcher — on an assume-role/permission failure the event must be routed to the target's DeadLetterConfig queue.
Traces: events__firehose__s3.events-target-role-without-trust-rejected.

Cause 4 of 4: CreateDeliveryStream accepts a role whose trust policy does not include firehose.amazonaws.com.
AWS: CreateDeliveryStream validates that the supplied RoleARN can be assumed by firehose.amazonaws.com and refuses otherwise with a 400 InvalidArgumentException-class refusal ('Firehose is unable to assume role ...').
Emulator: the call returns 200 with a DeliveryStreamARN for a role whose trust policy names a different principal. Note for triage: the open issue 20261022T064119Z-firehose-createdeliverystream-putrecordbatch-are-overly-permissive-and-misclassify describes the same permissiveness, but this check id is not on its replay list, so it is recorded here.
Why: the whole class of mis-provisioned-role failures disappears; an operator gets a stream that AWS would have refused and only discovers the broken role when delivery silently stops.
Fix: RoleARN trust-policy resolution in the CreateDeliveryStream handler of the Firehose service crate.
Traces: events__firehose__s3.firehose-role-without-trust-policy-rejected.

## Evidence
Cause 1 — expected 200 from events.tag_resource on a just-created rule; actual botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the TagResource operation: operation TagResource is scaffolded but not implemented in this emulator (2 checks).
Cause 2 — expected 200/400 from s3.put_bucket_replication; actual An error occurred (NotImplemented) when calling the PutBucketReplication operation: operation PutBucketReplication is scaffolded but not implemented in this emulator (2 checks).
Cause 3 — expected an SQS DLQ message for the undeliverable event; actual AssertionError: target invocation never settled after 2 attempts (last: marker sim2real.source.bed1b0edb0c7-1: no bad-role DLQ message within 15.0s).
Cause 4 — expected a 400 ClientError; actual AssertionError: create_delivery_stream unexpectedly succeeded for a role whose trust policy does not include firehose.amazonaws.com.

## Steps to Reproduce
1. Boot the emulator and point botocore at it.
2. Cause 1: events.put_rule(...) then events.tag_resource(ResourceARN=<RuleArn>, Tags=[{'Key':'k','Value':'v'}]) -> expect 200; observe 501 NotImplemented.
3. Cause 2: create a bucket, enable versioning, s3.put_bucket_replication(Bucket=b, ReplicationConfiguration={'Role': <role trusting s3.amazonaws.com>, 'Rules':[...]}) -> expect 200; observe 501 NotImplemented. Repeat with a role that does not trust s3.amazonaws.com -> expect 400 InvalidRequest.
4. Cause 3: put a rule with a target whose RoleArn is not assumable by events.amazonaws.com and a DeadLetterConfig pointing at an SQS queue; events.put_events a matching event -> expect a DLQ message; observe none within the budget.
5. Cause 4: firehose.create_delivery_stream with ExtendedS3DestinationConfiguration and a RoleARN whose trust policy names a non-Firehose principal -> expect 400; observe 200 with a DeliveryStreamARN.
6. Re-replay the check ids in trace_ids.
