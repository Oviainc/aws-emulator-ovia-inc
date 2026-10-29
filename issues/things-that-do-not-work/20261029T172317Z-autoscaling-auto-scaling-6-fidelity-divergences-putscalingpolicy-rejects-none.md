Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__cloudwatch__ec2.target-tracking-both-metric-specifications-rejected-and-no-alarm-minted autoscaling__cloudwatch__ec2.target-tracking-customized-metric-without-statistic-rejected-and-no-alarm-minted autoscaling__cloudwatch__ec2.target-tracking-zero-target-value-rejected-and-no-alarm-minted autoscaling__ec2__elbv2.asg-launches-ec2-instance-with-correct-launch-template autoscaling__ec2__elbv2.asg-scale-in-respects-termination-policy autoscaling__ec2__elbv2.elb-target-health-change-event-not-delivered-without-eventbridge-rule
Issue-Key: autoscaling__cloudwatch__ec2.target-tracking-both-metric-specifications-rejected-and-no-alarm-minted|run#10|AssertionError|assert response["code"] == "ValidationError", (
Issue-Key: autoscaling__cloudwatch__ec2.target-tracking-customized-metric-without-statistic-rejected-and-no-alarm-minted|run#10|AssertionError|assert response["code"] == "ValidationError", (
Issue-Key: autoscaling__cloudwatch__ec2.target-tracking-zero-target-value-rejected-and-no-alarm-minted|run#10|AssertionError|assert response["code"] == "ValidationError", (
Issue-Key: autoscaling__ec2__elbv2.asg-launches-ec2-instance-with-correct-launch-template|run#17|AssertionError|assert tags.get("sim2real-marker") == marker
Issue-Key: autoscaling__ec2__elbv2.asg-scale-in-respects-termination-policy|run#18|Timeout|time.sleep(15)
Issue-Key: autoscaling__ec2__elbv2.elb-target-health-change-event-not-delivered-without-eventbridge-rule|_receive_matching_message#24|Timeout|resp = sqs.receive_message(
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# Auto Scaling: 6 fidelity divergences — PutScalingPolicy rejects none of its three target-tracking validation rules, ASG-launched instances lose the launch template's instance tags, scale-in never terminates, and the target-group alarm transition never settles

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutScalingPolicy

## Description
SIX independent root causes are bundled here, all in the Auto Scaling surface the behavioral suite exercises. ALL SIX must be closed before submitting. Causes 1-3 are three distinct PutScalingPolicy input-validation rules; causes 4-6 are three distinct group-lifecycle/observability defects. Related open issue 20261024T015148Z-autoscaling-auto-scaling-putscalingpolicy-is-an-unimplemented-scaffold-stub.md names the PutScalingPolicy stub itself (Issue-Key autoscaling__cloudformation__cloudwatch__ec2|PutScalingPolicy|NotImplemented) but carries none of the replay keys below and names none of the three rules in causes 1-3, so implementing the handler is necessary but not sufficient for them.

Cause 1 of 4 is listed as Cause 1 of 6 — the bundle has SIX causes; the numbering below is authoritative.

Cause 1 of 6: PutScalingPolicy must refuse a TargetTrackingConfiguration that carries BOTH a PredefinedMetricSpecification and a CustomizedMetricSpecification.
AWS: Smithy (Auto Scaling 2011-01-01, awsQuery): PutScalingPolicy input PutScalingPolicyType -> TargetTrackingConfiguration; both TargetTrackingConfiguration.PredefinedMetricSpecification and .CustomizedMetricSpecification are optional members, and each one's own documentation states the rule the model cannot express: "A predefined metric. You must specify either a predefined metric or a customized metric." Recorded behavior (real AWS, us-west-2): supplying both returns HTTP 400 with awsQuery error code `ValidationError` and the message `You should provide either a CustomizedMetricSpecification or a PredefinedMetricSpecification` — that message is entirely fixed text, it has no request-derived parts — and leaves no policy and no CloudWatch alarm behind.
Emulator: the call is refused with error code `NotImplemented` and the message `operation PutScalingPolicy is scaffolded but not implemented in this emulator`, so the rule is never reached.
Why: a client that relies on AWS refusing an ambiguous target-tracking policy gets a wholly different error class (a scaffold error, not ValidationError), cannot distinguish "my input is wrong" from "this service is unavailable", and any retry/validation logic keyed on ValidationError never fires.
Fix: input validation in the PutScalingPolicy handler of the autoscaling service (services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/src/ops/put_scaling_policy.rs), after the handler itself exists.
Traces: autoscaling__cloudwatch__ec2.target-tracking-both-metric-specifications-rejected-and-no-alarm-minted.

