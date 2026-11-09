Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__firehose__s3.cfn-deploy-eventbridge-firehose-s3-pipeline
Issue-Key: events__firehose__s3.cfn-deploy-eventbridge-firehose-s3-pipeline|_poll_until#19|Timeout|last = probe()
Issue-Commit: d6495890c37f2f3170333ee7f057f680b864ff4b

# CloudFormation/EventBridge: the stack-deployed EventBridge -> Firehose -> S3 pipeline never delivers a matched event to S3

> _Note: this issue was identified against the code base at commit `d6495890c37f2f3170333ee7f057f680b864ff4b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
Cause 1 of 1: the deployed pipeline delivers nothing.
AWS: a stack that deploys an S3 bucket, a Firehose delivery stream with that bucket as its destination, the delivery/target IAM roles, and an `AWS::Events::Rule` whose target is the delivery stream reaches CREATE_COMPLETE; a matching PutEvents is then put to the stream by EventBridge and Firehose flushes the record into the bucket, where it is readable as an S3 object.
Emulator: nothing ever lands. The check exhausts its budget (`Check timed out after 180.0s`) waiting for the record to appear in S3, so the deployed rule -> Firehose -> S3 hop produces no object, no error and nothing on any dead-letter path.
Why: a client that deploys the standard event-archiving pipeline through CloudFormation sees a healthy stack and an ENABLED rule but no data in its bucket, with no diagnostic anywhere.
Fix: the EventBridge Firehose-target invocation path (PutRecord/PutRecordBatch to the delivery stream from a rule target) and the Firehose delivery-stream flush to its S3 destination, including the stack-deployed wiring of the two roles.
Traces: events__firehose__s3.cfn-deploy-eventbridge-firehose-s3-pipeline.

Note: open issues 20261029T175617Z-events-firehose-s3-an-eventbridge-rule-s-firehose.md, 20261107T185637Z-events-firehose-s3-an-eventbridge-rule-s-firehose-target-never-delivers.md and 20261109T135435Z-events-firehose-s3-an-eventbridge-rule-with-a.md carry other events__firehose__s3 checks; none of them carries this replay key, which is the CloudFormation-deployed variant.

## Evidence
`Error: Check timed out after 180.0s` — the check waits for the PutEvents-triggered Firehose record to appear as an object in the stack's S3 bucket and nothing arrives.

## Steps to Reproduce
1. CreateStack with AWS::S3::Bucket, AWS::IAM::Role (firehose delivery role and events target role), AWS::KinesisFirehose::DeliveryStream with the bucket as destination, and AWS::Events::Rule whose target is the delivery stream.
2. Wait for CREATE_COMPLETE, then PutEvents an event matching the rule's pattern.
3. Poll `s3.list_objects_v2(Bucket=<stack bucket>)` — no object appears.
4. Replay events__firehose__s3.cfn-deploy-eventbridge-firehose-s3-pipeline.
