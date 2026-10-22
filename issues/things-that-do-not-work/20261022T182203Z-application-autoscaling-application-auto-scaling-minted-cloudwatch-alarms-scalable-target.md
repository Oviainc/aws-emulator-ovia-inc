Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: application-autoscaling.disable-scale-in-withholds-the-scale-in-alarms application-autoscaling.dynamic-scaling-out-suspended-withholds-then-resume-delivers application-autoscaling.target-tracking-alarm-thresholds-are-recomputed-not-recorded application-autoscaling.target-tracking-policy-provisions-cloudwatch-alarms application-autoscaling.ecs-service-target-lifecycle-leaves-the-desired-count-after-deregister application-autoscaling.ecs-cpu-target-tracking-policy-round-trips-and-mints-two-alarms application-autoscaling.register-scalable-target-clamps-capacity-into-bounds application-autoscaling.scaling-activities-newest-first-with-fulfilment-text application-autoscaling.ecs-register-missing-service-rejected application-autoscaling.not-scaled-activity-records-already-at-max-capacity
Issue-Shape-Id: com.amazonaws.applicationautoscaling#AnyScaleFrontendService
Issue-Key: application-autoscaling.disable-scale-in-withholds-the-scale-in-alarms|_alarms_by_kind#15|AssertionError|assert match is not None, (
Issue-Key: application-autoscaling.dynamic-scaling-out-suspended-withholds-then-resume-delivers|_alarms_by_kind#22|AssertionError|assert match is not None, (
Issue-Key: application-autoscaling.target-tracking-alarm-thresholds-are-recomputed-not-recorded|_alarms_by_kind#17|AssertionError|assert match is not None, (
Issue-Key: application-autoscaling.target-tracking-policy-provisions-cloudwatch-alarms|run#20|AssertionError|assert action.startswith(f'{policy_arn}:createdBy/'), (
Issue-Key: application-autoscaling.ecs-service-target-lifecycle-leaves-the-desired-count-after-deregister|run#13|KeyError|assert target['SuspendedState'] == {'DynamicScalingInSuspended': False,
Issue-Key: application-autoscaling.ecs-cpu-target-tracking-policy-round-trips-and-mints-two-alarms|run#23|AssertionError|assert shape == {
Issue-Key: application-autoscaling.register-scalable-target-clamps-capacity-into-bounds|run#18|AssertionError|assert raised['ReadCapacityUnits'] == 5, (
Issue-Key: application-autoscaling.scaling-activities-newest-first-with-fulfilment-text|run#22|AssertionError|assert ddb.describe_table(TableName=table)['Table'][
Issue-Key: application-autoscaling.ecs-register-missing-service-rejected|_client_error#7|AssertionError|raise AssertionError(f'real AWS refuses this call, but it SUCCEEDED and returned {result!r}')
Issue-Key: application-auto-scaling|PutScalingPolicy|ValidationException
Issue-Commit: 242e2a822ca6eb3fec2aad3ba89160f0a6f40669

# Application Auto Scaling: minted CloudWatch alarms, scalable-target representation and capacity enforcement diverge from AWS (8 causes)