Cause 2 of 6: PutScalingPolicy must refuse a CustomizedMetricSpecification that omits Statistic (when the metric is given as Namespace/MetricName/Dimensions rather than as Metrics).
AWS: Smithy: CustomizedMetricSpecification.Statistic targets the MetricStatistic enum and carries NO @required trait (the metric-math `Metrics` form legitimately omits it), so the model alone admits this input. Recorded behavior (real AWS, us-west-2): a CustomizedMetricSpecification naming Namespace, MetricName and Dimensions but no Statistic returns HTTP 400 `ValidationError` with the message `1 validation error detected: Value null at 'targetTrackingConfiguration.customizedMetricSpecification.statistic' failed to satisfy constraint: Member must not be null`. Fixed text: `N validation error(s) detected: Value null at '<member path>' failed to satisfy constraint: Member must not be null`; the variable parts are the violation count and the member path, which here is the fixed path `targetTrackingConfiguration.customizedMetricSpecification.statistic`. No policy and no alarm is created.
Emulator: error code `NotImplemented`, message `operation PutScalingPolicy is scaffolded but not implemented in this emulator`.
Why: the request is accepted-or-refused for the wrong reason, and once the stub is replaced a model-driven implementation will ACCEPT this input for free (the constraint is not in the model), silently minting a policy AWS would never create.
Fix: input validation in the PutScalingPolicy handler (the customized-metric branch) of the autoscaling service.
Traces: autoscaling__cloudwatch__ec2.target-tracking-customized-metric-without-statistic-rejected-and-no-alarm-minted.

Cause 3 of 6: PutScalingPolicy must refuse an out-of-range TargetValue (0 or negative).
AWS: Smithy: TargetTrackingConfiguration.TargetValue is @required and targets MetricScale, a bare `double` with NO @range trait, so the model admits 0 and negatives. Recorded behavior (real AWS, us-west-2): TargetValue=0 on an otherwise valid customized-metric target-tracking policy returns HTTP 400 `ValidationError` with the message `Target value must be between 8.51592E-12 and 1.174271E17`; TargetValue=-5 returns the same. The message is fixed text including both documented bounds. No policy and no alarm is created.
Emulator: error code `NotImplemented`, message `operation PutScalingPolicy is scaffolded but not implemented in this emulator`.
Why: same class of breakage — the service-side range rule is absent from the model, so without an explicit check the implementation will accept a target value AWS rejects and create a policy and alarms that cannot exist in AWS.
Fix: input validation (range check on TargetValue) in the PutScalingPolicy handler of the autoscaling service.
Traces: autoscaling__cloudwatch__ec2.target-tracking-zero-target-value-rejected-and-no-alarm-minted.

