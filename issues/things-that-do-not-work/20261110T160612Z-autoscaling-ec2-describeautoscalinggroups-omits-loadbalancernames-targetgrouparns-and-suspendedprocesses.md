Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2.asgeni-asg-config-round-trips-template-reference-and-defaults autoscaling__ec2.asghc-asg-round-trips-elb-health-check-grace-and-capacity-bounds
Issue-Key: autoscaling__ec2.asgeni-asg-config-round-trips-template-reference-and-defaults|run#19|KeyError|assert (g["Instances"], g["LoadBalancerNames"], g["TargetGroupARNs"]) == ([], [], []), (
Issue-Key: autoscaling__ec2.asghc-asg-round-trips-elb-health-check-grace-and-capacity-bounds|run#13|AssertionError|assert defaults == {"DefaultCooldown": 300, "TerminationPolicies": ["Default"],
Issue-Commit: b11785bf9bf8af57a00ad7e4d814b1c5f1d66117

# autoscaling__ec2: DescribeAutoScalingGroups omits LoadBalancerNames, TargetGroupARNs and SuspendedProcesses instead of reporting them as empty lists

> _Note: this issue was identified against the code base at commit `b11785bf9bf8af57a00ad7e4d814b1c5f1d66117`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAutoScalingGroups

## Description
Cause 1 of 1: the list-typed members of a described Auto Scaling group are absent or null rather than empty lists.

AWS:      Smithy: AutoScalingGroup.LoadBalancerNames -> list LoadBalancerNames, AutoScalingGroup.TargetGroupARNs -> list TargetGroupARNs, AutoScalingGroup.SuspendedProcesses -> list SuspendedProcesses (com.amazonaws.autoscaling#AutoScalingGroup). DescribeAutoScalingGroups serialises each of these members on every group it returns; a group created with no classic load balancer, no target group and no suspended process reads back an EMPTY list for each of them, never a null and never an absent key. The same holds for the other list members already present in the emulator's answer (Instances, TerminationPolicies).
Emulator: the members are missing entirely or deserialise to null. One check dies on `KeyError: 'LoadBalancerNames'` while asserting `(g["Instances"], g["LoadBalancerNames"], g["TargetGroupARNs"]) == ([], [], [])`; the other reads the group's defaults back as `{'DefaultCooldown': 300, 'TerminationPolicies': ['Default'], 'NewInstancesProtectedFromScaleIn': False, 'Instances': [], 'TargetGroupARNs': None, 'LoadBalancerNames': None, 'SuspendedProcesses': None}` — the three members come back null while the neighbouring list members are correct.
Why:      every AWS SDK models these members as lists, so client code iterates them unguarded. A caller doing `for arn in group["TargetGroupARNs"]` gets a KeyError (botocore drops absent members) or a `NoneType is not iterable`, and any code that decides "is this group attached to a load balancer / are any processes suspended" cannot distinguish "none" from "unknown".
Fix:      the DescribeAutoScalingGroups response serializer / the group-state projection that builds the AutoScalingGroup structure for autoscaling — emit these list members always, empty when the group holds nothing.
Traces:   autoscaling__ec2.asgeni-asg-config-round-trips-template-reference-and-defaults, autoscaling__ec2.asghc-asg-round-trips-elb-health-check-grace-and-capacity-bounds.

Related but not covering: the open issue 20261107T081710Z-autoscaling-describeautoscalinggroups-omits-suspendedprocesses-the-launch-template-identity-and.md records a neighbouring representation gap under a different replay key, and 20261107T075601Z-autoscaling-ec2-kms-auto-scaling-group-reads-omit-servicelinkedrolearn-and-group.md records the ServiceLinkedRoleARN/defaults family; neither carries the two checks above.

## Evidence
Expected (AWS): DescribeAutoScalingGroups returns `LoadBalancerNames: []`, `TargetGroupARNs: []`, `SuspendedProcesses: []` for a group created with none of them.
Actual (emulator), check asgeni-asg-config-round-trips-template-reference-and-defaults:
  File "check_autoscaling__ec2_asgeni_asg_config_round_trips_template_reference_and_defaults.py", line 55, in run
    assert (g["Instances"], g["LoadBalancerNames"], g["TargetGroupARNs"]) == ([], [], [])
  KeyError: 'LoadBalancerNames'
Actual (emulator), check asghc-asg-round-trips-elb-health-check-grace-and-capacity-bounds:
  AssertionError: service defaults not applied: {'DefaultCooldown': 300, 'TerminationPolicies': ['Default'], 'NewInstancesProtectedFromScaleIn': False, 'Instances': [], 'TargetGroupARNs': None, 'LoadBalancerNames': None, 'SuspendedProcesses': None}
The diverging fields are the three list members; the scalar defaults in the same answer are correct.

## Steps to Reproduce
1. CreateLaunchTemplate, then CreateAutoScalingGroup with MinSize/MaxSize/DesiredCapacity and no LoadBalancerNames and no TargetGroupARNs.
2. Call DescribeAutoScalingGroups(AutoScalingGroupNames=[name]).
3. Inspect the returned group: `LoadBalancerNames`, `TargetGroupARNs` and `SuspendedProcesses` are absent/null; AWS returns `[]` for each.
