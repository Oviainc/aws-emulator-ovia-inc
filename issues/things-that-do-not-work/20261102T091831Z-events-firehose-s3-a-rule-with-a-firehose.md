Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__firehose__s3.eventbridge-fanout-firehose-and-sqs-targets
Issue-Key: events__firehose__s3.eventbridge-fanout-firehose-and-sqs-targets|_deliver_until_firehose_receives#11|Timeout|time.sleep(2)
Issue-Commit: b1a4a993822b0574a1f24a3d622a9d42702ca481

# events__firehose__s3: a rule with a Firehose target and an SQS target fans out to neither, so the two-target delivery contract is ungradeable

> _Note: this issue was identified against the code base at commit `b1a4a993822b0574a1f24a3d622a9d42702ca481`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
One net-new root cause.

Cause 1 of 1: a single rule carrying two independent targets — a Firehose delivery stream and an SQS queue — delivers to neither: no record lands in the stream's S3 destination and no envelope arrives on the queue.
Failing check: events__firehose__s3.eventbridge-fanout-firehose-and-sqs-targets

AWS: a matching event is delivered independently to every target on the rule. For the Firehose target EventBridge assumes the target RoleArn (granting firehose:PutRecord/PutRecordBatch) and puts the envelope as a record, which the delivery stream then writes to its S3 destination under the configured prefix; for the SQS target the same envelope is delivered as a queue message. Neither target's outcome affects the other. Recorded behavior only for delivery; Smithy (eventbridge) models Target{Id, Arn, RoleArn} on PutTargets, so a Firehose stream ARN with a RoleArn is a modeled, accepted target configuration.
Emulator: the check ends `Check timed out after 60.0s` — the S3 destination bucket stays empty and the SQS queue receives nothing, so neither leg of the fan-out is observable.
Why: a client teeing events into analytics storage via Firehose while also queueing them loses both copies, with an accepted PutTargets and an accepted PutEvents and no error to act on.
Fix: the EventBridge rule-target dispatch path in the events crate — per-target delivery for Firehose delivery-stream ARNs (assume RoleArn, PutRecord the envelope) and for SQS queue ARNs, each independent of the other. Open issue 20261029T175617Z-events-firehose-s3-an-eventbridge-rule-s-firehose.md owns the other checks in this group but not this replay key.
Traces: events__firehose__s3.eventbridge-fanout-firehose-and-sqs-targets.

## Evidence
Expected: one matching event produces a Firehose record written into the S3 destination AND the same envelope as an SQS message on the queue target.
Actual: `Check timed out after 60.0s` — the S3 destination has no object and the queue has no message.

## Steps to Reproduce
1. Create an S3 bucket, a Firehose role allowing s3:PutObject on it, and an ExtendedS3 delivery stream writing to that bucket.
2. Create an SQS queue with a policy admitting events.amazonaws.com.
3. Create a rule matching a chosen source; PutTargets both the delivery-stream ARN (with a RoleArn trusting events.amazonaws.com and allowing firehose:PutRecord) and the queue ARN.
4. PutEvents a matching event and poll the bucket and the queue: on AWS both receive the envelope; on the emulator neither does and the check times out.
