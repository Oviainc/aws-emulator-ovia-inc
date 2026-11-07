Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2__events__sqs.events-ec2-lifecycle-asg-desired-above-max-rejected-then-launch-event-names-group autoscaling__ec2__events__sqs.xsvc-autoscaling-1-filtered-by-group-name autoscaling__ec2__events__sqs.xsvc-autoscaling-1-scale-in-terminate-event
Issue-Key: autoscaling__ec2__events__sqs.events-ec2-lifecycle-asg-desired-above-max-rejected-then-launch-event-names-group|run#23|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: autoscaling__ec2__events__sqs.xsvc-autoscaling-1-filtered-by-group-name|_drain_raw#20|Timeout|msgs = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10,
Issue-Key: autoscaling__ec2__events__sqs.xsvc-autoscaling-1-scale-in-terminate-event|_drain_raw#19|Timeout|msgs = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10,
Issue-Commit: 620c90c23a321396910fd66a692ef2fcecf33f7f

# ASG lifecycle events never reach EventBridge/SQS, and the SetDesiredCapacity refusal message carries the group name

> _Note: this issue was identified against the code base at commit `620c90c23a321396910fd66a692ef2fcecf33f7f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetDesiredCapacity

## Description
Two net-new root causes behind the surviving autoscaling__ec2__events__sqs failures.

Cause 1 of 2: no Auto Scaling lifecycle event is ever delivered to an EventBridge rule, so every event-driven assertion times out.
AWS: a group's launch and scale-in emit "EC2 Instance Launch Successful" / "EC2 Instance Terminate Successful" events on the default event bus with source aws.autoscaling and a detail naming the AutoScalingGroupName and the EC2InstanceId; a rule whose pattern filters on that group name delivers them to its SQS target (recorded behavior).
Emulator: nothing is delivered — the checks wait on the queue and end "Check timed out after 60.0s" — so neither the group-name filter nor the scale-in terminate event can be observed.
Why: every event-driven integration built on ASG lifecycle events (the documented way to react to scaling) silently receives nothing, and a negative-delivery assertion cannot be distinguished from a broken bus.
Fix: the autoscaling launch/terminate path's event emission into the EventBridge bus (and the bus-to-SQS delivery it feeds).
Traces: autoscaling__ec2__events__sqs.xsvc-autoscaling-1-filtered-by-group-name, autoscaling__ec2__events__sqs.xsvc-autoscaling-1-scale-in-terminate-event.

Cause 2 of 2: SetDesiredCapacity's above-max ValidationError appends the group name where AWS ends the sentence.
AWS: ValidationError, message exactly "New SetDesiredCapacity value <n> is above max value <max> for the AutoScalingGroup." — fixed text with a terminating period; the only variable parts are the requested value and the group's MaxSize.
Emulator: "New SetDesiredCapacity value 2 is above max value 1 for the AutoScalingGroup: <group name>".
Why: a client that classifies the refusal by its documented sentence (and then asserts the subsequent launch event names the group) misreads the refusal.
Fix: SetDesiredCapacity validation message.
Traces: autoscaling__ec2__events__sqs.events-ec2-lifecycle-asg-desired-above-max-rejected-then-launch-event-names-group.

## Evidence
From the behavioral replay:
- events-ec2-lifecycle-asg-desired-above-max-rejected-then-launch-event-names-group [mismatch]: AssertionError: expected message 'New SetDesiredCapacity value 2 is above max value 1 for the AutoScalingGroup.', got 'New SetDesiredCapacity value 2 is above max value 1 for the AutoScalingGroup: sim2real-evlc-asg-...'.
- xsvc-autoscaling-1-filtered-by-group-name [mismatch]: Error: Check timed out after 60.0s.
- xsvc-autoscaling-1-scale-in-terminate-event [mismatch]: Error: Check timed out after 60.0s.

## Steps to Reproduce
Replay the behavioral suite and re-run the checks in trace_ids: create an ASG, an EventBridge rule filtered on the group name with an SQS target, then scale the group out and in and poll the queue for the launch/terminate events; also call SetDesiredCapacity above MaxSize and compare the ValidationError message.
