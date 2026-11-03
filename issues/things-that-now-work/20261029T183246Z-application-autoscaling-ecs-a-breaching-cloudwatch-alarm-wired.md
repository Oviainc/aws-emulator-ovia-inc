Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: application-autoscaling__ecs.a-metric-that-never-breaches-produces-no-scaling-activity
Issue-Key: application-autoscaling__ecs.a-metric-that-never-breaches-produces-no-scaling-activity|run#29|Timeout|time.sleep(_PUBLISH_INTERVAL_S)
Issue-Commit: 969cd59636329c2a8786028d0edb680658e5b91d

# application-autoscaling__ecs: a breaching CloudWatch alarm wired to a StepScaling policy never moves the ECS service's desiredCount, so no scale-in is ever executed

> _Note: this issue was identified against the code base at commit `969cd59636329c2a8786028d0edb680658e5b91d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutScalingPolicy

## Description
Cause 1 of 1: a StepScaling policy on an `ecs` / `ecs:service:DesiredCount` scalable target, invoked by a customer CloudWatch alarm that genuinely breaches, never results in a change to the ECS service's desiredCount at the consumer (ECS DescribeServices), so the scale-in never happens.

AWS: RegisterScalableTarget(ServiceNamespace=ecs, ScalableDimension=ecs:service:DesiredCount, ResourceId=service/<cluster>/<service>, MinCapacity=1, MaxCapacity=40) followed by PutScalingPolicy(PolicyType=StepScaling, StepScalingPolicyConfiguration={AdjustmentType: ChangeInCapacity, MetricAggregationType: Maximum, Cooldown: 0, StepAdjustments:[{MetricIntervalUpperBound: 0.0, ScalingAdjustment: -3}]}) returns a PolicyARN. When a CloudWatch alarm whose AlarmActions name that PolicyARN (Statistic=Maximum, Period=10, EvaluationPeriods=1, DatapointsToAlarm=1, ComparisonOperator=LessThanThreshold, TreatMissingData=notBreaching) goes to ALARM because the published datapoint sits below its threshold, Application Auto Scaling executes the policy: it calls ecs:UpdateService so ECS DescribeServices reports a decreased desiredCount (bounded below by MinCapacity), and DescribeScalingActivities records an activity for it. (Smithy: DescribeScalingActivities -> ScalingActivities with IncludeNotScaledActivities; the model does not describe execution semantics, so the execution-on-breach behavior is recorded behavior.)

Emulator: with the single value 50.0 published every 5s to the alarm's metric and the alarm's LessThanThreshold threshold at 90.0 (so it breaches), the ECS service's desiredCount stays at its initial 6 for the whole window the runner allowed — the positive-control loop that publishes and re-reads DescribeServices never observes a change, and the replay verdict is "mismatch — Check timed out after 60.0s" before the check's own 120s control budget expires. Nothing downstream of the breach is observable: no decreased desiredCount at ECS. Either the alarm never transitions to ALARM for a LessThanThreshold comparison on freshly published high-resolution custom datapoints, or the policy-execution path never issues the ECS UpdateService; both land in the same executed-scale-in that a client observes. Related but NOT covering this: open issue 20261024T115439Z-application-autoscaling-application-auto-scaling-scheduled-actions-never-fire-target.md (scheduled actions / target tracking) and 20261029T175650Z-application-autoscaling-ecs-5-net-new-root-causes.md (refusal wording, policy/alarm linkage, concurrent-policy arbitration) — neither names the StepScaling scale-in execution reaching ECS.

Why: a client that relies on alarm-driven step scaling gets no capacity change at all. Dashboards and reconciliation loops read desiredCount to confirm a scaling decision landed; here every step-scaling policy looks installed and wired yet inert, and any test or deployment gate that waits for capacity to move hangs. It also makes the negative property (a metric that never breaches must produce no scaling activity) impossible to verify, because a target that never scales for ANY metric is indistinguishable from one that correctly withholds.

Fix: the step-scaling execution path — CloudWatch's alarm-state evaluation for a LessThanThreshold alarm over just-published custom datapoints, and Application Auto Scaling's policy-invocation handler that must apply the StepAdjustment and call ECS UpdateService to set the new desiredCount (clamped to Min/MaxCapacity) and record the matching scaling activity.

Traces: application-autoscaling__ecs.a-metric-that-never-breaches-produces-no-scaling-activity.

## Evidence
Expected (AWS): with PutMetricData publishing Value=50.0 to <namespace>/<metric> dimensioned on the control service, and the control alarm at Threshold=90.0 / ComparisonOperator=LessThanThreshold / AlarmActions=[<step policy ARN>], DescribeAlarms reports StateValue == "ALARM" and ECS DescribeServices reports desiredCount strictly less than the initial 6 and at least MinCapacity 1 (ChangeInCapacity -3). Actual (emulator): DescribeServices keeps reporting desiredCount == 6 for the whole polling window and the check is reported as "mismatch — Check timed out after 60.0s"; the control leg's own assertion text is "the control service never moved off desiredCount 6 within 120s of publishing 50.0 against its threshold of 90.0". Cluster, service, policy and alarm names are per-run generated values; the requirement is that a breaching alarm bound to a StepScaling policy produces a bounded decrease in desiredCount.

## Steps to Reproduce
1. ECS: CreateCluster (no container instances), RegisterTaskDefinition (networkMode=bridge, requiresCompatibilities=[EC2]), CreateService launchType=EC2 desiredCount=6; confirm DescribeServices reports desiredCount 6.
2. application-autoscaling: RegisterScalableTarget ServiceNamespace=ecs, ResourceId=service/<cluster>/<service>, ScalableDimension=ecs:service:DesiredCount, MinCapacity=1, MaxCapacity=40.
3. PutScalingPolicy PolicyType=StepScaling with StepScalingPolicyConfiguration {AdjustmentType: ChangeInCapacity, MetricAggregationType: Maximum, Cooldown: 0, StepAdjustments: [{MetricIntervalUpperBound: 0.0, ScalingAdjustment: -3}]}; keep the returned PolicyARN.
4. cloudwatch: PutMetricAlarm Namespace=<ns>, MetricName=<m>, Dimensions=[{ServiceName: <service>}], Statistic=Maximum, Period=10, EvaluationPeriods=1, DatapointsToAlarm=1, Threshold=90.0, ComparisonOperator=LessThanThreshold, TreatMissingData=notBreaching, AlarmActions=[<PolicyARN>].
5. Loop: PutMetricData Value=50.0 for that metric/dimension every 5s while polling DescribeAlarms for StateValue == ALARM and ECS DescribeServices for desiredCount != 6.
6. Expect StateValue ALARM and desiredCount in [1, 5]; the emulator keeps desiredCount at 6 and the scale-in is never executed.
Replay check id application-autoscaling__ecs.a-metric-that-never-breaches-produces-no-scaling-activity to confirm.
