Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2__events__sqs.xsvc-autoscaling-1-scale-in-terminate-event
Issue-Key: autoscaling__ec2__events__sqs.xsvc-autoscaling-1-scale-in-terminate-event|_scale_in#19|AssertionError|assert (d.get("Origin"), d.get("Destination")) == ("EC2", "AutoScalingGroup"), (
Issue-Commit: 72bb6bfd937a489e314e0b070a9ac1619964b2b9

# autoscaling__ec2__events__sqs: the Auto Scaling lifecycle event detail omits Origin and Destination, so a launch/scale-in event carries no direction

> _Note: this issue was identified against the code base at commit `72bb6bfd937a489e314e0b070a9ac1619964b2b9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAutoScalingGroup

## Description
Cause 1 of 1: the 'EC2 Instance Launch Successful' / 'EC2 Instance Terminate Successful' events Auto Scaling publishes to EventBridge are emitted with a `detail` object that has no `Origin` and no `Destination` member, so a consumer cannot tell which direction the instance moved.

AWS: not described by the Smithy model — EventBridge event payloads are not modeled shapes (autoscaling's model has no Origin/Destination member; the closest modeled thing, DescribeScalingActivities' Activity, is a different projection), so this is AWS's documented event payload plus recorded behavior only. AWS's Auto Scaling lifecycle event detail carries, in addition to StatusCode / Description / AutoScalingGroupName / ActivityId / Details / RequestId / StatusMessage / EC2InstanceId / StartTime / EndTime / Cause, the pair `Origin` and `Destination`, naming where the instance came from and where it went. The values are fixed text drawn from the activity's direction, not from the request: a scale-out launch is `"Origin": "EC2"`, `"Destination": "AutoScalingGroup"`; a scale-in termination is the reverse, `"Origin": "AutoScalingGroup"`, `"Destination": "EC2"`. Only the surrounding values (group name, instance id, activity id, zone, subnet, timestamps) come from the request and the group's state.

Emulator: both members are absent from `detail`. The replayed launch event's detail was exactly `{'StatusCode': 'Successful', 'Description': 'Launching a new EC2 instance: <instance-id>', 'AutoScalingGroupName': '<group>', 'ActivityId': '<uuid>', 'Details': {'Availability Zone': '<az>', 'Subnet ID': '<subnet-id>'}, 'RequestId': '<uuid>', 'StatusMessage': '', 'EC2InstanceId': '<instance-id>', 'StartTime': '<ts>', 'EndTime': '<ts>', 'Cause': 'an Auto Scaling group was created: changing the desired capacity from 0 to 2.'}` — `detail.get("Origin")` and `detail.get("Destination")` are both None, where AWS gives ('EC2', 'AutoScalingGroup'). The envelope itself is delivered (the two launch events reach the rule's SQS queue and name the group's instances), so this is not the delivery gap already filed as 20261107T075601Z-autoscaling-ec2-events-sqs-asg-lifecycle-events-never-reach-eventbridge-sqs-and.md — it is the detail's shape. The check fails at the launch assertion, so the scale-in half (Origin='AutoScalingGroup', Destination='EC2' on the terminate event) is never reached and is not separately evidenced here; the same publisher owns it.

Why: a client that routes or branches on direction — an EventBridge pattern like {"detail": {"Origin": ["AutoScalingGroup"]}} to separate scale-in from scale-out, or a consumer that reads detail['Origin'] — matches nothing and raises KeyError/None against the emulator, while the same code works on AWS. Warm-pool and lifecycle automation keyed on these two fields silently never fires.

Fix: the Auto Scaling lifecycle-event publisher that builds the EventBridge `detail` for 'EC2 Instance Launch Successful' / 'EC2 Instance Terminate Successful' must add the Origin and Destination members and set them from the scaling activity's direction.

Traces: autoscaling__ec2__events__sqs.xsvc-autoscaling-1-scale-in-terminate-event.

## Evidence
Check autoscaling__ec2__events__sqs.xsvc-autoscaling-1-scale-in-terminate-event, [mismatch].

Expected (AWS): every 'EC2 Instance Launch Successful' detail has (Origin, Destination) == ('EC2', 'AutoScalingGroup'); the scale-in 'EC2 Instance Terminate Successful' detail has ('AutoScalingGroup', 'EC2').

Actual (emulator): AssertionError: a launch event must flow EC2 -> AutoScalingGroup: {'StatusCode': 'Successful', 'Description': 'Launching a new EC2 instance: i-...', 'AutoScalingGroupName': 'sim2real-xsvc-asgt-...', 'ActivityId': '...', 'Details': {'Availability Zone': 'us-west-2a', 'Subnet ID': 'subnet-...'}, 'RequestId': '...', 'StatusMessage': '', 'EC2InstanceId': 'i-...', 'StartTime': '...', 'EndTime': '...', 'Cause': 'an Auto Scaling group was created: changing the desired capacity from 0 to 2.'} — neither key is present, so both read None.

## Steps to Reproduce
1. Create a VPC/subnet and a launch template, an SQS queue, and two EventBridge rules on the default bus whose patterns are {source: ['aws.autoscaling'], 'detail-type': ['EC2 Instance Launch Successful' | 'EC2 Instance Terminate Successful'], detail: {AutoScalingGroupName: [<group>]}}, targeting the queue (queue policy granting events.amazonaws.com sqs:SendMessage).
2. CreateAutoScalingGroup with MinSize=0, MaxSize=2, DesiredCapacity=2 over that launch template and subnet.
3. Drain the queue and read the two 'EC2 Instance Launch Successful' envelopes' detail: neither has an Origin or a Destination member (AWS gives 'EC2' and 'AutoScalingGroup').
4. SetDesiredCapacity to 1 and read the 'EC2 Instance Terminate Successful' detail: the same two members are expected to be 'AutoScalingGroup' and 'EC2'.
