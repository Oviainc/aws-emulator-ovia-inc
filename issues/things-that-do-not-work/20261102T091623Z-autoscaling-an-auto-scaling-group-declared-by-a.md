Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2__iam.e2e-asgeni-secondary-eni-attach-failure-lifecycle
Issue-Key: autoscaling__ec2__iam.e2e-asgeni-secondary-eni-attach-failure-lifecycle|phase#5|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Commit: 3879dd05c1f9087ace2750498c91b002c8086d49

# autoscaling: an Auto Scaling group declared by a deployed stack describes as empty

> _Note: this issue was identified against the code base at commit `3879dd05c1f9087ace2750498c91b002c8086d49`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAutoScalingGroups

## Description
Cause 1 of 1: after the deploy phase completes, the Auto Scaling group the deployment declares is not present.
AWS: once the deployment completes, DescribeAutoScalingGroups filtered to the deployed group's name returns exactly that group (with its launch template, capacities and instances), so a workflow can read it back immediately.
Emulator: DescribeAutoScalingGroups returns an empty list for the group the deploy just created ('the deployed group describes as []'), so the group was never registered under the name the deployment reports.
Why: every later step that reads the group — capacity, instance list, the secondary-ENI attach failure this lifecycle diagnoses — operates on nothing, and an operator is told a successful deployment produced no group.
Fix: the Auto Scaling group creation path used by the deployment (the AWS::AutoScaling::AutoScalingGroup provider and CreateAutoScalingGroup state) must register the group so DescribeAutoScalingGroups finds it by name. See the related open issue 20261022T175547Z-autoscaling-auto-scaling-4-fidelity-divergences-cloudformation-asg-never.md, which carries a different check's replay key.
Traces: autoscaling__ec2__iam.e2e-asgeni-secondary-eni-attach-failure-lifecycle.

## Evidence
'AssertionError: [phase 1/6 deploy] the deployed group describes as []'.

## Steps to Reproduce
1. Deploy the stack that declares an Auto Scaling group with a launch template and wait for the deploy to complete.
2. Call DescribeAutoScalingGroups with AutoScalingGroupNames=[<the deployed group name>] and observe an empty list.
3. Or replay autoscaling__ec2__iam.e2e-asgeni-secondary-eni-attach-failure-lifecycle.
