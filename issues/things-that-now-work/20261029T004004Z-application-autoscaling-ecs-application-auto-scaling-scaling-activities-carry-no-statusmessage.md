Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: application-autoscaling__ecs.step-scaling-adjustment-is-clamped-into-the-current-registered-bounds
Issue-Key: application-autoscaling__ecs.step-scaling-adjustment-is-clamped-into-the-current-registered-bounds|_run_leg#6|AssertionError|assert f'{after}' in newest.get('StatusMessage', ''), (
Issue-Commit: f21d2937dde7ed50f2d73e4c6f2a7c8bdd006019

# Application Auto Scaling scaling activities carry no StatusMessage, so a clamped step-scaling activity never reports the capacity it set

> _Note: this issue was identified against the code base at commit `f21d2937dde7ed50f2d73e4c6f2a7c8bdd006019`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeScalingActivities

## Description
One root cause, grounded in the single surviving failing check of this bucket.

Cause 1 of 1: DescribeScalingActivities omits ScalingActivity.StatusMessage entirely.

AWS: ScalingActivity.StatusMessage is modeled (Smithy: com.amazonaws.applicationautoscaling#ScalingActivity member StatusMessage -> XmlString, optional, "A simple message about the current status of the scaling activity"; siblings ActivityId/Description/Cause/StatusCode are @required). Recorded behavior: once an activity has moved past its initial pending states (StatusCode no longer PendingBeforeAlarmThreshold / PendingBeforeCooldownAfterInternal / PendingSchedule), AWS populates StatusMessage with a human-readable progress line that NAMES the capacity the activity set — for an ecs:service:DesiredCount activity whose Description is "Setting desired count to <N>.", the StatusMessage is AWS's fixed status text for that activity class with <N> substituted from the capacity actually applied (the clamped value, here MaxCapacity/MinCapacity of the registered scalable target). The fixed wording is AWS's; the only variable part is <N>, which comes from the request's adjustment clamped against the target's registered bounds, and is never a literal from any one recording. No HTTP status or error code is involved: the call succeeds with HTTP 200 in both AWS and the emulator.

Emulator: the member is absent from every returned activity, so the SDK surfaces StatusMessage as None. The replay shows the emulator does clamp correctly — ECS desiredCount lands on the MaxCapacity bound of 6, the activity's Description is "Setting desired count to 6.", StatusCode is Successful/InProgress and NotScaledReasons is empty — and then fails only on the missing message: "leg out: the activity status message was None, which does not name the clamped count 6".

Why: StatusMessage is the only field that reports what capacity a scaling activity actually settled on and how far the fulfilment got; Description states the intent, not the outcome. A client (or operator tooling, or a CLI-driven runbook) that reads StatusMessage to confirm a clamped step adjustment landed on the registered bound gets a null and cannot distinguish "clamped to the bound and applied" from "nothing happened". Consumers that string-match or log the message see an empty field on every activity this service records.

Fix: the DescribeScalingActivities serializer plus the scaling-activity state that records activities — services/service-com-amazonaws-applicationautoscaling-anyscalefrontendservice/src/ops/describe_scaling_activities.rs and this service's scaling-activity state module must persist and emit a StatusMessage alongside Description/StatusCode when an activity leaves its pending state, templated on the applied (post-clamp) capacity.

Traces: application-autoscaling__ecs.step-scaling-adjustment-is-clamped-into-the-current-registered-bounds. Related but NOT covering this: open issue 20261022T182203Z-application-autoscaling-application-auto-scaling-minted-cloudwatch-alarms-scalable-target.md (cause 6 there is about activities reporting fulfilment that never reached the resource, under the separate `application-autoscaling` check bucket) and 20261024T115439Z-application-autoscaling-application-auto-scaling-scheduled-actions-never-fire-target.md; neither mentions StatusMessage.

## Evidence
Failing check: [mismatch] application-autoscaling__ecs.step-scaling-adjustment-is-clamped-into-the-current-registered-bounds

Assertion that failed (check line 1050 of the replayed source):
    assert f'{after}' in newest.get('StatusMessage', ''), (
        f'{leg}: the activity status message was {newest.get("StatusMessage")!r}, which does not name the clamped count {after}')
AssertionError: leg out: the activity status message was None, which does not name the clamped count 6

Expected (AWS): the newest non-pending activity for resource service/<cluster>/<service>, dimension ecs:service:DesiredCount, carries StatusMessage whose text includes the applied count (6 = MaxCapacity, after a ChangeInCapacity of +10 from a start of 4 against bounds Min 4 / Max 6).
Actual (emulator): the activity is returned with ActivityId, Description='Setting desired count to 6.', StatusCode in (Successful, InProgress), NotScaledReasons empty, and NO StatusMessage member at all (None).

Everything else in this scenario matched: the clamp itself, the Description text, the StatusCode and the absence of NotScaledReasons. The sole divergence is the missing StatusMessage.

## Steps to Reproduce
Boot the emulator and drive with botocore:
1. ecs.create_cluster + register a task definition and ecs.create_service with desiredCount=4.
2. application-autoscaling.register_scalable_target(ServiceNamespace='ecs', ResourceId='service/<cluster>/<service>', ScalableDimension='ecs:service:DesiredCount', MinCapacity=4, MaxCapacity=6).
3. application-autoscaling.put_scaling_policy with PolicyType='StepScaling' and a single step adjustment ChangeInCapacity=+10 (lower bound 0.0), and a CloudWatch alarm on a custom metric wired to that policy.
4. cloudwatch.put_metric_data to breach the alarm threshold until describe_alarms reports StateValue='ALARM'.
5. Poll ecs.describe_services — desiredCount moves from 4 to 6 (clamped at MaxCapacity; this part is already correct).
6. application-autoscaling.describe_scaling_activities(ServiceNamespace='ecs', ResourceId='service/<cluster>/<service>', ScalableDimension='ecs:service:DesiredCount', IncludeNotScaledActivities=True) and read the newest activity whose Description is 'Setting desired count to 6.' and whose StatusCode is not one of the Pending* codes.
Expected: that activity has a StatusMessage naming the applied count 6. Observed: StatusMessage is absent/None. Repeat symmetrically with ChangeInCapacity=-10 from a start of 6 to see the MinCapacity leg.
