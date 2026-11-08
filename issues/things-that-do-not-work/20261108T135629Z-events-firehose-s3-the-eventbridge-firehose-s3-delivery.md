Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__firehose__s3.cfn-deploy-firehose-role-missing-trust-fails-delivery
Issue-Key: events__firehose__s3.cfn-deploy-firehose-role-missing-trust-fails-delivery|_poll_until#24|Timeout|last = probe()
Issue-Commit: 9e2a1177787692948f5e67ac213048cdaf07cba6

# events__firehose__s3: the EventBridge→Firehose→S3 delivery differential never reaches a verdict — the deployed good-trust stream never lands an object, so the check is killed at 180 s

> _Note: this issue was identified against the code base at commit `9e2a1177787692948f5e67ac213048cdaf07cba6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
One root cause.

Cause 1 of 1: an event routed by an EventBridge rule to a CloudFormation-deployed Firehose delivery stream never lands under that stream's S3 prefix, so the whole role-trust differential is unobservable and the check exhausts its 180 s budget.
AWS: two CloudFormation stacks each deploy an S3 bucket, an AWS::KinesisFirehose::DeliveryStream with an S3 destination and a delivery role, and an AWS::Events::Rule whose target is that delivery stream. PutEvents on the bus with the rule's source causes EventBridge to assume the rule's target role and PutRecord the event into the stream; Firehose then assumes its own delivery role and writes the buffered record as an object under the configured S3 prefix, so the object is observable by ListObjectsV2 within the stream's buffering interval. For the second, otherwise identical stack whose delivery role trusts only ec2.amazonaws.com, Firehose cannot assume the role, nothing is ever written under that stream's prefix, and the prefix stays empty whether or not the stack itself deploys. The differential — object present on one side, permanently absent on the other — is what attributes the failure to the trust principal rather than to any other delivery fault. Smithy says nothing about delivery timing or about Firehose's role assumption, so this is recorded behavior only.
Emulator: the check never reaches any assertion verdict and is terminated with "Check timed out after 180.0s". The positive half never produces an object under the good stream's S3 prefix, so the polling loop that waits for it never satisfies and the negative half is never evaluated. Both prefixes are indistinguishable (empty), so the emulator cannot be told apart from one where the trust check is enforced and from one where delivery is simply broken.
Why: a client building an EventBridge→Firehose→S3 pipeline gets neither the delivery nor a refusal: the pipeline it deployed reports healthy resources and silently drops every event, and a mis-trusted delivery role — the error this configuration is most prone to — produces exactly the same observable (an empty prefix) as a correctly trusted one. There is no signal a client can poll on, so it waits out its own timeout instead of either consuming the data or diagnosing the role.
Fix: the EventBridge rule-target dispatch to a Firehose delivery stream and the Firehose S3 delivery path — the target role assumption on the EventBridge side, the delivery role assumption and S3 writer on the Firehose side (which must fail delivery, and write nothing, when the delivery role does not trust firehose.amazonaws.com) — together with the CloudFormation provisioning of AWS::KinesisFirehose::DeliveryStream and AWS::Events::Rule those stacks declare. The related open issues 20261029T175617Z-events-firehose-s3-an-eventbridge-rule-s-firehose.md and 20261107T185637Z-events-firehose-s3-an-eventbridge-rule-s-firehose-target-never-delivers.md describe the same delivery path but do not carry this check's replay key, so this check is not covered by either.
Traces: events__firehose__s3.cfn-deploy-firehose-role-missing-trust-fails-delivery

## Evidence
events__firehose__s3.cfn-deploy-firehose-role-missing-trust-fails-delivery (mismatch)
Error: Check timed out after 180.0s

Expected (AWS): after PutEvents on the bus, ListObjectsV2 under the good stream's S3 prefix returns the delivered event object within the stream's buffering interval, and ListObjectsV2 under the bad stream's prefix stays empty for the whole observation window.
Actual (emulator): no object ever appears under the good stream's prefix, so the check's poll-for-delivery loop never satisfies and the run is killed at its 180 s budget before any assertion is evaluated. No per-call error code or status is produced — the divergence is the absence of the delivered object.

## Steps to Reproduce
1. Deploy two CloudFormation stacks, each declaring: an AWS::S3::Bucket; an IAM role for Firehose with S3 write permissions on that bucket (stack A's role trusts Service firehose.amazonaws.com, stack B's role trusts only Service ec2.amazonaws.com); an AWS::KinesisFirehose::DeliveryStream with an ExtendedS3DestinationConfiguration pointing at the bucket/prefix and that role; an IAM role trusting events.amazonaws.com with firehose:PutRecord; and an AWS::Events::Rule whose EventPattern matches a distinct source and whose target is the delivery stream with that role.
2. events PutEvents with stack A's source, then poll s3 ListObjectsV2 under stack A's prefix for the delivered object.
3. events PutEvents with stack B's source, then confirm nothing ever appears under stack B's prefix.
4. Step 2 never produces an object against the emulator and the check is killed at 180 s.
