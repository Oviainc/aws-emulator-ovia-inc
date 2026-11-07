Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__events__sqs.events-ec2-lifecycle-running-rule-fires-on-start-not-on-stop ec2__events__sqs.events-ec2-lifecycle-ebs-attached-volume-delete-rejected-emits-no-notification ec2__events__sqs.xsvc-ec2-1-attach-and-detach-emit-no-volume-notification-unlike-create ec2__events__sqs.xsvc-ec2-1-create-image-ami-state-change-available-then-deregistered ec2__events__sqs.events-ec2-lifecycle-protected-terminate-rejected-emits-no-shutting-down
Issue-Key: ec2__events__sqs.events-ec2-lifecycle-running-rule-fires-on-start-not-on-stop|_drain#19|Timeout|msgs = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10,
Issue-Key: ec2__events__sqs.events-ec2-lifecycle-ebs-attached-volume-delete-rejected-emits-no-notification|_drain#26|Timeout|msgs = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10,
Issue-Key: ec2__events__sqs.xsvc-ec2-1-attach-and-detach-emit-no-volume-notification-unlike-create|_drain#17|Timeout|msgs = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10,
Issue-Key: ec2__events__sqs|CreateImage|NotImplemented
Issue-Key: ec2__events__sqs.events-ec2-lifecycle-protected-terminate-rejected-emits-no-shutting-down|run#17|AssertionError|assert message == expected, (
Issue-Commit: 620c90c23a321396910fd66a692ef2fcecf33f7f

# EC2 EventBridge: volume/instance lifecycle notifications never arrive, CreateImage is a stub, and the termination-protection message loses its period

> _Note: this issue was identified against the code base at commit `620c90c23a321396910fd66a692ef2fcecf33f7f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TerminateInstances

## Description
Three net-new root causes behind the surviving ec2__events__sqs failures.

Cause 1 of 3: EC2 lifecycle notifications are never delivered to an EventBridge rule, so both the positive and the negative delivery assertions hang.
AWS: EC2 publishes "EC2 Instance State-change Notification" on each state change and "EBS Volume Notification" / "EBS Snapshot Notification" on volume and snapshot events; a rule filtered to one of them delivers to its SQS target, and refused calls (a rejected attached-volume delete) emit nothing — a negative that can only be observed once the positive deliveries work (recorded behavior).
Emulator: nothing is delivered; the running-vs-stopped rule, the attach/detach negative and the attached-volume-delete negative all end "Check timed out after 60.0s".
Why: every event-driven EC2 integration receives nothing, and "no event" cannot be distinguished from "events do not work".
Fix: the EC2 state-change/volume-event emission into the EventBridge bus.
Traces: ec2__events__sqs.events-ec2-lifecycle-running-rule-fires-on-start-not-on-stop, .events-ec2-lifecycle-ebs-attached-volume-delete-rejected-emits-no-notification, .xsvc-ec2-1-attach-and-detach-emit-no-volume-notification-unlike-create.

Cause 2 of 3: CreateImage is an unimplemented scaffold variant, so the AMI state-change notification chain cannot start.
AWS: CreateImage returns an ImageId and the AMI moves pending -> available (then deregistered), each transition emitting an EC2 AMI State Change notification.
Emulator: ClientError (NotImplemented) — "operation CreateImage is scaffolded but not implemented in this emulator". The scaffold gap is filed against services/service-com-amazonaws-ec2-amazonec2/src/ops/create_image.rs for other checks; this check is a live failure under ec2__events__sqs and is recorded here because the fix must also emit the AMI state-change events.
Why: no image-based workflow or its notifications can be exercised.
Fix: the CreateImage handler plus its AMI state transitions and event emission.
Traces: ec2__events__sqs.xsvc-ec2-1-create-image-ami-state-change-available-then-deregistered.

Cause 3 of 3: the termination-protection refusal message loses its final period.
AWS: OperationNotPermitted with exactly "The instance '<id>' may not be terminated. Modify its 'disableApiTermination' instance attribute and try again." (fixed text terminated by a period; only the instance id varies).
Emulator: the same sentence without the trailing period.
Why: a client matching the documented message to confirm the refusal (before asserting no shutting-down event was emitted) fails the comparison.
Fix: TerminateInstances' disableApiTermination refusal message.
Traces: ec2__events__sqs.events-ec2-lifecycle-protected-terminate-rejected-emits-no-shutting-down.

## Evidence
From the behavioral replay:
- events-ec2-lifecycle-running-rule-fires-on-start-not-on-stop, events-ec2-lifecycle-ebs-attached-volume-delete-rejected-emits-no-notification, xsvc-ec2-1-attach-and-detach-emit-no-volume-notification-unlike-create [mismatch]: Error: Check timed out after 60.0s.
- xsvc-ec2-1-create-image-ami-state-change-available-then-deregistered [error]: botocore ClientError (NotImplemented) calling CreateImage.
- events-ec2-lifecycle-protected-terminate-rejected-emits-no-shutting-down [mismatch]: OperationNotPermitted message: expected "The instance 'i-...' may not be terminated. Modify its 'disableApiTermination' instance attribute and try again.", got the same text without the final period.

## Steps to Reproduce
Replay the behavioral suite and re-run the checks in trace_ids: create an EventBridge rule with an SQS target filtered to EC2/EBS notifications, then start/stop an instance, create/attach/detach/delete volumes, call CreateImage, and terminate a protection-enabled instance.
