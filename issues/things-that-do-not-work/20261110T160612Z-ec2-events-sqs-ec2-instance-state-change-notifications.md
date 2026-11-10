Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__events__sqs.xsvc-ec2-1-state-change-transformed-delivery-while-rejected-target-dead-letters
Issue-Key: ec2__events__sqs.xsvc-ec2-1-state-change-transformed-delivery-while-rejected-target-dead-letters|_drain_raw#29|Timeout|msgs = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=wait,
Issue-Commit: b11785bf9bf8af57a00ad7e4d814b1c5f1d66117

# ec2__events__sqs: EC2 Instance State-change Notifications never reach an EventBridge rule, so input-transformed delivery and dead-lettering of a rejected target never happen

> _Note: this issue was identified against the code base at commit `b11785bf9bf8af57a00ad7e4d814b1c5f1d66117`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRule

## Description
Cause 1 of 1: an instance's state transitions are never delivered to an EventBridge rule matching them, so neither the input-transformed delivery nor the dead-letter path of a target that rejects the event is observable.

AWS:      a RunInstances/StopInstances/TerminateInstances transition emits an `EC2 Instance State-change Notification` event with source `aws.ec2` and a detail carrying the instance id and the new state; a rule matching that pattern delivers to its targets, applying the target's InputTransformer to the payload, and an event a target rejects is retried and then written to the target's configured dead-letter queue. Recorded behavior only (the event envelope and delivery semantics are documented behaviour, not Smithy shapes).
Emulator: the check produces no verdict at all — `Error: Check timed out after 600.0s`. Its waits are the instance's state transitions and then ReceiveMessage on the transformed-delivery queue and on the dead-letter queue; no message ever arrives.
Why:      EC2 state-change events are the backbone of event-driven automation (cleanup on terminate, registration on run). With nothing delivered, every such automation silently never runs, and the dead-letter contract that is supposed to make a failed delivery visible produces no evidence either.
Fix:      the EC2 instance-lifecycle event emitter that should publish `aws.ec2` `EC2 Instance State-change Notification` events onto the event bus, and the EventBridge target delivery path (InputTransformer application and dead-letter routing for a rejecting target).
Traces:   ec2__events__sqs.xsvc-ec2-1-state-change-transformed-delivery-while-rejected-target-dead-letters.

Related but not covering: the open issue 20261107T075601Z-ec2-events-sqs-ec2-eventbridge-volume-instance-lifecycle-notifications-never-arrive.md records the same family for its own checks and does not carry this check's replay key.

## Evidence
Expected (AWS): after the instance transitions, the rule's transformed target receives a message built by its InputTransformer from the `EC2 Instance State-change Notification` detail (instance id + state), and the event the rejecting target refuses lands in that target's dead-letter queue.
Actual (emulator): `Error: Check timed out after 600.0s` — no message was ever received on either queue.

## Steps to Reproduce
1. Create an SQS queue and a dead-letter queue, and PutRule with an event pattern {source:['aws.ec2'], detail-type:['EC2 Instance State-change Notification']}.
2. PutTargets: one target with an InputTransformer onto the first queue, one target configured to reject with a DeadLetterConfig naming the DLQ.
3. RunInstances a t3.micro, wait for `running`, then StopInstances/TerminateInstances.
4. ReceiveMessage on both queues — nothing arrives.
