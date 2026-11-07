Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling.create-asg-state-transition autoscaling.op-contract-suspend-processes autoscaling.asg-reflects-launch-template autoscaling.mixed-instances-policy-overrides-launch autoscaling.termination-policy-selects-instance autoscaling.deploy-launch-template-asg-instance-launch autoscaling.deploy-asg-cloudformation-scaling-behavior autoscaling.deploy-scaling-policy-cloudformation-triggers-scaling
Issue-Shape-Id: com.amazonaws.autoscaling#AutoScaling_2011_01_01
Issue-Key: autoscaling.create-asg-state-transition|run#19|KeyError|assert group["SuspendedProcesses"] == []
Issue-Key: autoscaling.op-contract-suspend-processes|_suspended_names#7|KeyError|return {p["ProcessName"] for p in group["SuspendedProcesses"]}
Issue-Key: autoscaling.asg-reflects-launch-template|run#19|KeyError|assert spec["LaunchTemplateId"] == lt["LaunchTemplateId"], "group must resolve the template id"
Issue-Key: autoscaling.mixed-instances-policy-overrides-launch|run#29|AssertionError|assert dist.get("OnDemandAllocationStrategy") in ("lowest-price", "prioritized"), (
Issue-Key: autoscaling.termination-policy-selects-instance|run#18|AssertionError|assert survivors == [second], (
Issue-Key: autoscaling.deploy-launch-template-asg-instance-launch|run#18|AssertionError|assert instance_tags.get("sim2real-marker") == marker, instance_tags
Issue-Key: autoscaling.deploy-asg-cloudformation-scaling-behavior|run#14|IndexError|group = autoscaling.describe_auto_scaling_groups(AutoScalingGroupNames=[asg_name])[
Issue-Key: autoscaling.deploy-scaling-policy-cloudformation-triggers-scaling|run#12|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in cfn.describe_stacks(StackName=stack_name)["Stacks"][0]["Outputs"]}
Issue-Commit: 38b869ac761e6c1ceaf9cdd20433d5027a2449ee

# DescribeAutoScalingGroups omits SuspendedProcesses, the launch-template identity and mixed-instances defaults; scale-in ignores TerminationPolicies; launched instances lose template tags; CFN-declared groups are invisible (7 causes)

> _Note: this issue was identified against the code base at commit `38b869ac761e6c1ceaf9cdd20433d5027a2449ee`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAutoScalingGroups

## Description
SEVEN net-new root causes in how the emulator represents and reconciles Auto Scaling group state. Each must be closed separately. Related open issues that do NOT carry these check ids: 20261102T114505Z-autoscaling-describeautoscalinggroups-misreports-the-group-s-launch-template-and-mixed-instances-policy.md, 20261107T071147Z-autoscaling-auto-scaling-group-and-instance-state-is-never-applied.md, 20261101T215724Z-autoscaling-auto-scaling-scale-in-never-terminates-an-instance.md, 20261102T085002Z-autoscaling-ec2-elbv2-2-net-new-root-causes.md, 20261022T175547Z-autoscaling-auto-scaling-4-fidelity-divergences-cloudformation-asg-never.md, 20261029T172317Z-cloudformation-a-stack-deploying-an-auto-scaling-group.md.

Cause 1 of 7: DescribeAutoScalingGroups does not return SuspendedProcesses at all.
AWS: every AutoScalingGroup member of the response carries SuspendedProcesses — an EMPTY list when no process is suspended, never an absent member. Smithy: AutoScalingGroup.SuspendedProcesses targets SuspendedProcesses (list of SuspendedProcess{ProcessName, SuspensionReason}); the SDK surfaces it as a present key.
Emulator: the member is absent, so SDK access to the field raises KeyError and the suspend/resume contract cannot be read at all.
Why: any client inspecting which processes are suspended (every scaling-freeze workflow) breaks on a missing key instead of seeing an empty list.
Fix: the DescribeAutoScalingGroups serializer/state projection in the Auto Scaling crate — always emit SuspendedProcesses.
Traces: autoscaling.create-asg-state-transition, autoscaling.op-contract-suspend-processes.

Cause 2 of 7: the described group's LaunchTemplate loses the identifier the caller did not supply.
AWS: AutoScalingGroup.LaunchTemplate (LaunchTemplateSpecification) always comes back with BOTH LaunchTemplateId and LaunchTemplateName — the service resolves the sibling identifier — plus the Version literal exactly as supplied (including '$Latest'/'$Default').
Emulator: only the identifier used at create time is echoed (name-only when created by name, id-only when created by id), so the other key is absent.
Why: callers (and the AWS CLI's --query expressions) read LaunchTemplateId or LaunchTemplateName depending on their own convention and get a KeyError/None for a group that is perfectly valid.
Fix: resolve and emit both identifiers in the DescribeAutoScalingGroups launch-template projection.
Traces: autoscaling.asg-reflects-launch-template.

Cause 3 of 7: MixedInstancesPolicy defaults are not filled in on read.
AWS: a MixedInstancesPolicy stored without explicit distribution settings reads back with the service's defaults materialised — notably InstancesDistribution.OnDemandAllocationStrategy (a non-null string), not a null/absent member.
Emulator: OnDemandAllocationStrategy comes back None.
Why: a caller that round-trips a described group back into UpdateAutoScalingGroup now sends null for a member AWS always populates, and cannot tell which allocation strategy the group actually uses.
Fix: default materialisation for InstancesDistribution in the group's state/describe projection.
Traces: autoscaling.mixed-instances-policy-overrides-launch.

Cause 4 of 7: scale-in does not honour TerminationPolicies.
AWS: with TerminationPolicies=['OldestInstance'] a scale-in terminates the OLDEST instance and keeps the newer one.
Emulator: the newer instance is terminated and the oldest survives (the policy is inverted or ignored).
Why: the single most load-bearing scale-in guarantee is wrong, so any capacity-planning or rolling-replacement scenario selects the wrong victim.
Fix: victim selection in the scale-in/reconciliation path of the Auto Scaling crate's group state.
Traces: autoscaling.termination-policy-selects-instance.

Cause 5 of 7: an ASG-launched instance does not inherit the launch template's TagSpecifications.
AWS: an instance launched by a group carries the launch template's instance tags in addition to aws:autoscaling:groupName (and the group's PropagateAtLaunch tags).
Emulator: the instance carries only aws:autoscaling:groupName; the template's own tags are dropped.
Why: cost allocation, discovery filters and IAM conditions keyed on template tags all miss the instance. Open issue 20261106T015719Z-autoscaling-ec2-auto-scaling-launch-from-a-launch-template-loses.md reports the same loss under a different service heading and does not carry this check id.
Fix: tag composition in the group's launch path (template TagSpecifications + propagated group tags + aws: tags).
Traces: autoscaling.deploy-launch-template-asg-instance-launch.

