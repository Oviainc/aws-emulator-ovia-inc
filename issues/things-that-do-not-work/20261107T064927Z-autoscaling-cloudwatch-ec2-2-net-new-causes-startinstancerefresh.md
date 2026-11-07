Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__cloudwatch__ec2.instance-refresh-refuses-to-start-while-its-cloudwatch-alarm-is-in-alarm autoscaling__cloudwatch__ec2.target-tracking-disable-scale-in-omits-the-scale-in-alarm
Issue-Key: autoscaling__cloudwatch__ec2.instance-refresh-refuses-to-start-while-its-cloudwatch-alarm-is-in-alarm|_client_error#20|AssertionError|raise AssertionError(f"{what} was ACCEPTED, but it must be refused")
Issue-Key: autoscaling__cloudwatch__ec2|PutScalingPolicy|NotImplemented
Issue-Commit: 67bb1f0baca61c1b5b95e0062b2898063e06b421

# autoscaling__cloudwatch__ec2: 2 net-new causes — StartInstanceRefresh ignores an in-ALARM AlarmSpecification, and PutScalingPolicy answers NotImplemented so target-tracking alarms are never minted

> _Note: this issue was identified against the code base at commit `67bb1f0baca61c1b5b95e0062b2898063e06b421`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartInstanceRefresh

## Description
Two net-new root causes in the Auto Scaling / CloudWatch seam.

Cause 1 of 2: StartInstanceRefresh accepts a request whose Preferences.AlarmSpecification names a CloudWatch alarm that is currently in ALARM.
  AWS: when Preferences.AlarmSpecification.Alarms names an alarm and that alarm's StateValue is ALARM at the moment of the call, Auto Scaling refuses to start the refresh with an awsQuery `ValidationError` (HTTP 400) and the sentence `A CloudWatch monitor is in alarm. The instance refresh cannot start.` — fixed text, no variable parts. No instance refresh is created, so DescribeInstanceRefreshes is unchanged and nothing is left in flight; with the same alarm back in OK the identical call succeeds and mints a new InstanceRefreshId. Smithy (autoscaling): StartInstanceRefresh input Preferences -> RefreshPreferences (AlarmSpecification), modelled errors InstanceRefreshInProgressFault/LimitExceededFault/ResourceContentionFault; `ValidationError` is the service-common query-protocol validation error and the quoted sentence is recorded behavior.
  Emulator: the call is ACCEPTED (2xx with an InstanceRefreshId) — `AssertionError: StartInstanceRefresh with an AlarmSpecification naming an alarm that is in ALARM was ACCEPTED, but it must be refused`. The alarm state is never consulted, so the gate does not exist and a refresh is created.
  Why: a caller that relies on the alarm gate to stop a rolling replacement during an incident gets the replacement started anyway, and sees no ValidationError to branch on; the recorded safety contract of AlarmSpecification is absent.
  Fix: the StartInstanceRefresh handler's validation — it must read the named alarms' current state from the cloudwatch store before creating the refresh record, and refuse with ValidationError/that message, creating nothing.
  Traces: autoscaling__cloudwatch__ec2.instance-refresh-refuses-to-start-while-its-cloudwatch-alarm-is-in-alarm

Cause 2 of 2: PutScalingPolicy is an unimplemented scaffold stub, so a target-tracking policy mints no CloudWatch alarms at all.
  AWS: PutScalingPolicy with PolicyType=TargetTrackingScaling returns 200 with PolicyARN and an `Alarms` list naming the alarms Auto Scaling created for the policy — a scale-out alarm (GreaterThanThreshold / GreaterThanOrEqualToThreshold) and, unless TargetTrackingConfiguration.DisableScaleIn is true, a scale-in alarm (LessThanThreshold / LessThanOrEqualToThreshold). With DisableScaleIn=true only the scale-out alarm exists. Smithy (autoscaling): PutScalingPolicy output PolicyARNType{PolicyARN, Alarms}, input TargetTrackingConfiguration (DisableScaleIn); modelled errors LimitExceededFault/ResourceContentionFault/ServiceLinkedRoleFailure — NotImplemented is not among them.
  Emulator: `botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the PutScalingPolicy operation: operation PutScalingPolicy is scaffolded but not implemented in this emulator`.
  Why: no target-tracking policy can be created, so no policy-owned alarm is ever minted and the DisableScaleIn alarm contract is unobservable; any client that scales on tracked metrics fails at configuration time with an error code AWS never returns.
  Fix: implement the PutScalingPolicy handler (services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/src/ops/put_scaling_policy.rs) including minting the policy's CloudWatch alarms and returning them in `Alarms`. The open issue 20261024T015148Z-autoscaling-auto-scaling-putscalingpolicy-is-an-unimplemented-scaffold-stub.md describes the same stub but does not carry this check's replay key.
  Traces: autoscaling__cloudwatch__ec2.target-tracking-disable-scale-in-omits-the-scale-in-alarm

## Evidence
Cause 1 — Expected: `ValidationError` (HTTP 400) with message `A CloudWatch monitor is in alarm. The instance refresh cannot start.`; DescribeInstanceRefreshes unchanged (no new refresh id, nothing non-terminal). Actual: the call returned success with a new InstanceRefreshId; harness reports `AssertionError: StartInstanceRefresh with an AlarmSpecification naming an alarm that is in ALARM was ACCEPTED, but it must be refused`. (The alarm was forced to ALARM via SetAlarmState immediately before the call; the same call with the alarm in OK is expected to succeed, and that positive control passed.)

Cause 2 — Expected: HTTP 200 with PolicyARN and Alarms listing exactly one scale-out alarm when DisableScaleIn=true, and both a scale-out and a scale-in alarm for the twin policy without the flag. Actual: `An error occurred (NotImplemented) when calling the PutScalingPolicy operation: operation PutScalingPolicy is scaffolded but not implemented in this emulator`.

## Steps to Reproduce
Cause 1:
1. Create an Auto Scaling group backed by a launch template and wait for it to be healthy.
2. PutMetricAlarm to create an alarm, then SetAlarmState(StateValue=ALARM) on it.
3. Call StartInstanceRefresh(AutoScalingGroupName=<group>, Strategy=Rolling, Preferences={AlarmSpecification:{Alarms:[<alarm>]}, MinHealthyPercentage:100, MaxHealthyPercentage:200}).
4. Observe a 2xx with an InstanceRefreshId instead of ValidationError, and the new refresh visible in DescribeInstanceRefreshes.

Cause 2:
1. Create an Auto Scaling group.
2. Call PutScalingPolicy(PolicyType="TargetTrackingScaling", TargetTrackingConfiguration={PredefinedMetricSpecification:..., TargetValue:43.5, DisableScaleIn:true}).
3. Observe HTTP error NotImplemented.
