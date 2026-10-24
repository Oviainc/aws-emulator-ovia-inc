Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: application-autoscaling__ecs.step-scaling-adjustment-is-clamped-into-the-current-registered-bounds
Issue-Key: application-autoscaling__ecs.step-scaling-adjustment-is-clamped-into-the-current-registered-bounds|_run_leg#6|Timeout|time.sleep(5.0)
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# Step-scaling adjustment on an ECS scalable target is never applied, so it is never clamped into the current registered bounds

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutScalingPolicy

## Description
One root cause, grounded in the single surviving failing check of this check-id bucket (the other two failures in it are owned by 20261024T081305Z-application-autoscaling-application-auto-scaling-getpredictivescalingforecast-fabricates-numeric-forecasts-for.md and are skipped).

Cause 1 of 1: a step-scaling adjustment that would take an ECS service outside the scalable target's CURRENT MinCapacity/MaxCapacity is never applied at all, so the clamping behavior the check measures never becomes observable.
AWS: Smithy PutScalingPolicy accepts a StepScalingPolicyConfiguration (ScalingAdjustment/AdjustmentType/StepAdjustments) and models ValidationException and ObjectNotFoundException; MinCapacity/MaxCapacity on the target are plain integers with no modeled range, so the bound is service-enforced. Recorded behavior: when the policy's alarm breaches, Application Auto Scaling computes the new desiredCount from the adjustment and then clamps it into the bounds registered at dispatch time (re-reading the bounds, not the bounds in force when the policy was created), records the change as a scaling activity, and sets the ECS service's desiredCount to the clamped value.
Emulator: nothing is applied — the check exhausts the replay deadline (`Check timed out after 60.0s`) inside its bounded poll of the ECS service's desiredCount / DescribeScalingActivities waiting for the adjusted-and-clamped value to appear.
Why: a client's step-scaling policy never moves the ECS service, and because the adjustment never lands the bound re-read that protects against over-scaling is untested and unimplemented; capacity stays wherever it was regardless of the breach.
Fix: the alarm-breach / step-scaling execution path in this service — compute the adjustment, clamp against the bounds read at dispatch time, mutate the ECS service's desiredCount, and record the activity. Shares the missing enforcement pipeline reported for the other two Application Auto Scaling check-id buckets in this same sweep; this issue carries the only check id in this bucket.
Traces: application-autoscaling__ecs.step-scaling-adjustment-is-clamped-into-the-current-registered-bounds

## Evidence
Replay report (simparity-behavioral-ga-prod-v1), application-autoscaling__ecs 1/7 (14%), 6 failed. Surviving (not owned, not already filed) failure:

[mismatch] application-autoscaling__ecs.step-scaling-adjustment-is-clamped-into-the-current-registered-bounds — `Error: Check timed out after 60.0s`. The check registers an ecs:service:DesiredCount target, creates a StepScaling policy, narrows the registered bounds, breaches the policy's alarm, and then polls for the resulting desiredCount (clamped into the narrowed bounds) and for the matching scaling activity; its internal poll budget exceeds the 60 s replay deadline, which it reaches because neither the desiredCount nor the activity ever changes.

Handler surface: services/service-com-amazonaws-applicationautoscaling-anyscalefrontendservice/src/ops/put_scaling_policy.rs and this service's state / enforcement module; the ECS side is services/service-com-amazonaws-ecs-amazonec2containerservicev20141113 (UpdateService desiredCount).

## Steps to Reproduce
Boot the emulator and, with botocore:
1. ecs.create_cluster and ecs.create_service (EC2 launch type on an empty cluster, desiredCount 0 so nothing is ever placed).
2. application-autoscaling.register_scalable_target(ServiceNamespace='ecs', ResourceId='service/<cluster>/<service>', ScalableDimension='ecs:service:DesiredCount', MinCapacity=1, MaxCapacity=10).
3. put_scaling_policy(PolicyType='StepScaling', StepScalingPolicyConfiguration with AdjustmentType='ExactCapacity' or a ChangeInCapacity step large enough to exceed the ceiling).
4. register_scalable_target again to narrow the bounds (e.g. Min=1, Max=3).
5. Breach the policy's alarm (cloudwatch.set_alarm_state on the minted alarm, or invoke the policy's alarm action path).
6. Poll ecs.describe_services for desiredCount and application-autoscaling.describe_scaling_activities. AWS applies the adjustment clamped to the CURRENT MaxCapacity (3) within seconds and records the activity; the emulator leaves desiredCount unchanged and records no activity, so the poll runs to its deadline.
