Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: application-autoscaling.target-tracking-policy-provisions-cloudwatch-alarms application-autoscaling.target-tracking-policy-scales-out-table-capacity application-autoscaling.target-tracking-alarm-thresholds-are-recomputed-not-recorded application-autoscaling.disable-scale-in-withholds-the-scale-in-alarms application-autoscaling.dynamic-scaling-out-suspended-withholds-then-resume-delivers application-autoscaling.register-on-demand-table-rejected application-autoscaling.register-scalable-target-rejects-nonexistent-table application-autoscaling.register-table-resource-id-with-an-index-dimension-rejected application-autoscaling.reregistering-after-deregister-mints-a-new-target-without-the-old-tags-or-policy application-autoscaling.put-scaling-policy-rejects-unregistered-target application-autoscaling.put-scaling-policy-unregistered-dimension-rejected application-autoscaling.register-new-target-without-capacity-bounds-rejected application-autoscaling.ecs-alb-request-count-policy-without-resource-label-rejected application-autoscaling.ecs-at-scheduled-action-sets-the-desired-count-and-stays-listed application-autoscaling.ecs-cpu-and-memory-policies-coexist-and-deleting-one-keeps-the-other application-autoscaling.ecs-cpu-target-tracking-policy-round-trips-and-mints-two-alarms application-autoscaling.ecs-register-missing-service-rejected application-autoscaling.ecs-service-target-lifecycle-leaves-the-desired-count-after-deregister application-autoscaling.ecs-step-adjustment-without-any-bound-rejected application-autoscaling.ecs-step-scaling-policy-without-step-adjustments-rejected application-autoscaling.target-tracking-with-both-metric-specifications-rejected application-autoscaling.register-missing-global-secondary-index-rejected application-autoscaling.put-scheduled-action-rejects-invalid-cron-expression application-autoscaling.scheduled-action-name-with-a-slash-rejected
Issue-Shape-Id: com.amazonaws.applicationautoscaling#AnyScaleFrontendService
Issue-Key: application-autoscaling.target-tracking-policy-provisions-cloudwatch-alarms|run#18|AssertionError|assert sorted(by_kind) == sorted(_EXPECTED), (
Issue-Key: application-autoscaling.target-tracking-policy-scales-out-table-capacity|run#20|AssertionError|assert len(alarm_high) == 1, (
Issue-Key: application-autoscaling.target-tracking-alarm-thresholds-are-recomputed-not-recorded|_poll_until#18|AssertionError|raise AssertionError(f'{description} not observed within {timeout_s}s; last saw {last!r}')
Issue-Key: application-autoscaling.disable-scale-in-withholds-the-scale-in-alarms|_poll_until#15|AssertionError|raise AssertionError(f'{description} not observed within {timeout_s}s; last saw {last!r}')
Issue-Key: application-autoscaling.dynamic-scaling-out-suspended-withholds-then-resume-delivers|_scale_out_alarm#22|AssertionError|assert len(candidates) == 1, (
Issue-Key: application-autoscaling.register-on-demand-table-rejected|_client_error#8|AssertionError|raise AssertionError(f'real AWS refuses this call, but it SUCCEEDED and returned {result!r}')
Issue-Key: application-autoscaling.register-scalable-target-rejects-nonexistent-table|_client_error#9|AssertionError|raise AssertionError(
Issue-Key: application-autoscaling.register-table-resource-id-with-an-index-dimension-rejected|_client_error#8|AssertionError|raise AssertionError(f'real AWS refuses this call, but it SUCCEEDED and returned {result!r}')
Issue-Key: application-autoscaling.reregistering-after-deregister-mints-a-new-target-without-the-old-tags-or-policy|run#13|AssertionError|assert new_arn != old_arn, 're-registering after deregister returned the old target ARN'
Issue-Key: application-autoscaling.put-scaling-policy-rejects-unregistered-target|run#13|AssertionError|assert error.get('Message') == (
Issue-Key: application-autoscaling.put-scaling-policy-unregistered-dimension-rejected|run#17|AssertionError|assert error.get("Message") == (
Issue-Key: application-autoscaling.register-new-target-without-capacity-bounds-rejected|run#8|AssertionError|assert (neither.get('Code'), neither.get('Message')) == (
Issue-Key: application-autoscaling|CreateService|NotImplemented
Issue-Key: application-autoscaling|CreateTable|NotImplemented
Issue-Key: application-autoscaling.put-scheduled-action-rejects-invalid-cron-expression|run#12|AssertionError|assert error.get('Code') == 'ValidationException', (
Issue-Key: application-autoscaling.scheduled-action-name-with-a-slash-rejected|run#12|AssertionError|assert error.get('Code') == 'ValidationException', (
Issue-Commit: 5598afbcdd0af564c379fb305d1563cf7235aea7

