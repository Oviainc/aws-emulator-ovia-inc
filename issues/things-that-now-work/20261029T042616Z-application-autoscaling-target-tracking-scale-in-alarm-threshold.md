Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: application-autoscaling.target-tracking-alarm-thresholds-are-recomputed-not-recorded application-autoscaling.target-tracking-policy-provisions-cloudwatch-alarms application-autoscaling.deregister-scalable-target-cascades-but-keeps-activities application-autoscaling.dynamic-scaling-out-suspended-withholds-then-resume-delivers
Issue-Shape-Id: com.amazonaws.applicationautoscaling#AnyScaleFrontendService
Issue-Key: application-autoscaling.target-tracking-alarm-thresholds-are-recomputed-not-recorded|_assert_rule#18|AssertionError|assert abs(low['Threshold'] - expected_low) <= _TOLERANCE, (
Issue-Key: application-autoscaling.target-tracking-policy-provisions-cloudwatch-alarms|run#20|AssertionError|assert alarm[field] == expected, (
Issue-Key: application-autoscaling.deregister-scalable-target-cascades-but-keeps-activities|run#45|AssertionError|assert put_error.get('Message') == expected_message, f"expected the message {expected_message!r}, got {put_error.get('Message')!r}"
Issue-Key: application-autoscaling.dynamic-scaling-out-suspended-withholds-then-resume-delivers|_poll_until#23|Timeout|time.sleep(interval_s)
Issue-Commit: 1fd13cac8c9d59f908c3acc103dd89ab8a3a1d61

# application-autoscaling: target-tracking scale-in alarm threshold is a fraction of the scale-out threshold instead of a 20-point buffer below TargetValue, the post-deregister ObjectNotFoundException says 'found' where AWS says 'registered', and DynamicScalingOutSuspended never withholds-then-delivers (3 causes)

> _Note: this issue was identified against the code base at commit `1fd13cac8c9d59f908c3acc103dd89ab8a3a1d61`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutScalingPolicy

## Description
Three independent net-new root causes, each grounded in failing behavioral checks whose ids carry the service token `application-autoscaling`. The checks the replay marked OWNED by 20261024T115439Z-application-autoscaling-application-auto-scaling-scheduled-actions-never-fire-target.md are skipped and not re-filed here.

Cause 1 of 3: the scale-in (AlarmLow) threshold of a target-tracking policy's minted CloudWatch alarms is derived as a FRACTION of the scale-out threshold, not as a fixed 20-percentage-point buffer below TargetValue.
AWS: Smithy TargetTrackingScalingPolicyConfiguration.TargetValue is an unconstrained double, so the alarm arithmetic is service-defined; recorded behavior for a DynamoDB*CapacityUtilization predefined metric is that the alarms watch the per-minute SUM of consumed capacity units, so the scale-out (AlarmHigh) threshold is `TargetValue/100 * Period * provisioned` and the scale-in (AlarmLow) threshold is `(TargetValue - 20)/100 * Period * provisioned` — a fixed 20 PERCENTAGE-POINT buffer below the target value that scales with provisioned capacity. The thresholds are recomputed (and the alarms re-minted) whenever the tracked capacity or TargetValue changes; they are not a stored copy. For TargetValue=40, Period=60, provisioned=1 that is AlarmHigh=24.0 and AlarmLow=12.0.
Emulator: AlarmHigh matches, AlarmLow is 19.200000000000003 — i.e. 0.8 x the scale-out threshold ((TargetValue*0.8)/100 * 60 * provisioned), a proportional haircut rather than a fixed point offset. Replay: `AlarmLow has Threshold=19.200000000000003, expected 12.0` and `leg 1 (TargetValue 40, capacity 1): the scale-in alarm threshold is 19.200000000000003, expected (40.0 - 20)/100 * 60 * 1 == 12.0`. Because the rule is wrong in form (proportional, not a fixed offset), the error grows with TargetValue, and the recomputation legs at other TargetValues diverge too.
Why: the AlarmLow threshold is what decides when capacity is removed. A client reading its policy's alarms sees a scale-in trigger far closer to the target than AWS would set, so the emulated policy scales in under load AWS would still consider on-target (flapping), and any client or test that recomputes the documented threshold to validate its policy disagrees with the service.
Fix: the PutScalingPolicy handler's target-tracking alarm derivation (the scale-in threshold formula and its recomputation on capacity/TargetValue change) in this service's ops module.
Traces: application-autoscaling.target-tracking-policy-provisions-cloudwatch-alarms, application-autoscaling.target-tracking-alarm-thresholds-are-recomputed-not-recorded

