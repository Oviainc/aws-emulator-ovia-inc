Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: application-autoscaling__ecs.two-policies-triggered-together-scale-by-the-largest-not-the-sum
Issue-Key: application-autoscaling__ecs.two-policies-triggered-together-scale-by-the-largest-not-the-sum|_arbitrate#22|AssertionError|assert len(set(settle)) == 1, (
Issue-Commit: 77b7d3dfd1a46160280007e44fe4e2e402c212f4

# Two simultaneously-triggered step-scaling policies are BOTH applied to the ECS desiredCount (5 -> 7 -> 16) instead of only the largest-capacity policy executing once

> _Note: this issue was identified against the code base at commit `77b7d3dfd1a46160280007e44fe4e2e402c212f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutScalingPolicy

## Description
One net-new root cause in the Application Auto Scaling scaling-execution path for the `ecs` service namespace (ServiceNamespace=ecs, ScalableDimension=ecs:service:DesiredCount).

Cause 1 of 1: when two step-scaling policies on the SAME scalable target are triggered at the same time, the emulator executes both of them and applies both adjustments to the ECS service's desiredCount in sequence, so the capacity settles on the accumulated result of both policies instead of the single arbitrated outcome.

AWS: Application Auto Scaling arbitrates between scaling policies that are triggered at the same time on one scalable target: per the Application Auto Scaling user guide (target-tracking-scaling-policy-overview), "If multiple scaling policies instruct the scalable target to scale out or in at the same time, Application Auto Scaling scales based on the policy that provides the largest capacity for both scale in and scale out." The losing policy is fully suppressed — exactly ONE scaling decision is taken per arbitration event, so the scalable target's capacity moves ONCE and then stays put, and DescribeScalingActivities (even with IncludeNotScaledActivities=true) carries an activity attributable only to the winning policy, named in its Cause field. Smithy: PutScalingPolicy(PolicyName, ServiceNamespace, ResourceId, ScalableDimension, PolicyType, StepScalingPolicyConfiguration) and DescribeScalingActivities(ScalingActivities) model the API surface but say nothing about arbitration; the arbitration rule is the documented behavior cited above, and the measured outcome (the winner's adjustment applied once, with no second movement in the following ~45s) is recorded behavior from the check's own against-AWS measurement.

Emulator: with a scalable target registered at desiredCount 5, two StepScaling policies (ScalingAdjustment +2 and +9, both ChangeInCapacity, both breaching in the same window) are both executed. The ECS service's desiredCount read via DescribeServices moves through [7, 16, 16, 16, 16, 16] across the 45s settle window: first +2 is applied (5 -> 7), then, after the arbitration moment, +9 is applied on top of the already-adjusted capacity (7 -> 16). Two triggers therefore produce two capacity changes and a final capacity that is the sum of both adjustments, rather than one arbitrated outcome.

Why: a client relying on AWS's arbitration gets capacity it never asked for — the target overshoots to the sum of every policy that happened to breach together, which on a real fleet means over-provisioning (and, in the scale-in direction, over-shrinking) proportional to the number of policies attached. The scaling history is equally wrong: a client reading DescribeScalingActivities sees two scaling decisions for one event and cannot tell which policy actually governed capacity, and anything that reconciles desiredCount against the policy that won will disagree with the emulator's state.

Fix: the scaling-execution path's arbitration step for a scalable target — when more than one policy is triggered for the same target in the same evaluation, compute each policy's resulting capacity, apply only the policy yielding the largest capacity (exactly once, against the capacity as of the arbitration, not re-applied against the already-adjusted value), suppress the other policy's execution entirely, and record exactly one activity for the winner. The capacity writer for ecs:service:DesiredCount must not apply a second adjustment for an arbitration event it already settled.

Relation to what is already open: 20261029T175650Z-application-autoscaling-ecs-5-net-new-root-causes.md (Cause 5) described the same apply-both implementation as observed through a SURPLUS ACTIVITY attributable to the losing policy. That issue's replay key for this check no longer matches: the check now fails earlier, on the CAPACITY itself moving twice, which is the apply-both behavior observed on the state rather than on the activity ledger. Filed separately so this symptom is not lost; a fixer closing the arbitration path should expect both to resolve together.

Traces: application-autoscaling__ecs.two-policies-triggered-together-scale-by-the-largest-not-the-sum.

## Evidence
Cause 1 — check application-autoscaling__ecs.two-policies-triggered-together-scale-by-the-largest-not-the-sum, verdict mismatch.

Expected (AWS): after two step policies with ScalingAdjustment +2 and +9 are triggered together on one ecs:service:DesiredCount target, the ECS desiredCount changes ONCE and every subsequent read inside the settle window returns the same value (set(settle) has exactly one element); only the winning (largest-capacity) policy appears in DescribeScalingActivities.

Actual (emulator): the desiredCount readings across the 45s settle window were [7, 16, 16, 16, 16, 16] against a pre-arbitration capacity of 5 — i.e. +2 applied (5 -> 7), then +9 applied on top (7 -> 16). Assertion raised:
  AssertionError: leg 1: ECS desiredCount for '<ecs service>' moved through [7, 16, 16, 16, 16, 16] over the 45s after the arbitration. Two triggers must produce ONE outcome; a second adjustment arriving during this window is the apply-both implementation caught in the act
(raised at _arbitrate, `assert len(set(settle)) == 1`). The service name, the generated policy/alarm names and the timestamps are per-run values; the requirement is that the capacity settles on ONE value for ONE arbitration event and that the second adjustment is never applied.

The two adjustment values and the start capacity are the check's inputs (+2, +9, start 5); the defect is the double application, not the specific numbers.

## Steps to Reproduce
1. ECS: create a cluster and an ACTIVE service with desiredCount=5.
2. application-autoscaling: RegisterScalableTarget(ServiceNamespace=ecs, ResourceId=service/<cluster>/<service>, ScalableDimension=ecs:service:DesiredCount, MinCapacity=1, MaxCapacity=60).
3. PutScalingPolicy twice on that target with PolicyType=StepScaling and StepScalingPolicyConfiguration AdjustmentType=ChangeInCapacity: one with StepAdjustments=[{MetricIntervalLowerBound: 0.0, ScalingAdjustment: 2}], one with [{MetricIntervalLowerBound: 0.0, ScalingAdjustment: 9}].
4. CloudWatch: PutMetricAlarm for each policy on the same custom metric with AlarmActions=[<that policy's ARN>], ComparisonOperator=GreaterThanThreshold, Threshold=50.
5. PutMetricData above the threshold until BOTH alarms report StateValue=ALARM within the same window (DescribeAlarms StateUpdatedTimestamp values within ~60s of each other).
6. Poll ECS DescribeServices for desiredCount to move off 5, then keep reading it every ~9s for 45s.
Expected: it moves once and then holds a single value; observed: it moves to 7 and then again to 16 (both adjustments applied).
7. Also DescribeScalingActivities(ServiceNamespace=ecs, ResourceId=..., IncludeNotScaledActivities=True) and confirm only the winning policy is named in any Cause.
Replay the check id in trace_ids to confirm the fix.