# application-autoscaling: policies mint no CloudWatch alarms, RegisterScalableTarget accepts resources AWS refuses, deregister leaves target state behind, rejection-message rules diverge, and the ECS/DynamoDB-GSI/scheduled-action dependencies are stubs (7 causes)

> _Note: this issue was identified against the code base at commit `5598afbcdd0af564c379fb305d1563cf7235aea7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutScalingPolicy

## Description
Seven distinct root causes grounded in failing checks of the `application-autoscaling` suite. Smithy model consulted: services/service-com-amazonaws-applicationautoscaling-anyscalefrontendservice/smithy-model/com-amazonaws-applicationautoscaling-anyscalefrontendservice.json.

Cause 1 of 7: target-tracking policies provision no CloudWatch alarms, and therefore no alarm-driven scaling, no recomputed thresholds and no suspended-state withholding.
AWS: Smithy: PutScalingPolicyResponse.Alarms (list) is part of the response. Real AWS mints the policy's alarm set (recorded names AlarmHigh/AlarmLow/ProvisionedCapacityHigh/ProvisionedCapacityLow), re-COMPUTES the utilisation thresholds from TargetValue, the alarm Period and current provisioned capacity, withholds only the scale-IN alarms when DisableScaleIn is set, and withholds then delivers scale-out while DynamicScalingOutSuspended toggles.
Emulator: PutScalingPolicy returns `Alarms: []` and CloudWatch has no alarms for the policy under any AlarmNamePrefix — checks report 'the policy minted alarms for [], expected [AlarmHigh, AlarmLow, ProvisionedCapacityHigh, ProvisionedCapacityLow]', 'PutScalingPolicy minted 0 AlarmHigh alarms, expected exactly 1', and the threshold/DisableScaleIn/suspended-state probes see `{}`.
Why: the entire target-tracking mechanism is inert; clients cannot observe thresholds, scale-in suppression, suspension semantics, or any capacity movement.
Fix: PutScalingPolicy's side-effect/state path — materialise and maintain the alarm set (threshold computation included) in the CloudWatch emulator's state, honouring SuspendedState and DisableScaleIn.
Traces: application-autoscaling.target-tracking-policy-provisions-cloudwatch-alarms, application-autoscaling.target-tracking-policy-scales-out-table-capacity, application-autoscaling.target-tracking-alarm-thresholds-are-recomputed-not-recorded, application-autoscaling.disable-scale-in-withholds-the-scale-in-alarms, application-autoscaling.dynamic-scaling-out-suspended-withholds-then-resume-delivers.

Cause 2 of 7: RegisterScalableTarget accepts resource/dimension combinations AWS refuses.
AWS: Smithy: RegisterScalableTarget declares ValidationException (client, httpError 400). AWS refuses (400 ValidationException) a target on a PAY_PER_REQUEST (on-demand) table, on a table that does not exist, and a `table/<name>` ResourceId paired with an INDEX dimension (dynamodb:index:*), which do not correspond to one another.
Emulator: all three return HTTP 200 with a ScalableTargetARN.
Why: clients get success for registrations AWS rejects — on-demand tables appear to be auto-scalable and a mismatched ResourceId/dimension pair appears valid, so the same call fails only in production.
Fix: RegisterScalableTarget handler validation — resolve the resource, check its billing mode, and cross-check ResourceId shape against ScalableDimension.
Traces: application-autoscaling.register-on-demand-table-rejected, application-autoscaling.register-scalable-target-rejects-nonexistent-table, application-autoscaling.register-table-resource-id-with-an-index-dimension-rejected.

Cause 3 of 7: DeregisterScalableTarget does not retire the target identity, so re-registration returns the OLD target.
AWS: after DeregisterScalableTarget, re-registering the same resource/dimension mints a NEW scalable target — a new ScalableTargetARN, with none of the old target's tags or policies (recorded behavior).
Emulator: the re-registration returns the previous ScalableTargetARN ('re-registering after deregister returned the old target ARN'), i.e. the deregistered target's state was retained and reused.
Why: clients that key on the target ARN (tagging, IAM resource conditions, cleanup verification) see a resurrected resource with stale associations that AWS would have dropped.
Fix: the scalable-target state layer — deregistration must delete the target record (and its tags/policies) rather than mark it inactive, and registration must mint a fresh identity.
Traces: application-autoscaling.reregistering-after-deregister-mints-a-new-target-without-the-old-tags-or-policy.

Cause 4 of 7: rejection messages built from the request use emulator wording.
AWS: ObjectNotFoundException from PutScalingPolicy on an unregistered target/dimension carries 'No scalable target registered for service namespace: <ns>, resource ID: <resourceId>, scalable dimension: <dim>'; a first registration with neither bound is refused with ValidationException naming the missing member ('Minimum capacity must be specified').
Emulator: codes match but wording does not: 'No scalable target FOUND for service namespace: ...' and 'MinCapacity and MaxCapacity are required when registering a new scalable target'.
Why: clients that surface or match on the message diverge even though the error class is right.
Fix: the message templates in PutScalingPolicy's target lookup and RegisterScalableTarget's bounds validation. Note: the same suite also shows a different CONSTANT message for min-above-max ('MinCapacity (9) must be less than or equal to MaxCapacity (2)' vs the recorded 'Maximum capacity cannot be less than minimum capacity'); that string has no derivable rule and is reported as an example only, not as a requirement.
Traces: application-autoscaling.put-scaling-policy-rejects-unregistered-target, application-autoscaling.put-scaling-policy-unregistered-dimension-rejected, application-autoscaling.register-new-target-without-capacity-bounds-rejected.

Cause 5 of 7: ECS CreateService is unimplemented, so every ECS-backed check dies at setup.
AWS: CreateService returns the service with its desiredCount; `service/<cluster>/<service>` + ecs:service:DesiredCount is the ECS scalable resource, and the ECS negative paths (ALBRequestCountPerTarget without ResourceLabel, step policy without StepAdjustments, a step adjustment with no bound, both metric specifications at once) are 400 ValidationException.
Emulator: error code `NotImplemented`, 'operation CreateService is scaffolded but not implemented in this emulator'.
Why: the whole ECS surface of Application Auto Scaling — policies, scheduled actions, lifecycle and its rejections — is unreachable.
Fix: implement the ECS CreateService operation handler (services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/create_service.rs) and the service state it owns; then the ECS-specific Application Auto Scaling validations must be re-replayed.
Traces: application-autoscaling.ecs-alb-request-count-policy-without-resource-label-rejected, application-autoscaling.ecs-at-scheduled-action-sets-the-desired-count-and-stays-listed, application-autoscaling.ecs-cpu-and-memory-policies-coexist-and-deleting-one-keeps-the-other, application-autoscaling.ecs-cpu-target-tracking-policy-round-trips-and-mints-two-alarms, application-autoscaling.ecs-register-missing-service-rejected, application-autoscaling.ecs-service-target-lifecycle-leaves-the-desired-count-after-deregister, application-autoscaling.ecs-step-adjustment-without-any-bound-rejected, application-autoscaling.ecs-step-scaling-policy-without-step-adjustments-rejected, application-autoscaling.target-tracking-with-both-metric-specifications-rejected.

Cause 6 of 7: DynamoDB CreateTable with GlobalSecondaryIndexes is unimplemented, so the index-dimension negative path is unreachable.
AWS: CreateTable accepts GlobalSecondaryIndexes; registering `table/<t>/index/<missing>` is then refused with 400 ValidationException.
Emulator: `NotImplemented` — 'operation CreateTable with GlobalSecondaryIndexes is scaffolded but not implemented in this emulator'.
Why: a client cannot create the resource the check needs, and the missing-index rejection cannot be observed.
Fix: the DynamoDB CreateTable handler's GlobalSecondaryIndexes path.
Traces: application-autoscaling.register-missing-global-secondary-index-rejected.

Cause 7 of 7: PutScheduledAction negative paths answer NotImplemented instead of the modeled rejection.
AWS: Smithy: PutScheduledAction declares ValidationException (client, 400); ScheduledActionName carries pattern ^(?!((^[ ]+.*)|(.*([\u0000-\u001f]|[\u007f-\u009f]|[:/|])+.*)|(.*[ ]+$))).+$ , which forbids '/'. An unparseable cron Schedule and a name containing '/' are both 400 ValidationException.
Emulator: error code `NotImplemented`, 'operation PutScheduledAction is scaffolded but not implemented in this emulator'.
Why: clients cannot distinguish an invalid schedule or name from an unsupported operation; the modeled pattern constraint is never enforced.
Fix: implement PutScheduledAction (services/service-com-amazonaws-applicationautoscaling-anyscalefrontendservice/src/ops/put_scheduled_action.rs) with the cron/name validation. The bare stub is already filed as its own 'unimplemented scaffold stub' work item; this cause records the validation contract and these two checks must be re-replayed.
Traces: application-autoscaling.put-scheduled-action-rejects-invalid-cron-expression, application-autoscaling.scheduled-action-name-with-a-slash-rejected.

## Evidence
Verbatim replay evidence (suite simparity-behavioral-aws-v1):
- 'AssertionError: the policy minted alarms for [], expected [AlarmHigh, AlarmLow, ProvisionedCapacityHigh, ProvisionedCapacityLow]'; 'PutScalingPolicy minted 0 AlarmHigh alarms, expected exactly 1; alarms were []'; threshold check: 'the utilisation scale-out threshold settling on 40.0% * Period * 1 provisioned units (== 24.0 at the measured Period of 60 s) not observed within 45.0s; last saw {}'.
- RegisterScalableTarget on an on-demand/nonexistent table, or table ResourceId with an index dimension: HTTP 200 `{'ScalableTargetARN': 'arn:aws:application-autoscaling:us-west-2:000000000000:scalable-target/...'}` — expected 400 ValidationException.
- 're-registering after deregister returned the old target ARN'.
- Messages: 'No scalable target found for service namespace: dynamodb, resource ID: table/..., scalable dimension: dynamodb:table:ReadCapacityUnits' (AWS: 'No scalable target registered for ...'); refusal `{'Message': 'MinCapacity and MaxCapacity are required when registering a new scalable target', 'Code': 'ValidationException'}` (AWS: 'Minimum capacity must be specified').
- `ClientError: An error occurred (NotImplemented) when calling the CreateService operation: operation CreateService is scaffolded but not implemented in this emulator` (9 checks).
- `ClientError: An error occurred (NotImplemented) when calling the CreateTable operation: operation CreateTable with GlobalSecondaryIndexes is scaffolded but not implemented in this emulator`.
- 'an unparseable cron was rejected with NotImplemented, expected ValidationException'; 'a scheduled action name with "/" was refused with NotImplemented, expected ValidationException'.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Cause 1: create a PROVISIONED DynamoDB table, RegisterScalableTarget on dynamodb:table:ReadCapacityUnits, PutScalingPolicy (TargetTrackingScaling, DynamoDBReadCapacityUtilization, TargetValue 40) — observe `Alarms: []` and CloudWatch DescribeAlarms (AlarmNamePrefix 'TargetTracking') empty.
3. Cause 2: RegisterScalableTarget against a PAY_PER_REQUEST table, a nonexistent table, and `table/<t>` with dimension dynamodb:index:ReadCapacityUnits — each returns 200.
4. Cause 3: DeregisterScalableTarget then RegisterScalableTarget the same resource/dimension — the ScalableTargetARN is unchanged.
5. Cause 4: PutScalingPolicy on an unregistered target and RegisterScalableTarget with no bounds — compare the Message strings.
6. Causes 5-7: ECS CreateService, DynamoDB CreateTable with GlobalSecondaryIndexes, and PutScheduledAction all answer NotImplemented.
