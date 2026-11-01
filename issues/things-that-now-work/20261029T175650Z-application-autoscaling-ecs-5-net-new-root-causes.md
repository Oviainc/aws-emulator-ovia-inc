Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: application-autoscaling__ecs.changing-a-policy-type-under-the-same-name-rejected application-autoscaling__ecs.overlapping-step-adjustment-intervals-rejected application-autoscaling__ecs.step-scaling-policy-round-trips-and-lists-the-alarm-that-invokes-it application-autoscaling__ecs.deregister-strips-the-policy-from-a-customer-alarm-but-keeps-the-alarm application-autoscaling__ecs.two-policies-triggered-together-scale-by-the-largest-not-the-sum
Issue-Key: application-autoscaling__ecs.changing-a-policy-type-under-the-same-name-rejected|run#8|AssertionError|assert msg == "You cannot change a StepScaling policy to a non StepScaling policy, or vice versa.", (
Issue-Key: application-autoscaling__ecs.overlapping-step-adjustment-intervals-rejected|run#8|AssertionError|assert msg == "Step adjustment intervals cannot overlap", f"rejection message was {msg!r}"
Issue-Key: application-autoscaling__ecs.step-scaling-policy-round-trips-and-lists-the-alarm-that-invokes-it|_poll_until#17|Timeout|time.sleep(interval_s)
Issue-Key: application-autoscaling__ecs.deregister-strips-the-policy-from-a-customer-alarm-but-keeps-the-alarm|_poll_until#14|Timeout|time.sleep(interval_s)
Issue-Key: application-autoscaling__ecs.two-policies-triggered-together-scale-by-the-largest-not-the-sum|_arbitrate#22|AssertionError|assert not attributable_to_loser, (
Issue-Commit: e8891ffb15bbac5be85198cac9e3766d8ad3529e

# application-autoscaling__ecs: 5 net-new root causes — two refusal messages use emulator wording, the policy/alarm linkage is missing in both directions, and two simultaneously-triggered policies both execute

