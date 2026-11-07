Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling.op-contract-cancelInstanceRefresh autoscaling.deep-autoscaling-1-asg-recon-oob-termination-replacement autoscaling.desired-capacity-type-vcpu-counts-vcpus-not-instances autoscaling.attach-operations-accepted-during-instance-refresh autoscaling.deploy-asg-cloudformation-scales-to-desired
Issue-Shape-Id: com.amazonaws.autoscaling#AutoScaling_2011_01_01
Issue-Key: autoscaling.op-contract-cancelInstanceRefresh|_poll_until#7|Timeout|last = probe()
Issue-Key: autoscaling.deep-autoscaling-1-asg-recon-oob-termination-replacement|_poll#16|Timeout|last = fn()
Issue-Key: autoscaling.desired-capacity-type-vcpu-counts-vcpus-not-instances|_poll#11|Timeout|last = fn()
Issue-Key: autoscaling.attach-operations-accepted-during-instance-refresh|_poll_until#19|Timeout|last = probe()
Issue-Key: autoscaling.deploy-asg-cloudformation-scales-to-desired|_poll_until_instance_count#16|TimeoutError|raise TimeoutError(f"{expected} InService instances in {asg_name}: the run budget ended "
Issue-Commit: ee9893ed79b0abeb742411b16639d26129234894

# A group with DesiredCapacity > 0 never reports an InService instance, so instance-refresh, reconciliation and vCPU-capacity flows never converge (1 cause)

> _Note: this issue was identified against the code base at commit `ee9893ed79b0abeb742411b16639d26129234894`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAutoScalingGroup

## Description
Cause 1 of 1 — DescribeAutoScalingGroups never reports an InService instance for a group whose DesiredCapacity is greater than zero, so every flow that must observe a live instance before it can proceed stalls until its budget ends.
AWS: Smithy: CreateAutoScalingGroup/SetDesiredCapacity take DesiredCapacity (AutoScalingGroupDesiredCapacity) and the group's Instances are returned by DescribeAutoScalingGroups as AutoScalingGroups[].Instances with LifecycleState (Pending -> InService) and HealthStatus; the model states the representation, not the timing, so the convergence itself is recorded behavior only. In the recordings a group created with DesiredCapacity=1 (EC2 health check, grace period 30s, a launch template that resolves) launches an EC2 instance and that instance appears in Instances with LifecycleState="InService" within the checks' polling budgets; from there StartInstanceRefresh reports Status=InProgress and CancelInstanceRefresh drives it to Cancelled, an out-of-band EC2 TerminateInstances is reconciled by launching a replacement, and with DesiredCapacityType="vcpu" the group launches DesiredCapacity/vCPUs-per-instance machines.
Emulator: the group is created and describes back, but Instances never contains an entry with LifecycleState="InService" (and for the CloudFormation-provisioned group, never reaches 1 InService instance at all). Every check that polls for it exhausts its budget: four are graded `mismatch` with "Check timed out after 60.0s" and one errors with "TimeoutError: 1 InService instances in <group>: the run budget ended first; not graded". Because the poll is the first step, none of the downstream assertions (refresh Status transitions, CancelInstanceRefresh's ActiveInstanceRefreshNotFound sentence, replacement after an out-of-band terminate, vCPU-budget arithmetic, attach-during-refresh acceptance) is ever reached.
Why: a client's standard pattern is create/scale then wait for Instances[].LifecycleState=="InService" (the shape the CLI `wait` and CDK/CloudFormation resource signals use). Against this emulator that wait never returns, so the caller cannot tell "still launching" from "will never launch", and nothing downstream of a live instance — instance refresh, health replacement, termination policies, capacity-unit semantics — can be exercised at all.
Fix: the Auto Scaling group reconciliation/state component that turns DesiredCapacity into EC2 instances and advances their LifecycleState (the group state and the launch path behind CreateAutoScalingGroup/SetDesiredCapacity, and the instance-refresh state machine that reads it), so DescribeAutoScalingGroups reports Pending then InService instances. Related open issues describe neighbouring symptoms but carry none of these checks' replay keys: 20261107T071147Z-autoscaling-auto-scaling-group-and-instance-state-is-never.md, 20261101T215724Z-autoscaling-auto-scaling-scale-in-never-terminates-an-instance.md, 20261107T075601Z-autoscaling-ec2-elbv2-auto-scaling-elbv2-asg-instances-never-register-with.md.
Traces: autoscaling.op-contract-cancelInstanceRefresh, autoscaling.deep-autoscaling-1-asg-recon-oob-termination-replacement, autoscaling.desired-capacity-type-vcpu-counts-vcpus-not-instances, autoscaling.attach-operations-accepted-during-instance-refresh, autoscaling.deploy-asg-cloudformation-scales-to-desired.

## Evidence
Expected: after CreateAutoScalingGroup(DesiredCapacity=1, LaunchTemplate=..., HealthCheckType="EC2") (or a CloudFormation AWS::AutoScaling::AutoScalingGroup with DesiredCapacity 1), DescribeAutoScalingGroups(AutoScalingGroupNames=[group]) returns AutoScalingGroups[0].Instances with at least one entry whose LifecycleState is "InService"; the checks poll for this for 420s/480s.

Actual: the poll never satisfies that predicate.
- autoscaling.op-contract-cancelInstanceRefresh — mismatch, "Check timed out after 60.0s"; its helper `_create_asg_with_instance` polls `groups[0]["Instances"]` for `LifecycleState == "InService"` (timeout_s=420) and never returns, so StartInstanceRefresh/CancelInstanceRefresh are never reached.
- autoscaling.deep-autoscaling-1-asg-recon-oob-termination-replacement — mismatch, "Check timed out after 60.0s".
- autoscaling.desired-capacity-type-vcpu-counts-vcpus-not-instances — mismatch, "Check timed out after 60.0s".
- autoscaling.attach-operations-accepted-during-instance-refresh — mismatch, "Check timed out after 60.0s".
- autoscaling.deploy-asg-cloudformation-scales-to-desired — error, `TimeoutError: 1 InService instances in sim2real-behav-asg-<id>: the run budget ended first; not graded` raised from `_poll_until_instance_count(..., expected=1, timeout_s=480)`.
(The group name, instance ids and the 60s budget are per-run values, not requirements; the requirement is that an InService instance appears.)

## Steps to Reproduce
1. Boot the emulator; create a launch template in EC2 with a resolvable ImageId and InstanceType=t3.micro.
2. CreateAutoScalingGroup with MinSize=0, MaxSize=2, DesiredCapacity=1, that LaunchTemplate and a VPCZoneIdentifier subnet, HealthCheckType="EC2", HealthCheckGracePeriod=30.
3. Poll DescribeAutoScalingGroups(AutoScalingGroupNames=[group]) and inspect AutoScalingGroups[0].Instances.
4. Observe no entry ever reaches LifecycleState="InService" (AWS shows Pending then InService). Consequently StartInstanceRefresh -> Status="InProgress" -> CancelInstanceRefresh -> Status="Cancelled", replacement after an out-of-band ec2:TerminateInstances, and DesiredCapacityType="vcpu" instance counts cannot be observed.
5. Same via CloudFormation: deploy a stack with an AWS::AutoScaling::AutoScalingGroup of DesiredCapacity 1 and poll for 1 InService instance.
6. Re-replay the checks in trace_ids to confirm a fix.