Cause 6 of 7: an Auto Scaling group declared by a CloudFormation stack is never visible to DescribeAutoScalingGroups.
AWS: once the stack reaches CREATE_COMPLETE, DescribeAutoScalingGroups(AutoScalingGroupNames=[<physical id>]) returns exactly that group.
Emulator: the response's AutoScalingGroups list is empty, so the SDK call raises IndexError on [0].
Why: no CloudFormation-driven Auto Scaling scenario can be observed at all; the stack reports success for a resource that does not exist in the Auto Scaling service.
Fix: provisioning of AWS::AutoScaling::AutoScalingGroup in the CloudFormation resource handler so it creates the group in the Auto Scaling service's state.
Traces: autoscaling.deploy-asg-cloudformation-scaling-behavior.

Cause 7 of 7: CloudFormation stack Outputs carry no OutputValue.
AWS: DescribeStacks Outputs entries each carry OutputKey AND OutputValue once the stack is CREATE_COMPLETE.
Emulator: the OutputValue key is absent, so reading it raises KeyError and nothing downstream of the stack can be addressed.
Why: every scenario that passes a created resource's id out of a stack is blocked. Open issue 20261024T015148Z-cloudformation-stack-outputs-are-unusable-and-create-complete.md reports this for other checks and does not carry this check id.
Fix: Output resolution in the CloudFormation stack handler.
Traces: autoscaling.deploy-scaling-policy-cloudformation-triggers-scaling.

## Evidence
Behavioral replay of simparity-behavioral-aws-v1 against the emulator built from this checkout (50/414 passed; emulator up).

Cause 1 — autoscaling.create-asg-state-transition [error] and autoscaling.op-contract-suspend-processes [error]: KeyError: 'SuspendedProcesses' raised on the group dict returned by DescribeAutoScalingGroups.
Cause 2 — autoscaling.asg-reflects-launch-template [error]: KeyError: 'LaunchTemplateId' on the described group's LaunchTemplate (the group was created by LaunchTemplateName).
Cause 3 — autoscaling.mixed-instances-policy-overrides-launch [mismatch]: AssertionError: OnDemandAllocationStrategy must be filled with a valid default, got None.
Cause 4 — autoscaling.termination-policy-selects-instance [mismatch]: AssertionError: TerminationPolicies=['OldestInstance'] must terminate the oldest instance i-00000000000000001 and keep i-00000000000000002; survivors were ['i-00000000000000001']. (The instance ids are per-run values; the requirement is oldest-terminated.)
Cause 5 — autoscaling.deploy-launch-template-asg-instance-launch [mismatch]: assert instance_tags.get('sim2real-marker') == marker -> AssertionError: {'aws:autoscaling:groupName': 'sim2real-behav-asg-68b6663cb0ae'} — the instance's full tag set is just the one aws: tag.
Cause 6 — autoscaling.deploy-asg-cloudformation-scaling-behavior [error]: IndexError: list index out of range at autoscaling.describe_auto_scaling_groups(AutoScalingGroupNames=[asg_name])['AutoScalingGroups'][0] for the group the stack declares.
Cause 7 — autoscaling.deploy-scaling-policy-cloudformation-triggers-scaling [error]: KeyError: 'OutputValue' reading the stack's Outputs.

## Steps to Reproduce
1. Replay the behavioral checks in trace_ids against the emulator built from this checkout.
2. By hand:
   a. Create a group and call DescribeAutoScalingGroups -> expect a SuspendedProcesses key (empty list) on the group, and a LaunchTemplate carrying BOTH LaunchTemplateId and LaunchTemplateName plus the Version literal, whichever identifier was used at create time.
   b. Create a group with a MixedInstancesPolicy with no InstancesDistribution -> expect InstancesDistribution.OnDemandAllocationStrategy to read back non-null.
   c. Run a group at DesiredCapacity=2 with TerminationPolicies=['OldestInstance'], then lower DesiredCapacity to 1 -> expect the OLDEST instance to be terminated.
   d. Launch via a launch template that carries TagSpecifications instance tags -> expect those tags on the instance alongside aws:autoscaling:groupName.
   e. Deploy a stack containing AWS::AutoScaling::AutoScalingGroup -> expect DescribeAutoScalingGroups to return the group, and DescribeStacks Outputs to carry OutputValue for every OutputKey.