Cause 4 of 6: an instance launched by an Auto Scaling group does not carry the launch template's instance TagSpecifications.
AWS: ec2 CreateLaunchTemplate with LaunchTemplateData.TagSpecifications[{ResourceType:'instance', Tags:[...]}] applies those tags to every instance launched from the template, including instances launched by an ASG whose LaunchTemplate points at it; ec2 DescribeInstances then returns them in Reservations[].Instances[].Tags. Recorded behavior only for the exact tag set.
Emulator: for the same group the InstanceType (t3.small) and SecurityGroups assertions pass, so the template IS consulted, but the launched instance's Tags do not contain the template's instance tag key at all — the lookup of the template's tag key in DescribeInstances' Tags returns nothing (AssertionError on `tags.get(<template tag key>) == <value>`).
Why: tag-driven clients break downstream of a correct launch: cost allocation, tag-filtered DescribeInstances, tag-based IAM conditions and tag-based discovery all see untagged instances, and the ASG/launch-template contract silently drops user data that AWS preserves.
Fix: launch-template TagSpecifications propagation in the ASG instance-launch path (and/or the EC2 RunInstances tag-specification handling it drives).
Traces: autoscaling__ec2__elbv2.asg-launches-ec2-instance-with-correct-launch-template.

Cause 5 of 6: a scale-in on a group with TerminationPolicies=[OldestInstance] never terminates anything, so the check reaches no terminal result.
AWS: UpdateAutoScalingGroup lowering DesiredCapacity from 2 to 1 on a group with TerminationPolicies=['OldestInstance'] terminates the first-launched instance (EC2 state 'terminated') and leaves the newer one 'running'; Smithy: UpdateAutoScalingGroup(UpdateAutoScalingGroupType.TerminationPolicies, list of XmlStringMaxLen1600) — the policy semantics are documented service behavior, recorded behavior only for the selection.
Emulator: after DesiredCapacity is reduced, polling ec2 DescribeInstances over both member instances never shows either instance 'terminated'; the check produces no terminal result and is cut off — `Check timed out after 60.0s`. Hypothesis: the desired-capacity-decrease path performs no scale-in at all (no instance is selected or terminated), so the wait can never converge.
Why: scale-in is half of autoscaling. A client that lowers desired capacity sees the group's declared capacity change while the instances (and their cost, their target-group registrations and their licences) stay alive forever, and the termination-policy choice AWS makes is unobservable.
Fix: the scale-in path of the Auto Scaling group reconciler — termination-policy evaluation and instance termination on a DesiredCapacity decrease (UpdateAutoScalingGroup/SetDesiredCapacity handlers and the group state they drive).
Traces: autoscaling__ec2__elbv2.asg-scale-in-respects-termination-policy.

Cause 6 of 6: a forced CloudWatch alarm transition on an ALB target group's UnHealthyHostCount is never observed, so the no-rule/with-rule delivery scenario reaches no terminal result.
AWS: cloudwatch SetAlarmState(AlarmName, StateValue='ALARM', StateReason=...) immediately sets the alarm's state; the next DescribeAlarms returns MetricAlarms[0].StateValue == 'ALARM' and the transition appears in the alarm history. With no EventBridge rule wired to it, that state change is delivered nowhere; wiring a matching rule plus an SQS target and forcing the next transition delivers it. Recorded behavior only for the delivery legs.
Emulator: the check creates the ALB + target group, puts the AWS/ApplicationELB UnHealthyHostCount alarm, calls SetAlarmState('ALARM') and polls DescribeAlarms for StateValue == 'ALARM'; it never settles and is cut off — `Check timed out after 60.0s`. Hypothesis: SetAlarmState does not update the state DescribeAlarms reports (or the alarm-state-change event leg never reaches a terminal state).
Why: alarm state is the only observable an operator has for target health, and the negative control (nothing is delivered without a rule) cannot be distinguished from a hung service. Any client that forces or waits on an alarm transition blocks indefinitely.
Fix: alarm state transition and its visibility (cloudwatch SetAlarmState -> DescribeAlarms/DescribeAlarmHistory) plus the alarm-state-change delivery path to EventBridge, reached through this autoscaling-suite check.
Traces: autoscaling__ec2__elbv2.elb-target-health-change-event-not-delivered-without-eventbridge-rule.

