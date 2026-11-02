Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2__elbv2.asg-scale-in-respects-termination-policy autoscaling__ec2__elbv2.e2e-asgdx-diagnose-asg-instance-issues-lifecycle autoscaling__ec2__elbv2.e2e-asghc-asg-instance-health-check-failure-lifecycle
Issue-Key: autoscaling__ec2__elbv2.asg-scale-in-respects-termination-policy|run#19|AssertionError|assert states.get(oldest_id) == "terminated", f"expected oldest instance terminated, states={states}"
Issue-Key: autoscaling__ec2__elbv2.e2e-asgdx-diagnose-asg-instance-issues-lifecycle|phase#6|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Key: autoscaling__ec2__elbv2.e2e-asghc-asg-instance-health-check-failure-lifecycle|phase#8|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Commit: b1a4a993822b0574a1f24a3d622a9d42702ca481

# autoscaling__ec2__elbv2: 2 net-new root causes — scale-in ignores the OldestInstance termination policy, and a deployed stack's AutoScalingGroup does not exist afterwards

> _Note: this issue was identified against the code base at commit `b1a4a993822b0574a1f24a3d622a9d42702ca481`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateAutoScalingGroup

## Description
Two net-new root causes from the autoscaling__ec2__elbv2 checks that are not owned by 20261029T172317Z-autoscaling-auto-scaling-6-fidelity-divergences-putscalingpolicy-rejects-none (PutScalingPolicy validation, launch-template propagation) nor by 20261022T093447Z-fis-4-fidelity-divergences-cloudformation-aws-fis-experimenttemplate.

Cause 1 of 2: scale-in terminates the newest instance under TerminationPolicies=['OldestInstance'].
AWS: with TerminationPolicies ['OldestInstance'], lowering DesiredCapacity by one terminates the instance that launched FIRST and leaves the later one running: EC2 DescribeInstances reports the oldest instance's State.Name 'terminated' (or shutting-down on the way there) and the newer one 'running'. Recorded behavior only for the pairing; the policy semantics are AWS's documented rule for OldestInstance.
Emulator: the opposite instance is terminated — the first-launched instance stays 'running' and the second-launched one goes 'terminated', so the configured policy is not consulted (it appears to terminate the most recently added member).
Why: termination policy is how operators protect state-carrying or canary members; an emulator that inverts it validates capacity automation that destroys exactly the instance AWS would keep.
Fix: the Auto Scaling scale-in selection logic in the group's state machine (honour TerminationPolicies when choosing victims).
Traces: autoscaling__ec2__elbv2.asg-scale-in-respects-termination-policy.

Cause 2 of 2: a CloudFormation stack that finishes deploying leaves no Auto Scaling group behind.
AWS: once a stack containing an AWS::AutoScaling::AutoScalingGroup reaches CREATE_COMPLETE, DescribeAutoScalingGroups for the stack's physical group name returns exactly one group carrying the declared HealthCheckType/HealthCheckGracePeriod, LaunchTemplate and TargetGroupARNs. Recorded behavior only.
Emulator: DescribeAutoScalingGroups for the name the stack reports returns 0 groups — the ASG resource is never materialised even though the deploy phase completed (two separate e2e stacks, asgdx-fault-estate and asghc-elbv2-stack, fail identically in phase 1).
Why: every downstream diagnosis step (launch template -> security group -> target group -> subnet walk) has no group to start from, so the whole CFN-fronted ASG workflow is unusable.
Fix: the CloudFormation resource provider for AWS::AutoScaling::AutoScalingGroup (create the group and register its physical id), in the same family as the already-open 20261023T205437Z-cloudformation-reports-create-complete-for-resources-it-never — which names ELBv2 target groups, trust stores and ECS services but not AutoScalingGroup.
Traces: autoscaling__ec2__elbv2.e2e-asgdx-diagnose-asg-instance-issues-lifecycle, autoscaling__ec2__elbv2.e2e-asghc-asg-instance-health-check-failure-lifecycle.

## Evidence
Cause 1: after UpdateAutoScalingGroup(DesiredCapacity=1) on a group created with TerminationPolicies=['OldestInstance'], min 1 / max 2, expected the first-launched instance 'terminated'; got states={'i-00000000000000001': 'running', 'i-00000000000000002': 'terminated'} — the oldest survived and the newest died (instance ids are one run's values; the oldest/newest pairing is the requirement).
Cause 2: expected exactly one group from DescribeAutoScalingGroups for the stack's group name; got "[phase 1/6 deploy] DescribeAutoScalingGroups('<stack group name>') returned 0 groups" in both e2e checks.

## Steps to Reproduce
Replay the behavioral suite and re-run the autoscaling__ec2__elbv2 checks in trace_ids. (1) CreateAutoScalingGroup with a launch template, min 1 / max 2 / desired 1 and TerminationPolicies=['OldestInstance']; wait for one InService instance, UpdateAutoScalingGroup DesiredCapacity=2, wait for two, then UpdateAutoScalingGroup DesiredCapacity=1 and read EC2 DescribeInstances for both ids. (2) Deploy either e2e check's CloudFormation template (deployments/asgdx-fault-estate, deployments/asghc-elbv2-stack) and call DescribeAutoScalingGroups with the group name the stack reports.
