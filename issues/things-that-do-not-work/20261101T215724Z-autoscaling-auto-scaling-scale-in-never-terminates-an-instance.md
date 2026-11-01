Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2__elbv2.asg-scale-in-respects-termination-policy
Issue-Key: autoscaling__ec2__elbv2.asg-scale-in-respects-termination-policy|run#19|Timeout|assert states.get(oldest_id) == "terminated", f"expected oldest instance terminated, states={states}"
Issue-Commit: e898b7083ade325d607ce2fc3ec607fe797d8f21

# Auto Scaling scale-in never terminates an instance: DesiredCapacity decrease with TerminationPolicies=[OldestInstance] leaves both instances running

> _Note: this issue was identified against the code base at commit `e898b7083ade325d607ce2fc3ec607fe797d8f21`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateAutoScalingGroup

## Description
ONE root cause (Cause 1 of 1), grounded in the single surviving failing check autoscaling__ec2__elbv2.asg-scale-in-respects-termination-policy. Note: the open issue 20261029T172317Z-autoscaling-auto-scaling-6-fidelity-divergences-putscalingpolicy-rejects-none.md lists this same check id in its Issue-Trace-Ids (its Cause 5, replay key `autoscaling__ec2__elbv2.asg-scale-in-respects-termination-policy|run#18|Timeout|time.sleep(15)`), but this run's failure was NOT marked OWNED by the suite — the replay key has drifted (the failure now surfaces as a bare `Check timed out after 60.0s` with no inner frame), so the key that issue carries no longer binds this failure and it is filed here with the current key.

Cause 1 of 1: an Auto Scaling group does not scale in. Lowering DesiredCapacity on a group that is above the new capacity must terminate member instances, selecting them with the group's TerminationPolicies; the emulator terminates nothing and the group stays at its old instance count forever.

AWS: Smithy (Auto Scaling 2011-01-01, awsQuery): UpdateAutoScalingGroup takes UpdateAutoScalingGroupType with members AutoScalingGroupName (@required) and DesiredCapacity, and TerminationPolicies (a list of XmlStringMaxLen1600, not @required); the model expresses no scale-in semantics, so this is documented service behavior plus recorded behavior only. Recorded behavior (real AWS, us-west-2): with a group at Min=1/Max=2 and TerminationPolicies=['OldestInstance'], UpdateAutoScalingGroup DesiredCapacity=2 brings a second instance to InService; a subsequent UpdateAutoScalingGroup DesiredCapacity=1 returns 200, then the group reduces to one member by terminating the FIRST-LAUNCHED instance — ec2 DescribeInstances over both member ids shows the oldest instance in State.Name == 'terminated' and the newer one still 'running', and autoscaling DescribeAutoScalingGroups drops the terminated instance from Instances[]. Selection is the OldestInstance policy's documented rule; the oldest member is the one with the earliest launch time, taken from group state, not from any value in one recording.

Emulator: after DesiredCapacity is reduced from 2 to 1, polling ec2 DescribeInstances on both member instance ids never reports either one 'terminated' — both stay 'running' indefinitely, so the check reaches no terminal result and is cut off with `Check timed out after 60.0s` (the check's own wait budget is 420s, and against a correct emulator each transition is immediate, so the 60s cap is not what is failing here — nothing ever transitions). Hypothesis: the desired-capacity-decrease path updates only the group's declared DesiredCapacity and performs no reconciliation, so no instance is ever selected for termination and the termination-policy evaluation is never reached.

Why: scale-in is half of Auto Scaling. A client that lowers desired capacity sees the group report the new capacity while every instance keeps running — cost keeps accruing, target-group registrations and licences are never released, and the instance AWS would have chosen (oldest vs newest, per TerminationPolicies) is unobservable, so any test or automation that asserts which instance survives a scale-in can never pass. A waiter or poll on a scale-in blocks forever instead of converging.

Fix: the scale-in path of the Auto Scaling group reconciler in the autoscaling service — on a DesiredCapacity decrease (UpdateAutoScalingGroup and SetDesiredCapacity handlers and the group state they drive), select victims by the group's TerminationPolicies (default + OldestInstance/NewestInstance/...), drive the selected instances through the terminating lifecycle to EC2 state 'terminated', and remove them from the group's Instances[].

Traces: autoscaling__ec2__elbv2.asg-scale-in-respects-termination-policy.

## Evidence
Check autoscaling__ec2__elbv2.asg-scale-in-respects-termination-policy — verdict [mismatch], detail: `Error: Check timed out after 60.0s`.
Expected (real AWS): after CreateAutoScalingGroup(Min=1, Max=2, Desired=1, TerminationPolicies=['OldestInstance']), UpdateAutoScalingGroup(DesiredCapacity=2) then UpdateAutoScalingGroup(DesiredCapacity=1), ec2 DescribeInstances(InstanceIds=[oldest, newer]) yields States {oldest: 'terminated', newer: 'running'} (check assertions: `assert states.get(oldest_id) == "terminated"` and `assert states.get(newer_id) == "running"`).
Actual (emulator): the poll loop `while time.monotonic() < deadline: ... if states.get(oldest_id) == "terminated" or states.get(newer_id) == "terminated": break` never breaks — neither instance ever reports 'terminated' — so the check produces no terminal result and is cut off at the harness cap (`Check timed out after 60.0s`). UpdateAutoScalingGroup(DesiredCapacity=1) itself returns success, i.e. the capacity change is accepted and then not acted on.
The preceding legs of the check pass against the emulator (the group reaches 1 InService instance and scales out to 2 InService instances, with the original instance still InService), so the divergence is isolated to the capacity-DECREASE path.

## Steps to Reproduce
1. ec2: CreateVpc (10.99.0.0/24), CreateSubnet (10.99.0.0/25) in an available AZ, CreateSecurityGroup in that VPC, pick an available amazon-owned al2023 x86_64 AMI via DescribeImages, CreateLaunchTemplate with LaunchTemplateData {ImageId, InstanceType: t3.micro, SecurityGroupIds:[sg]}.
2. autoscaling: CreateAutoScalingGroup on that launch template with MinSize=1, MaxSize=2, DesiredCapacity=1, VPCZoneIdentifier=<subnet>, TerminationPolicies=['OldestInstance']; wait until DescribeAutoScalingGroups reports exactly 1 instance with LifecycleState 'InService' — record its InstanceId as the oldest.
3. autoscaling: UpdateAutoScalingGroup(AutoScalingGroupName, DesiredCapacity=2); wait until 2 instances are InService; the second id is the newer one.
4. autoscaling: UpdateAutoScalingGroup(AutoScalingGroupName, DesiredCapacity=1).
5. ec2: poll DescribeInstances(InstanceIds=[oldest, newer]) and read Reservations[].Instances[].State.Name. Expected: oldest -> 'terminated', newer -> 'running'. Observed: both remain 'running' indefinitely and the check times out.
Replay: the check id in trace_ids.
