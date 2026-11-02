Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2__kms.e2e-asglf-diagnose-auto-scaling-group-launch-failure-lifecycle
Issue-Key: autoscaling__ec2__kms.e2e-asglf-diagnose-auto-scaling-group-launch-failure-lifecycle|phase#7|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Commit: fb0a9c1858b32d12ecab08653c5d3b6198801bbb

# autoscaling__ec2__kms: an Auto Scaling group created by a deployed CloudFormation stack is invisible to DescribeAutoScalingGroups, so the ASG launch-failure diagnosis cannot start

> _Note: this issue was identified against the code base at commit `fb0a9c1858b32d12ecab08653c5d3b6198801bbb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAutoScalingGroups

## Description
ONE net-new root cause.

Cause 1 of 1 — a stack whose resources include an AWS::AutoScaling::AutoScalingGroup settles and publishes its Outputs, but the group it declares never exists in Auto Scaling.
AWS: DescribeAutoScalingGroups (Smithy com.amazonaws.autoscaling#DescribeAutoScalingGroups, input AutoScalingGroupNames, output AutoScalingGroups required) returns the group a settled stack created: the name the stack publishes as an Output resolves to exactly one AutoScalingGroup, with (MinSize, DesiredCapacity, MaxSize) == the template's (0, 0, 1) and LaunchTemplate == {LaunchTemplateId, LaunchTemplateName, Version '1'} naming the stack's launch template. A stack that reaches a successful terminal state and hands out a resource identifier in its Outputs has in fact created that resource — every subsequent read of it resolves.
Emulator: after the deploy phase the stack's Outputs are read and DescribeAutoScalingGroups(AutoScalingGroupNames=[<the published group name>]) returns an EMPTY AutoScalingGroups list, so the group CloudFormation reported creating does not exist in the Auto Scaling store. The lifecycle fails in its first phase and the solve/cleanup/destroy phases never run.
Why: a client that deploys infrastructure declaratively and then operates on it by the identifiers the stack publishes — exactly the CLI/SDK diagnosis this lifecycle performs (group bounds -> launch-template version -> the KMS key the root volume is encrypted under) — finds nothing to operate on, while the stack claims success. Every cross-service walk that starts from a stack output is unreachable, and so is the whole fault the scenario exists to diagnose.
Fix: the CloudFormation resource provider for AWS::AutoScaling::AutoScalingGroup must actually create the group in the Auto Scaling service's state (and only then report the resource created and publish its identifier as an Output) — the stack-resource provisioning path, not the Auto Scaling read path, which answers correctly for groups created through its own API in the same replay.
Related but not covering: open issues 20261102T091623Z-autoscaling-an-auto-scaling-group-declared-by-a.md, 20261102T085002Z-autoscaling-ec2-elbv2-2-net-new-root-causes.md and 20261029T172317Z-cloudformation-a-stack-deploying-an-auto-scaling-group.md record the same class of defect from other service groupings; none carries this check id, and the behavior is still served on this replay.
Traces: autoscaling__ec2__kms.e2e-asglf-diagnose-auto-scaling-group-launch-failure-lifecycle.

## Evidence
Behavioral replay of suite simparity-behavioral-aws-v1 against this checkout. Check autoscaling__ec2__kms.e2e-asglf-diagnose-auto-scaling-group-launch-failure-lifecycle [mismatch] failed in its first of four phases: AssertionError: [phase 1/4 deploy] DescribeAutoScalingGroups('<stack-output AutoScalingGroupName>') returned []. The group name is a per-run value taken from the deployed stack's Outputs; the requirement is that the name a settled stack publishes resolves to exactly one AutoScalingGroup. Expected in that phase: the group with (MinSize, DesiredCapacity, MaxSize) == (0, 0, 1) and LaunchTemplate == {LaunchTemplateId: <stack output>, LaunchTemplateName: <stack output>, Version: '1'}; the later phases then read the KMS key (KeyState 'Disabled', Enabled false) and the launch template version's encrypted root volume. Because the group is absent, none of that was exercised. Smithy grounding: DescribeAutoScalingGroups input AutoScalingGroupNames / output AutoScalingGroups (required), AutoScalingGroup{MinSize, MaxSize, DesiredCapacity, LaunchTemplate} in services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/smithy-model/com-amazonaws-autoscaling-autoscaling-2011-01-01.json; the stack-settles-then-resource-exists rule is recorded behavior.

## Steps to Reproduce
1) Build and boot the emulator from this checkout and replay suite simparity-behavioral-aws-v1, check autoscaling__ec2__kms.e2e-asglf-diagnose-auto-scaling-group-launch-failure-lifecycle.
2) Deploy the check's stack (deployments/asglf-helper-asg/template.json: VPC + subnet + two security groups + IAM role/profile + a KMS key created Disabled + launch template with an encrypted root volume + an AWS::AutoScaling::AutoScalingGroup at 0/0/1) and wait for the stack to settle.
3) Read the stack's Outputs and call autoscaling DescribeAutoScalingGroups(AutoScalingGroupNames=[<AutoScalingGroupName output>]) -> AutoScalingGroups must contain exactly that group with (MinSize, DesiredCapacity, MaxSize) == (0, 0, 1) and LaunchTemplate == {LaunchTemplateId, LaunchTemplateName, Version '1'} matching the stack's launch-template outputs; the emulator returns [].