> _Note: this issue was identified against the code base at commit `242e2a822ca6eb3fec2aad3ba89160f0a6f40669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutScalingPolicy

## Description
Eight independent root causes, each grounded in failing behavioral checks of this service.

Cause 1 of 8: the trailing segment of a minted alarm name is not a UUID.
AWS: recorded behavior - target-tracking alarms are named 'TargetTracking-<resourceId>-<kind>-<uuid>', where <uuid> is a dashed 8-4-4-4-12 hex UUID and <kind> is AlarmHigh / AlarmLow / ProvisionedCapacityHigh / ProvisionedCapacityLow (Cassandra*ProvisionedCapacity* for the cassandra namespace). The grammar is the requirement; the id itself is per-environment.
Emulator: emits 32 undashed hex characters, e.g. 'TargetTracking-table/<tbl>-AlarmHigh-f1a85113e3d8ac282c637e03660b2964'.
Why: any consumer that parses the alarm kind out of the name (or matches the AWS-documented grammar) cannot identify the alarms, so alarm discovery fails outright.
Fix: PutScalingPolicy handler, alarm-name generation.
Traces: application-autoscaling.disable-scale-in-withholds-the-scale-in-alarms, application-autoscaling.dynamic-scaling-out-suspended-withholds-then-resume-delivers, application-autoscaling.target-tracking-alarm-thresholds-are-recomputed-not-recorded

Cause 2 of 8: alarm AlarmActions are missing the ':createdBy/<uuid>' suffix.
AWS: recorded behavior - each minted alarm has exactly one action, '<policyARN>:createdBy/<uuid>' (dashed UUID).
Emulator: the action is the bare policy ARN.
Why: the action is how a breach reaches the policy; a client (or the emulator's own alarm dispatch) matching the documented '<policyARN>:createdBy/' prefix routes nothing.
Fix: PutScalingPolicy handler, alarm-action construction.
Traces: application-autoscaling.target-tracking-policy-provisions-cloudwatch-alarms

Cause 3 of 8: DescribeScalableTargets omits SuspendedState.
AWS: ScalableTarget.SuspendedState is modeled (Smithy: ScalableTarget member SuspendedState -> SuspendedState{DynamicScalingInSuspended, DynamicScalingOutSuspended, ScheduledScalingSuspended}); recorded behavior - it is always present, defaulting to all three flags false.
Emulator: the member is absent from the described target, so a client reading target['SuspendedState'] raises KeyError.
Why: callers cannot tell whether scaling is suspended, and SDK consumers crash on a member AWS always returns.
Fix: DescribeScalableTargets serializer / scalable-target state default.
Traces: application-autoscaling.ecs-service-target-lifecycle-leaves-the-desired-count-after-deregister

Cause 4 of 8: ECS CPU target-tracking alarm parameters are wrong.
AWS: recorded behavior - for ecs:service:DesiredCount with PredefinedMetricType ECSServiceAverageCPUUtilization the two alarms watch AWS/ECS CPUUtilization and the check pins AWS's measured thresholds and evaluation periods for the configured TargetValue.
Emulator: High=(AWS/ECS, CPUUtilization, Average, GreaterThanThreshold, 60.0, EvaluationPeriods 2, Period 60), Low=(..., LessThanThreshold, 48.0, EvaluationPeriods 15, Period 60) - i.e. the scale-in threshold is computed as 0.8x the target and the scale-out alarm uses 2 evaluation periods, neither of which matches the recorded AWS pair.
Why: the alarm thresholds decide when scaling happens; a client validating its policy's alarms sees different breach points than in AWS.
Fix: PutScalingPolicy handler, per-namespace target-tracking alarm derivation (compare against the check's expected tuple).
Traces: application-autoscaling.ecs-cpu-target-tracking-policy-round-trips-and-mints-two-alarms

Cause 5 of 8: RegisterScalableTarget does not correct current capacity into the registered bounds.
AWS: recorded behavior - registering (or raising) MinCapacity above the resource's current capacity makes Application Auto Scaling move the resource up to the floor; the DynamoDB table's provisioned capacity becomes the new minimum.
Emulator: the table still reads 1 read capacity unit after the floor moved to 5 - the registration is pure bookkeeping.
Why: the bound a caller registered is not enforced, so capacity stays outside the declared range indefinitely.
Fix: RegisterScalableTarget handler plus the enforcement path that mutates the target resource.
Traces: application-autoscaling.register-scalable-target-clamps-capacity-into-bounds

Cause 6 of 8: scaling-activity history reports fulfilled changes that never reached the target resource.
AWS: recorded behavior - a Successful activity with fulfilment text corresponds to a capacity change actually applied to the resource, and activities are listed newest first.
Emulator: DescribeScalingActivities claims a fulfilled change to 6 read capacity units that DynamoDB does not show.
Why: the activity log lies - a client that trusts it believes capacity moved when it did not.
Fix: scaling-activity recording must follow the actual resource mutation (activity state + the enforcement path).
Traces: application-autoscaling.scaling-activities-newest-first-with-fulfilment-text

Cause 7 of 8: RegisterScalableTarget accepts an ECS service that does not exist.
AWS: recorded behavior - registering ecs:service:DesiredCount for a missing service is refused with HTTP 400 ValidationException (Smithy: RegisterScalableTarget errors include ValidationException).
Emulator: HTTP 200 with a ScalableTargetARN.
Why: a phantom target is registered against nothing and its policies can never act.
Fix: RegisterScalableTarget handler, ECS resource resolution.
Traces: application-autoscaling.ecs-register-missing-service-rejected

Cause 8 of 8: a valid DynamoDB target-tracking TargetValue is rejected because the lower bound is 20 instead of 10.
AWS: TargetValue is an unconstrained double in the model, so the range is service-enforced; recorded behavior is 10.0 to 90.0 inclusive for DynamoDBReadCapacityUtilization / DynamoDBWriteCapacityUtilization.
Emulator: PutScalingPolicy with TargetValue 10 raises ValidationException "Value '10' at 'targetTrackingScalingPolicyConfiguration.targetValue' failed to satisfy constraint: Member must be between 20 and 90 for metric DynamoDBReadCapacityUtilization", aborting the check before it can observe the already-at-max-capacity activity.
Why: policies AWS accepts cannot be created at all. (The same wrong range shows up under the application-auto-scaling check id and is filed there; this check id belongs to this service's bucket.)
Fix: PutScalingPolicy validation, per-predefined-metric target-value range table.
Traces: application-autoscaling.not-scaled-activity-records-already-at-max-capacity

## Evidence
Replay report details:
- disable-scale-in-withholds-the-scale-in-alarms / dynamic-scaling-out-suspended-withholds-then-resume-delivers / target-tracking-alarm-thresholds-are-recomputed-not-recorded: "alarm 'TargetTracking-table/<tbl>-AlarmHigh-f1a85113e3d8ac282c637e03660b2964' does not match the TargetTracking-<resourceId>-<kind>-<uuid> grammar ...; alarm discovery is broken".
- target-tracking-policy-provisions-cloudwatch-alarms: assertion action.startswith(f'{policy_arn}:createdBy/') failed - "AlarmHigh points at 'arn:aws:autoscaling:...:scalingPolicy:<id>:resource/dynamodb/table/<tbl>:policyName/<pol>'; the action must be the scaling policy ... with a createdBy suffix, or a breach reaches nothing".
- ecs-service-target-lifecycle-leaves-the-desired-count-after-deregister: KeyError: 'SuspendedState'.
- ecs-cpu-target-tracking-policy-round-trips-and-mints-two-alarms: "the alarms are {'High': ('AWS/ECS','CPUUtilization','Average','GreaterThanThreshold',60.0,2,60), 'Low': ('AWS/ECS','CPUUtilization','Average','LessThanThreshold',48.0,15,60)}".
- register-scalable-target-clamps-capacity-into-bounds: 'table read capacity is 1 after the floor moved to 5; a capacity below MinCapacity must be corrected up to it'.
- scaling-activities-newest-first-with-fulfilment-text: 'the history claims a fulfilled change to 6 read capacity units that DynamoDB does not show'.
- ecs-register-missing-service-rejected: 'real AWS refuses this call, but it SUCCEEDED and returned {ScalableTargetARN: ..., HTTPStatusCode: 200}'.
- not-scaled-activity-records-already-at-max-capacity: botocore.errorfactory.ValidationException on PutScalingPolicy for TargetValue 10.
Handler surface: services/service-com-amazonaws-applicationautoscaling-anyscalefrontendservice/src/ops/ (put_scaling_policy.rs, register_scalable_target.rs, describe_scalable_targets.rs, describe_scaling_activities.rs) and this service's state module.

## Steps to Reproduce
Boot the emulator and drive these with botocore:
1. put_scaling_policy TargetTrackingScaling on dynamodb:table:ReadCapacityUnits, then cloudwatch.describe_alarms - inspect the alarm names (trailing segment must be a dashed UUID) and AlarmActions (must be '<policyARN>:createdBy/<uuid>').
2. register_scalable_target then describe_scalable_targets - SuspendedState must be present with three false flags.
3. put_scaling_policy TargetTrackingScaling ECSServiceAverageCPUUtilization on ecs:service:DesiredCount and compare the two alarms' thresholds/evaluation periods.
4. create a PROVISIONED table at 1 RCU, register with MinCapacity 5, then describe_table - capacity must rise to 5; describe_scaling_activities must only report changes the table shows.
5. register_scalable_target for a nonexistent ECS service -> expect ValidationException, observe 200.
6. put_scaling_policy with TargetValue 10.0 on a DynamoDB utilization metric -> expect success, observe ValidationException.
