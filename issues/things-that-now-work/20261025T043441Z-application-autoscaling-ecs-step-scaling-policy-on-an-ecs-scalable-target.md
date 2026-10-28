Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: application-autoscaling__ecs.step-scaling-adjustment-is-clamped-into-the-current-registered-bounds
Issue-Key: application-autoscaling__ecs.step-scaling-adjustment-is-clamped-into-the-current-registered-bounds|_run_leg#6|AssertionError|raise AssertionError(f'{exc} -- DEBUG all activities: {dump!r}') from None
Issue-Commit: 77162043f7392d23f93660cf14c6adb796d45cd4

# Step-scaling policy on an ECS scalable target never executes: desiredCount never moves and DescribeScalingActivities stays empty, so the MaxCapacity/MinCapacity clamp is unobservable

> _Note: this issue was identified against the code base at commit `77162043f7392d23f93660cf14c6adb796d45cd4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutScalingPolicy

## Description
One net-new root cause, grounded in the one surviving (not ALREADY FILED, not OWNED) failure of the application-autoscaling__ecs bucket in this sweep.

Related open issue, which does NOT carry this replay key (the previous failure was a bare 60 s harness timeout; this sweep fails with an explicit assertion plus an empty activity dump, so the suite did not mark the check OWNED): 20261024T115439Z-application-autoscaling-ecs-step-scaling-adjustment-on-an-ecs-scalable-target.md. This filing re-grounds the same observable behavior in the current replay; it is not a new claim about a different operation.

Cause 1 of 1: a step-scaling policy whose alarm breaches is never executed on an ecs:service:DesiredCount scalable target, so neither the adjustment nor its clamp into the registered bounds ever happens.
AWS: Smithy PutScalingPolicy (application-autoscaling, awsQuery) takes PolicyType plus StepScalingPolicyConfiguration and returns PolicyARN + Alarms; it models ValidationException, ObjectNotFoundException, LimitExceededException, FailedResourceAccessException, ConcurrentUpdateException (all 400 except the 500 server ones). DescribeScalingActivities returns ScalingActivities. The model does not state the arithmetic, so for the behavior itself this is recorded behavior only: when the policy's CloudWatch alarm transitions to ALARM, Application Auto Scaling computes new capacity from ScalingAdjustment/AdjustmentType, CLAMPS the result into the MinCapacity/MaxCapacity registered at dispatch time, still performs the scaling (it is not refused as AlreadyAtMaxCapacity/AlreadyAtMinCapacity), sets the ECS service's desiredCount to the clamped value, and records a scaling activity whose Description names the clamped desired count (recorded: "Setting desired count to N.") with StatusMessage "...Waiting for change to be fulfilled by ecs." and no NotScaledReasons. The symmetric case clamps against MinCapacity and never yields 0 or a negative desiredCount.
Emulator: nothing happens. After the alarm is driven into ALARM, the ECS service's desiredCount stays at its starting value (observed: still 4 after a ChangeInCapacity of +10 against MaxCapacity 6, polled for 45 s) and DescribeScalingActivities for that ResourceId/ScalableDimension returns an empty ScalingActivities list (debug dump: []). Both the out-clamp leg and the in-clamp leg are affected; PutScalingPolicy itself answers 200.
Why: a client's step-scaling policy is inert — capacity never responds to a breach, so autoscaled services silently stay at whatever capacity they were left at, and the bound re-read that protects against over-scaling can never be exercised. Worse, the control plane reports success, so the misconfiguration is invisible: a client sees a policy with an ARN and alarms attached and no scaling.
Fix: the alarm-breach / policy-execution path of this service — on an alarm action, resolve the policy, compute the adjustment, clamp against the bounds read at dispatch time, mutate the ECS service's desiredCount, and append a ScalingActivity readable by DescribeScalingActivities. Belongs in the application-autoscaling crate's scaling-execution/state module (put_scaling_policy's handler only stores the policy) together with its call into ECS UpdateService.
Traces: application-autoscaling__ecs.step-scaling-adjustment-is-clamped-into-the-current-registered-bounds.

## Evidence
Replay report (simparity-behavioral-ga-prod-v1), service bucket application-autoscaling__ecs 1/7 (14%), 6 failed; 5 of those are OWNED/ALREADY FILED and skipped.

[mismatch] application-autoscaling__ecs.step-scaling-adjustment-is-clamped-into-the-current-registered-bounds:
  AssertionError: leg out: ECS desiredCount for 'sim2real-behav-aas-clampout-svc-<id>' moving off 4 after a ChangeInCapacity +10 that overshoots the MaxCapacity of 6 not observed within 45.0s; last saw 4 -- DEBUG all activities: []

Expected (recorded on real AWS): desiredCount moves to the clamped bound (6) and DescribeScalingActivities reports one activity for the resource whose Description names the clamped desired count. Observed: desiredCount unchanged, zero scaling activities.

## Steps to Reproduce
1. ecs.create_cluster, ecs.register_task_definition, ecs.create_service with launchType EC2 and desiredCount 4 on a cluster with no container instances.
2. application-autoscaling.register_scalable_target(ServiceNamespace='ecs', ResourceId='service/<cluster>/<service>', ScalableDimension='ecs:service:DesiredCount', MinCapacity=4, MaxCapacity=6).
3. application-autoscaling.put_scaling_policy(PolicyType='StepScaling', StepScalingPolicyConfiguration={'AdjustmentType':'ChangeInCapacity','StepAdjustments':[{'MetricIntervalLowerBound':0,'ScalingAdjustment':10}]}).
4. Drive the policy's CloudWatch alarm into ALARM (put_metric_data over the threshold, or cloudwatch.set_alarm_state on the minted alarm).
5. Poll ecs.describe_services for desiredCount and application-autoscaling.describe_scaling_activities for the resource. AWS clamps to MaxCapacity (6) and records the activity; the emulator leaves desiredCount at 4 and returns ScalingActivities: [].
6. Repeat with MinCapacity=4/ScalingAdjustment=-10 for the in-clamp direction: same inertness.
