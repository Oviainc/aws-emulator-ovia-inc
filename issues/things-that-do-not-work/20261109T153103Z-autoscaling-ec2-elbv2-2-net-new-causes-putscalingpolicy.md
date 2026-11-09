Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2__elbv2.asg-cooldown-prevents-premature-ec2-termination autoscaling__ec2__elbv2.asg-scale-up-blocked-after-target-group-deleted
Issue-Key: autoscaling__ec2__elbv2|PutScalingPolicy|NotImplemented
Issue-Key: autoscaling__ec2__elbv2.asg-scale-up-blocked-after-target-group-deleted|_poll_until#15|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s (last observed: {last!r})")
Issue-Commit: 6bab9504f4457a639e729a9dc2b08c46f536304f

# autoscaling__ec2__elbv2: 2 net-new causes — PutScalingPolicy answers 501 NotImplemented, and an ASG never registers its instances into a target group so scale-up after target-group deletion cannot be observed

> _Note: this issue was identified against the code base at commit `6bab9504f4457a639e729a9dc2b08c46f536304f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutScalingPolicy

## Description
Two net-new root causes (Cause 1 of 2, Cause 2 of 2). Related open filings that do NOT carry these replay keys: 20261029T172317Z-autoscaling-auto-scaling-6-fidelity-divergences-putscalingpolicy-rejects-none.md concerns PutScalingPolicy's validation (not a 501), and 20261107T075601Z-autoscaling-ec2-elbv2-auto-scaling-elbv2-asg-instances-never-register-with.md / 20261107T173527Z-autoscaling-ec2-elbv2-auto-scaling-elbv2-asg-instances-are-never-registered.md own other registration checks; the suite marks neither id below OWNED or ALREADY FILED.

Cause 1 of 2: PutScalingPolicy is a scaffold stub — it answers HTTP 501 with error code NotImplemented instead of creating the policy.
AWS: Smithy (autoscaling) PutScalingPolicy takes AutoScalingGroupName, PolicyName, PolicyType, AdjustmentType, ScalingAdjustment, Cooldown etc. and returns PolicyARN plus Alarms; its modeled errors are LimitExceededFault, ResourceContentionFault and ServiceLinkedRoleFailure. NotImplemented is not among them, and a valid request is answered 200 with the policy's ARN. The policy's Cooldown is what makes a scaling activity suppress a following one.
Emulator: PutScalingPolicy answers "An error occurred (NotImplemented) when calling the PutScalingPolicy operation: operation PutScalingPolicy is scaffolded but not implemented in this emulator".
Why: a client cannot create any scaling policy, so cooldown-governed behaviour — the whole point of the check — is unreachable, and it receives an error code that is not in the operation's modeled error set, so generic AWS error handling (retry/backoff classification) cannot place it.
Fix: implement the autoscaling PutScalingPolicy operation handler (services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/src/ops/put_scaling_policy.rs) so the policy is stored with its Cooldown and returned by DescribePolicies, and the scaling engine honours the cooldown window.
Traces: autoscaling__ec2__elbv2.asg-cooldown-prevents-premature-ec2-termination.

Cause 2 of 2: an Auto Scaling group's instances are never registered into its attached target group, so DescribeTargetHealth stays empty and a scale-up that must be blocked after the target group is deleted cannot be observed.
AWS: Smithy (elasticloadbalancingv2) DescribeTargetHealth -> TargetHealthDescriptions[]; Smithy (autoscaling) CreateAutoScalingGroup.TargetGroupARNs / AttachLoadBalancerTargetGroups bind an ASG to target groups, and AWS registers every instance the group launches into each attached target group, so the target group lists those instance ids. After the target group is deleted, the group's subsequent launch attempt fails and the failure is reported on the scaling activity.
Emulator: DescribeTargetHealth for the ASG's target group returns [] for the whole 180 s window ("condition not met within 180.0s (last observed: [])") — no instance is ever registered, so the pre-condition the check needs never holds.
Why: the ASG/ELB binding is reported as configured while producing no targets, so a client sees a load balancer with no backends and an Auto Scaling group that believes it is wired up; nothing distinguishes that from a target group that is simply still filling.
Fix: the Auto Scaling group's instance-launch path must register each launched instance into every attached target group (TargetGroupARNs / traffic sources) in the ELBv2 target-group state, and report a launch failure on the scaling activity when an attached target group no longer exists.
Traces: autoscaling__ec2__elbv2.asg-scale-up-blocked-after-target-group-deleted.

## Evidence
Replay of suite simparity-behavioral-aws-v1: autoscaling__ec2__elbv2 7/60 (12%), 53 failed; two are neither OWNED nor ALREADY FILED.

Cause 1: [error] autoscaling__ec2__elbv2.asg-cooldown-prevents-premature-ec2-termination
  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the PutScalingPolicy operation: operation PutScalingPolicy is scaffolded but not implemented in this emulator
  Expected: HTTP 200 with PolicyARN. Actual: 501 NotImplemented, an error code the operation does not model.

Cause 2: [mismatch] autoscaling__ec2__elbv2.asg-scale-up-blocked-after-target-group-deleted
  AssertionError: condition not met within 180.0s (last observed: [])
  Expected: the ASG's instances appear in DescribeTargetHealth for the attached target group. Actual: the list stays empty.

Smithy consulted: autoscaling PutScalingPolicy (output PolicyARN/Alarms; errors LimitExceededFault, ResourceContentionFault, ServiceLinkedRoleFailure), CreateAutoScalingGroup.TargetGroupARNs; elasticloadbalancingv2 DescribeTargetHealth -> TargetHealthDescriptions[].

## Steps to Reproduce
1. Clone the repo and replay the behavioral suite; both ids in trace_ids reproduce.
2. Cause 1: create an Auto Scaling group, then autoscaling.put_scaling_policy(AutoScalingGroupName=..., PolicyName=..., PolicyType='SimpleScaling', AdjustmentType='ChangeInCapacity', ScalingAdjustment=-1, Cooldown=300). Expect HTTP 200 with PolicyARN; the emulator answers 501 NotImplemented.
3. Cause 2: create a target group, create an ASG with TargetGroupARNs=[that group] and a nonzero DesiredCapacity, and poll elbv2.describe_target_health on it. Expect the launched instance ids; the emulator returns [] for 180 s.
4. Re-replay the ids in trace_ids to confirm the fix.