## Evidence
Cause 1 — expected error code `ValidationError`; actual: AssertionError: expected ValidationError, got 'NotImplemented' ('operation PutScalingPolicy is scaffolded but not implemented in this emulator') (check line 85, `assert response["code"] == "ValidationError"`). AWS message for this class: 'You should provide either a CustomizedMetricSpecification or a PredefinedMetricSpecification'.
Cause 2 — same actual: expected ValidationError, got 'NotImplemented' ('operation PutScalingPolicy is scaffolded but not implemented in this emulator'). AWS message for this class: '1 validation error detected: Value null at 'targetTrackingConfiguration.customizedMetricSpecification.statistic' failed to satisfy constraint: Member must not be null'.
Cause 3 — same actual: expected ValidationError, got 'NotImplemented' ('operation PutScalingPolicy is scaffolded but not implemented in this emulator'). AWS message for this class: 'Target value must be between 8.51592E-12 and 1.174271E17'.
Cause 4 — expected DescribeInstances Instances[].Tags to contain the launch template's instance tag key with the value set in LaunchTemplateData.TagSpecifications; actual the key is absent: AssertionError at `assert tags.get("sim2real-marker") == marker` (the preceding `inst["InstanceType"] == "t3.small"` and `sg_ids == {sg_id}` assertions passed).
Cause 5 — expected the oldest instance 'terminated' and the newer 'running' after DesiredCapacity 2 -> 1 with TerminationPolicies=['OldestInstance']; actual no instance ever reaches 'terminated' and the check produces no terminal result: 'Check timed out after 60.0s'.
Cause 6 — expected DescribeAlarms StateValue == 'ALARM' right after SetAlarmState(StateValue='ALARM'); actual the poll never satisfies it and the check produces no terminal result: 'Check timed out after 60.0s'.

## Steps to Reproduce
Replay the six check ids in trace_ids. Minimal repros:
1. Cause 1: ec2 CreateVpc/CreateSubnet/CreateLaunchTemplate, autoscaling CreateAutoScalingGroup (Min=0/Max=1/Desired=0), then autoscaling PutScalingPolicy PolicyType='TargetTrackingScaling' with TargetTrackingConfiguration carrying BOTH PredefinedMetricSpecification={'PredefinedMetricType':'ASGAverageCPUUtilization'} and CustomizedMetricSpecification={'Namespace':...,'MetricName':...,'Statistic':'Maximum'} plus TargetValue=50.0; expect 400 ValidationError 'You should provide either a CustomizedMetricSpecification or a PredefinedMetricSpecification' and no policy/alarm.
2. Cause 2: same setup, PutScalingPolicy with CustomizedMetricSpecification={'Namespace':...,'MetricName':...,'Dimensions':[...]} and NO Statistic, TargetValue=50.0; expect 400 ValidationError '1 validation error detected: Value null at 'targetTrackingConfiguration.customizedMetricSpecification.statistic' failed to satisfy constraint: Member must not be null'.
3. Cause 3: same setup with a valid CustomizedMetricSpecification and TargetValue=0 (and again -5); expect 400 ValidationError 'Target value must be between 8.51592E-12 and 1.174271E17'.
4. Cause 4: ec2 CreateLaunchTemplate with LaunchTemplateData.TagSpecifications=[{'ResourceType':'instance','Tags':[{'Key':'<k>','Value':'<v>'}]}], autoscaling CreateAutoScalingGroup Min=1/Max=1/Desired=1 on that template, wait for the member instance to be InService, then ec2 DescribeInstances on it and read Tags.
5. Cause 5: create the same group with TerminationPolicies=['OldestInstance'], Min=1/Max=2/Desired=1; UpdateAutoScalingGroup DesiredCapacity=2, wait for both InService; UpdateAutoScalingGroup DesiredCapacity=1 and poll ec2 DescribeInstances for a 'terminated' state.
6. Cause 6: elbv2 CreateLoadBalancer (internal ALB) + CreateTargetGroup, cloudwatch PutMetricAlarm on Namespace='AWS/ApplicationELB' MetricName='UnHealthyHostCount' with TargetGroup/LoadBalancer dimensions, then cloudwatch SetAlarmState(StateValue='ALARM') and poll DescribeAlarms for StateValue=='ALARM'.