Cause 2 of 3: after DeregisterScalableTarget, PutScalingPolicy against the now-absent target is refused with the right status and code but the wrong request-derived message template — 'No scalable target found for ...' where AWS says 'No scalable target registered for ...'.
AWS: Smithy models ObjectNotFoundException as a client error with @httpError 400, documented as "The specified object could not be found. For any operation that depends on the existence of a scalable target, this exception is thrown if the scalable target with the specified service namespace, resource ID, and scalable dimension does not exist." Its message is built from the request, so the template is the requirement: fixed text `No scalable target registered for service namespace: `, `, resource ID: `, `, scalable dimension: ` with the three variable parts taken verbatim from the request's ServiceNamespace, ResourceId and ScalableDimension. HTTP 400 and the ObjectNotFoundException code already match.
Emulator: the same template with the verb `found` instead of `registered`: `No scalable target found for service namespace: dynamodb, resource ID: table/<table>, scalable dimension: dynamodb:table:ReadCapacityUnits` (the table name shown in the replay is a per-run generated name and is the request-derived part, not part of the template).
Why: this message is the only place the service names the namespace/resource/dimension triple it could not resolve, and clients, CLI users and conformance suites match the AWS text; a paraphrase does not match.
Fix: the shared ObjectNotFoundException message builder used by this service's PutScalingPolicy / PutScheduledAction / DeleteScheduledAction / DeregisterScalableTarget handlers.
Note: cause 6 of the open issue 20261024T115439Z-application-autoscaling-application-auto-scaling-scheduled-actions-never-fire-target.md and cause 10 of 20261024T115439Z-application-auto-scaling-has-no-enforcement-or-scheduling.md describe the same 'found'/'registered' template defect for OTHER check ids; this check id is carried by neither under its current failure key (it previously failed as a timeout, `_poll_until#24|Timeout`, and the replay no longer marks it owned), so it is filed here and should be closed by the same shared-builder fix.
Traces: application-autoscaling.deregister-scalable-target-cascades-but-keeps-activities

Cause 3 of 3: SuspendedState.DynamicScalingOutSuspended is stored and echoed correctly but has no behavior behind it — the withhold-then-deliver-on-resume path never completes.
AWS: Smithy SuspendedState.DynamicScalingOutSuspended is documented as "Whether scale out by a target tracking scaling policy or a step scaling policy is suspended. Set the value to true if you don't want Application Auto Scaling to add capacity when a scaling policy is triggered." Recorded behavior: suspending scale out does NOT delete the policy or its CloudWatch alarms (the suspended target keeps the same alarm kinds as an unsuspended control), the suspended target records NO scaling activity and DynamoDB stays at the starting capacity while its own scale-out alarm sits in ALARM, and once DynamicScalingOutSuspended is lifted the withheld scale-out is delivered without new metric movement — as a Successful activity whose Cause is `monitor alarm <alarm name> in state ALARM triggered policy <policy name>` and whose Description is `Setting read capacity units to <new capacity>.` (both templates; the alarm name, policy name and capacity come from state/the request).
Emulator: the check exhausts the replay deadline (`Error: Check timed out after 60.0s`). Its first barrier is the UNSUSPENDED control target completing a policy-driven scale-out under identical live read load, and no Successful activity and no capacity movement ever arrives for it, so neither the withheld leg nor the resume-delivery leg is ever reached. Hypothesis: the dynamic-scaling execution path (alarm evaluation -> activity -> capacity change) is not running for target-tracking policies, so the suspend/resume gate has nothing to gate.
Why: a client that suspends scale out to freeze capacity, and later resumes it, gets no scaling either way; nothing in DescribeScalingActivities distinguishes a withheld scale-out from a dead control loop, and the resumed target never receives the capacity it was owed.
Fix: the dynamic-scaling execution path of this service (alarm-breach ingestion for target-tracking policies, the activity recorder with the AWS Cause/Description templates, and the capacity-enforcement write to the backing resource), with the SuspendedState.DynamicScalingOutSuspended gate applied on dispatch and re-evaluation of still-breaching alarms on resume.
Note: cause 2 of the open issue 20261024T115439Z-application-autoscaling-application-auto-scaling-scheduled-actions-never-fire-target.md reports that target tracking never scales, grounded in a different check id; this check additionally pins the suspend/resume gate, the alarm-retention-while-suspended rule and the delivered activity's Cause/Description templates, none of which that issue describes.
Traces: application-autoscaling.dynamic-scaling-out-suspended-withholds-then-resume-delivers

## Evidence
Replay report (simparity-behavioral-ga-prod-v1), service application-autoscaling 16/38 (42%), 22 failing checks of which 18 are marked OWNED by an open issue and skipped. The 4 remaining are clustered into the 3 causes above.

Cause 1 (expected -> actual):
```
File "check_application_autoscaling_target_tracking_policy_provisions_cloudwatch_alarms.py", line 164, in run
    assert alarm[field] == expected, (
AssertionError: AlarmLow has Threshold=19.200000000000003, expected 12.0
```
```
File "check_application_autoscaling_target_tracking_alarm_thresholds_are_recomputed_not_recorded.py", line 177, in _assert_rule
    assert abs(low['Threshold'] - expected_low) <= _TOLERANCE, (
AssertionError: leg 1 (TargetValue 40, capacity 1): the scale-in alarm threshold is 19.200000000000003, expected (40.0 - 20)/100 * 60 * 1 == 12.0 -- real AWS buffers scale-in by a fixed 20 PERCENTAGE POINTS below the target value, not by halving the scale-out threshold
```
TargetValue=40, Period=60 s, provisioned=1: AWS AlarmHigh=24.0 (matches), AWS AlarmLow=12.0; emulator AlarmLow=19.2 == 0.8 x 24.0.

