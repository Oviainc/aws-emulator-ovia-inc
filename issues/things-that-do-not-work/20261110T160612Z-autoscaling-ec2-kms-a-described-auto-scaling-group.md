Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2__kms.asglf-group-round-trips-bounds-template-reference-and-defaults
Issue-Key: autoscaling__ec2__kms.asglf-group-round-trips-bounds-template-reference-and-defaults|run#11|AssertionError|assert defaults == {
Issue-Commit: b11785bf9bf8af57a00ad7e4d814b1c5f1d66117

# autoscaling__ec2__kms: a described Auto Scaling group's LoadBalancerNames, TargetGroupARNs, SuspendedProcesses and EnabledMetrics are null instead of empty lists

> _Note: this issue was identified against the code base at commit `b11785bf9bf8af57a00ad7e4d814b1c5f1d66117`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAutoScalingGroups

## Description
Cause 1 of 1: the list-typed members of a described Auto Scaling group deserialise to null rather than to empty lists.

AWS:      Smithy: AutoScalingGroup.LoadBalancerNames -> list LoadBalancerNames, .TargetGroupARNs -> list TargetGroupARNs, .SuspendedProcesses -> list SuspendedProcesses, .EnabledMetrics -> list EnabledMetrics (com.amazonaws.autoscaling#AutoScalingGroup). DescribeAutoScalingGroups emits every one of these members for every group; a freshly created group with no load balancer, no target group, no suspended process and no metrics collection enabled reads back an EMPTY list for each, never a null.
Emulator: the group's defaults read back as `{'HealthCheckType': 'EC2', 'HealthCheckGracePeriod': 0, 'DefaultCooldown': 300, 'TerminationPolicies': ['Default'], 'NewInstancesProtectedFromScaleIn': False, 'LoadBalancerNames': None, 'TargetGroupARNs': None, 'SuspendedProcesses': None, 'EnabledMetrics': None, 'Instances': []}` — four list members are null while the scalar defaults and `Instances` are right.
Why:      SDK callers iterate these members unguarded because the model types them as lists; a null makes `for m in group["EnabledMetrics"]` fail, and a client cannot tell "no metrics enabled" from "not reported".
Fix:      the DescribeAutoScalingGroups response serializer / the group-state projection for autoscaling — always emit these list members, empty when the group holds nothing.
Traces:   autoscaling__ec2__kms.asglf-group-round-trips-bounds-template-reference-and-defaults.

This is the same defect family as the issue filed in this pass for service autoscaling__ec2 (checks asgeni-asg-config-round-trips… / asghc-asg-round-trips…), kept separate because it is grounded in a check the suite reports under a different service heading.

## Evidence
Expected (AWS): `LoadBalancerNames: []`, `TargetGroupARNs: []`, `SuspendedProcesses: []`, `EnabledMetrics: []` on a newly created group.
Actual (emulator):
  File "check_autoscaling__ec2__kms_asglf_group_round_trips_bounds_template_reference_and_defaults.py", line 41, in run
    assert defaults == {...}
  AssertionError: defaults were not applied: {'HealthCheckType': 'EC2', 'HealthCheckGracePeriod': 0, 'DefaultCooldown': 300, 'TerminationPolicies': ['Default'], 'NewInstancesProtectedFromScaleIn': False, 'LoadBalancerNames': None, 'TargetGroupARNs': None, 'SuspendedProcesses': None, 'EnabledMetrics': None, 'Instances': []}

## Steps to Reproduce
1. CreateLaunchTemplate and CreateAutoScalingGroup naming it, with no LoadBalancerNames/TargetGroupARNs and without EnableMetricsCollection.
2. DescribeAutoScalingGroups(AutoScalingGroupNames=[name]).
3. Inspect `LoadBalancerNames`, `TargetGroupARNs`, `SuspendedProcesses`, `EnabledMetrics`: all null; AWS returns `[]` for each.