> _Note: this issue was identified against the code base at commit `e8891ffb15bbac5be85198cac9e3766d8ad3529e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutScalingPolicy

## Description
Five independent Application Auto Scaling defects, all exercised against the `ecs` service namespace. Each cause below must be closed before this issue is submitted.

Cause 1 of 5: PutScalingPolicy's policy-type-change refusal uses emulator wording.
AWS: PutScalingPolicy refuses re-putting an existing policy name under a different PolicyType with a ValidationException whose message is the fixed sentence "You cannot change a StepScaling policy to a non StepScaling policy, or vice versa." — no variable parts at all. (Smithy: PutScalingPolicy errors include ValidationException, ObjectNotFoundException, ConcurrentUpdateException, LimitExceededException; the model does not spell the text, so the sentence is recorded behavior.)
Emulator: the same call is refused with "You can't change the policy type of an existing scaling policy: the policy <PolicyName> already exists with the policy type StepScaling. Delete the policy and create a new one with the policy type you want" — invented prose that additionally interpolates the request's policy name and the stored policy type.
Why: a client (or a CDK/Terraform error matcher) that string-matches AWS's fixed sentence does not recognise the refusal, and a client that logs it reports an error AWS never emits.
Fix: the refusal text in the PutScalingPolicy handler's validation of a policy-type change.
Traces: application-autoscaling__ecs.changing-a-policy-type-under-the-same-name-rejected.

Cause 2 of 5: PutScalingPolicy's overlapping-step-interval refusal uses emulator wording.
AWS: a StepScalingPolicyConfiguration whose StepAdjustments have overlapping metric-interval ranges is refused with the fixed sentence "Step adjustment intervals cannot overlap" — again no variable parts.
Emulator: "The ranges of your step adjustments can't overlap, and [0, 10) overlaps [5, inf)" — different wording, plus a rendering of the two offending intervals that AWS never includes.
Why: same as cause 1 — the message is part of the error contract and diverging text breaks error matching.
Fix: the refusal text in the PutScalingPolicy handler's step-adjustment overlap validation.
Traces: application-autoscaling__ecs.overlapping-step-adjustment-intervals-rejected.

Cause 3 of 5: DescribeScalingPolicies never reports the CloudWatch alarm that invokes a policy.
AWS: a step-scaling policy mints no alarms of its own (PutScalingPolicy returns Alarms: []), but once a customer creates a CloudWatch alarm whose AlarmActions name that policy's ARN, DescribeScalingPolicies derives the policy's `Alarms` from CloudWatch state and lists exactly that alarm, with its AlarmName and its ARN in the caller's account and region. (Recorded behavior; the model only types Alarms as a list of Alarm{AlarmName, AlarmARN}.)
Emulator: after PutMetricAlarm names the policy ARN in AlarmActions, DescribeScalingPolicies keeps returning `Alarms: []` for the whole window the runner allowed, so the check polling for the association never observes it. The emulator stores Alarms only from what was sent to Application Auto Scaling and never joins against CloudWatch.
Why: console-equivalent and drift tooling reads a policy's Alarms to show what triggers it; here every policy looks permanently untriggered and a client cannot confirm its own alarm is wired to the policy.
Fix: DescribeScalingPolicies' projection — derive `Alarms` by scanning CloudWatch alarms whose AlarmActions contain the policy ARN, rather than from the policy record.
Traces: application-autoscaling__ecs.step-scaling-policy-round-trips-and-lists-the-alarm-that-invokes-it.

Cause 4 of 5: DeregisterScalableTarget leaves a dangling policy ARN in a customer alarm's AlarmActions.
AWS: deregistering a scalable target deletes the policies attached to it AND removes those policies' ARNs from the AlarmActions of customer-owned CloudWatch alarms that referenced them, while leaving the alarms themselves (namespace, metric, threshold, comparison operator) in place.
Emulator: DescribeScalingPolicies does show the policy gone, but the customer alarm's AlarmActions still contains the deleted policy's ARN for the whole window the runner allowed. The alarm is correctly not deleted, but the cross-service cleanup never happens.
Why: the alarm is left pointing at a policy that no longer exists, so a client re-reading its own alarm sees an action that can never fire and cannot tell a live wiring from a dead one.
Fix: DeregisterScalableTarget's cascade — when it deletes a target's policies it must also strip those ARNs from CloudWatch alarm AlarmActions.
Traces: application-autoscaling__ecs.deregister-strips-the-policy-from-a-customer-alarm-but-keeps-the-alarm.

Cause 5 of 5: two policies triggered together both execute instead of the largest-resulting-capacity one suppressing the other.
AWS: when two scaling policies on the same scalable target are triggered at the same time, Application Auto Scaling applies the one that yields the largest resulting capacity and fully suppresses the other — DescribeScalingActivities records an activity for the winning policy only, not one per policy and not the sum.
Emulator: DescribeScalingActivities records an additional Successful activity attributable to the LOSING policy, with Cause "monitor alarm <alarm> in state ALARM triggered policy <losing policy>" and Description "Setting desired count to <n>.", alongside the winning one.
Why: a client reading scaling history sees two scaling decisions for one event and a desired count that reflects whichever activity landed last, so capacity and the audit trail both diverge from AWS.
Fix: the scaling-execution path's arbitration between concurrently triggered policies for one scalable target, and the activity recorder that writes one activity per executed policy.
Traces: application-autoscaling__ecs.two-policies-triggered-together-scale-by-the-largest-not-the-sum.

## Evidence
Cause 1 — expected: ValidationException message "You cannot change a StepScaling policy to a non StepScaling policy, or vice versa."; actual: "You can't change the policy type of an existing scaling policy: the policy sim2real-behav-aas-ty-f82e3bd3c158 already exists with the policy type StepScaling. Delete the policy and create a new one with the policy type you want" (the policy name shown is this run's generated name; only the fixed sentence is the requirement).
Cause 2 — expected message "Step adjustment intervals cannot overlap"; actual "The ranges of your step adjustments can't overlap, and [0, 10) overlaps [5, inf)".
Cause 3 — expected: DescribeScalingPolicies[0].Alarms == [{AlarmName: <the customer alarm>, AlarmARN: arn:aws:cloudwatch:<region>:<account>:alarm:<that alarm>}] after PutMetricAlarm names the policy ARN in AlarmActions; actual: Alarms stayed [] for the whole polling window (replay verdict: mismatch, reported as "Check timed out after 60.0s" polling for "the customer alarm appearing under the policy").
Cause 4 — expected: after DeregisterScalableTarget, DescribeAlarms([alarm])[0].AlarmActions == [] with Namespace/MetricName/Threshold/ComparisonOperator unchanged; actual: AlarmActions still == [<deleted policy ARN>] for the whole polling window (replay verdict: mismatch, reported as "Check timed out after 60.0s" polling for "the policy ARN leaving the customer alarm").
Cause 5 — expected: zero DescribeScalingActivities entries attributable to the losing policy; actual: 1 entry, e.g. {'ActivityId': '...', 'ServiceNamespace': 'ecs', 'ScalableDimension': 'ecs:service:DesiredCount', 'Description': 'Setting desired count to 7.', 'Cause': 'monitor alarm <losing alarm> in state ALARM triggered policy <losing policy>', 'StatusCode': 'Successful'} (ids, names and timestamps are per-run; the surplus activity is the defect).

## Steps to Reproduce
Cause 1: RegisterScalableTarget for ServiceNamespace=ecs, ScalableDimension=ecs:service:DesiredCount on an ECS service; PutScalingPolicy <name> with PolicyType=StepScaling; PutScalingPolicy the SAME <name> with PolicyType=TargetTrackingScaling; read the refusal message.
Cause 2: on the same scalable target, PutScalingPolicy with StepScalingPolicyConfiguration whose StepAdjustments ranges overlap (e.g. [0,10) and [5,inf)); read the refusal message.
Cause 3: PutScalingPolicy a StepScaling policy and assert its Alarms == []; then CloudWatch PutMetricAlarm with AlarmActions=[<policy ARN>]; then poll DescribeScalingPolicies(PolicyNames=[<policy>]) for Alarms to become non-empty.
Cause 4: PutScalingPolicy a step policy, PutMetricAlarm with AlarmActions=[<policy ARN>], DeregisterScalableTarget, then poll DescribeAlarms(AlarmNames=[<alarm>]) for AlarmActions to stop containing the policy ARN; also assert the alarm itself still exists with its original metric/threshold.
Cause 5: register one scalable target, put two step-scaling policies with different resulting capacities, drive both alarms into ALARM in the same window, then DescribeScalingActivities(IncludeNotScaledActivities=True) and count activities whose Cause names each policy.
Replay the five check ids in trace_ids to confirm.