Cause 2 (expected -> actual):
```
File "check_application_autoscaling_deregister_scalable_target_cascades_but_keeps_activities.py", line 222, in run
    assert put_error.get('Message') == expected_message
AssertionError: expected the message 'No scalable target registered for service namespace: dynamodb, resource ID: table/<generated-table>, scalable dimension: dynamodb:table:ReadCapacityUnits', got 'No scalable target found for service namespace: dynamodb, resource ID: table/<generated-table>, scalable dimension: dynamodb:table:ReadCapacityUnits'
```
Fixed text of the template: `No scalable target registered for service namespace: ` / `, resource ID: ` / `, scalable dimension: `. Variable parts: ServiceNamespace, ResourceId, ScalableDimension, all copied from the request. HTTP 400 / ObjectNotFoundException already match; only the verb diverges.

Cause 3:
```
[mismatch] application-autoscaling.dynamic-scaling-out-suspended-withholds-then-resume-delivers
Error: Check timed out after 60.0s
```
The check's internal budgets are far larger than the replay deadline; its first poll is `_poll_until(lambda: [a for a in _activities(aas, control_id) if a['StatusCode'] == 'Successful'], bool, timeout_s=_BARRIER_S, ...)` on the UNSUSPENDED control, described as "the unsuspended control completing a scale-out under the same read load -- this arrival is the barrier for the suspended subject". It then asserts `control_acts[0]['Cause'] == f'monitor alarm {alarm} in state ALARM triggered policy {policy}'`, that the suspended subject keeps the same alarm kinds as the control, that the subject records zero activities (including the IncludeNotScaledActivities view) while its own scale-out alarm is in ALARM, and after `register_scalable_target(..., SuspendedState=all-false)` that the withheld scale-out is delivered with Description `f'Setting read capacity units to {subject_capacity}.'`.

Smithy evidence: com.amazonaws.applicationautoscaling#ObjectNotFoundException — client error, @httpError 400; com.amazonaws.applicationautoscaling#SuspendedState.DynamicScalingOutSuspended docs as quoted; TargetTrackingScalingPolicyConfiguration.TargetValue is an unconstrained double.

Handler surface: services/service-com-amazonaws-applicationautoscaling-anyscalefrontendservice/src/ops/ (put_scaling_policy.rs, register_scalable_target.rs, deregister_scalable_target.rs, describe_scaling_activities.rs) and this service's state / alarm-derivation modules.

## Steps to Reproduce
Boot the emulator and drive with botocore (single region).

Cause 1: create a PROVISIONED DynamoDB table at 1 ReadCapacityUnit; register_scalable_target(dynamodb, 'table/<t>', 'dynamodb:table:ReadCapacityUnits', Min=1, Max=N); put_scaling_policy(PolicyType='TargetTrackingScaling', TargetTrackingScalingPolicyConfiguration={'TargetValue': 40.0, 'PredefinedMetricSpecification': {'PredefinedMetricType': 'DynamoDBReadCapacityUtilization'}}); cloudwatch.describe_alarms for the minted pair. AlarmHigh.Threshold must be 40/100*60*1 == 24.0 and AlarmLow.Threshold must be (40-20)/100*60*1 == 12.0; observe AlarmLow == 19.2. Repeat with a different TargetValue and with the provisioned capacity moved, and re-read the alarms: the thresholds must be recomputed from the live capacity by the same two formulas.

Cause 2: register_scalable_target as above, then deregister_scalable_target for the same (ServiceNamespace, ResourceId, ScalableDimension), then put_scaling_policy against that same triple. Expect HTTP 400 ObjectNotFoundException with Message 'No scalable target registered for service namespace: dynamodb, resource ID: table/<t>, scalable dimension: dynamodb:table:ReadCapacityUnits'; observe the same message with 'found' instead of 'registered'. Also confirm the pre-existing DescribeScalingActivities entries survive the deregister.

Cause 3: create two identical PROVISIONED DynamoDB tables (subject and control) at the same starting read capacity; register both scalable targets, the subject with SuspendedState={'DynamicScalingInSuspended': False, 'DynamicScalingOutSuspended': True, 'ScheduledScalingSuspended': False}; put an identical TargetTrackingScaling policy (DynamoDBReadCapacityUtilization, ScaleOutCooldown=0, ScaleInCooldown=0) on each; drive identical consistent-read load against both tables. The control must record a Successful activity with Cause 'monitor alarm <control AlarmHigh> in state ALARM triggered policy <control policy>' and its table capacity must rise; the subject must keep the same alarm kinds, record no activity (also with IncludeNotScaledActivities=True) and stay at the starting capacity while its own AlarmHigh is in ALARM. Then re-register the subject with all suspension flags false: the withheld scale-out must be delivered with the same policy-driven Cause and Description 'Setting read capacity units to <new capacity>.'. Observe that the control never scales at all, so nothing downstream happens.
