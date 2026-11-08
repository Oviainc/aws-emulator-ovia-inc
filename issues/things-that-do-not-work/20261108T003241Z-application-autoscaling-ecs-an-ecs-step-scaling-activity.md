Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: application-autoscaling__ecs.a-metric-that-never-breaches-produces-no-scaling-activity
Issue-Key: application-autoscaling__ecs.a-metric-that-never-breaches-produces-no-scaling-activity|run#39|AssertionError|assert repaired[0]['StatusCode'] == 'InProgress', (
Issue-Commit: f4d58d8ba640bd38bd7af13aafdf9baa78485a99

# application-autoscaling__ecs: an ECS step-scaling activity is recorded Successful immediately instead of staying InProgress until ECS fulfils the new desiredCount

> _Note: this issue was identified against the code base at commit `f4d58d8ba640bd38bd7af13aafdf9baa78485a99`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeScalingActivities

## Description
Cause 1 of 1: the scaling activity Application Auto Scaling records when a StepScaling policy resizes an ECS service is written straight into the terminal state `Successful`, with no fulfilment tracking of the downstream ECS service. Failing check: application-autoscaling__ecs.a-metric-that-never-breaches-produces-no-scaling-activity.

AWS: Smithy (application-autoscaling) — DescribeScalingActivities returns ScalingActivity{ActivityId, ServiceNamespace, ResourceId, ScalableDimension, Description, Cause, StartTime, StatusCode (required, enum ScalingActivityStatusCode = Pending|InProgress|Successful|Overridden|Unfulfilled|Failed), StatusMessage, EndTime, NotScaledReasons}; StatusCode doc: "Indicates the status of the scaling activity", StatusMessage doc: "A simple message about the current status of the scaling activity", EndTime doc: "The Unix timestamp for when the scaling activity ended". The per-namespace status narrative is recorded behavior only: after a step adjustment fires, Application Auto Scaling calls the target service (here ECS UpdateService) and records the activity as NOT yet terminal — StatusCode `InProgress` with StatusMessage whose fixed text is `Successfully set desired count to ` + <the computed desiredCount> + `. Waiting for change to be fulfilled by ` + <the fulfilling namespace> + `.` (the count comes from the computed adjustment and the namespace from the target's ServiceNamespace, here `ecs`), alongside Description whose fixed text is `Setting desired count to ` + <the computed desiredCount> + `.`. It only leaves `InProgress` when the target service actually fulfils the change (then `Successful`), or is superseded/unfulfillable (`Overridden`/`Unfulfilled`/`Failed`). On an estate whose ECS cluster has NO container instances the tasks can never be placed or drained, so the activity cannot become terminal at all and stays `InProgress` indefinitely; a reported `Successful` is a claim that ECS finished a change it demonstrably cannot finish.

Emulator: after the subject's alarm threshold is raised so the published metric breaches, the emulator does execute the step adjustment and ECS reports the reduced desiredCount, but the newest entry DescribeScalingActivities(ServiceNamespace='ecs', ResourceId='service/<cluster>/<service>', ScalableDimension='ecs:service:DesiredCount', IncludeNotScaledActivities=True) returns is `('Successful', 'Setting desired count to 3.')` — StatusCode `Successful` the moment the UpdateService call is issued, rather than `InProgress` pending ECS fulfilment. The Description text matches; only the status (and therefore the status narrative/StatusMessage and EndTime that go with it) diverges. The emulator never models the fulfilment step: it treats "told ECS" as "ECS finished".

Why: a client that polls DescribeScalingActivities to decide whether a scale-out/scale-in has actually landed is told the change completed successfully while the ECS service still has runningCount 0 and will never reach the new desiredCount. Deployment gates, test harnesses and operator tooling that wait for `InProgress` -> `Successful` (or that surface a stuck/unfulfillable scaling activity) can never observe a pending or a failed fulfilment against the emulator: every activity is instantly green, so the `Unfulfilled`/`Failed`/pending paths and the "waiting for change to be fulfilled by ecs" status narrative are untestable and a genuinely stuck capacity change is reported as done.

Fix: the scaling-activity state component behind the application-autoscaling step-scaling execution path (the activity record written when the policy drives the ECS-namespace UpdateService, in the application-autoscaling service's scaling-engine/activity state and read back by its describe_scaling_activities handler): an activity must be created non-terminal (`InProgress`, with the ecs-fulfilment-pending StatusMessage above and no EndTime) and only transition to a terminal StatusCode when the target service's own state shows the capacity change fulfilled.

Traces: application-autoscaling__ecs.a-metric-that-never-breaches-produces-no-scaling-activity (leg 3). Related but distinct: open issue 20261103T205324Z-application-autoscaling-ecs-an-alarm-driven-stepscaling-policy.md filed the same check when the alarm -> policy -> ECS path produced NO scale-in at all (the check timed out); that path now executes and the check gets past legs 1 and 2, leaving this terminal-status defect, which that issue's replay key does not cover.

## Evidence
Suite verdict: [mismatch] application-autoscaling__ecs.a-metric-that-never-breaches-produces-no-scaling-activity

Assertion that failed (leg 3, after the subject alarm's threshold was raised so the same published value breaches and the subject ECS service's desiredCount dropped from 6):
  assert repaired[0]['StatusCode'] == 'InProgress'
  AssertionError: leg 3: the subject's newest activity is [('Successful', 'Setting desired count to 3.')], expected InProgress -- this estate has no container instances, so ECS can never finish fulfilling the new desiredCount and the activity cannot reach a different terminal state

Expected (AWS), for a target whose computed post-scale count is N on an ECS cluster with no container instances:
  DescribeScalingActivities(...IncludeNotScaledActivities=True).ScalingActivities[0] =
    StatusCode    = 'InProgress'                     (fixed enum value; stays until ECS fulfils)
    Description   = 'Setting desired count to N.'    (fixed text + N from the computed adjustment)
    StatusMessage = 'Successfully set desired count to N. Waiting for change to be fulfilled by ecs.'
                    (fixed text; N from the computed adjustment, 'ecs' from the target's ServiceNamespace)
    EndTime       = absent while non-terminal

Actual (emulator):
    StatusCode    = 'Successful'   <-- terminal immediately, no fulfilment tracking
    Description   = 'Setting desired count to 3.'  (matches the expected shape)
    StatusMessage/EndTime not reached by the check because the StatusCode assertion fails first.

Note: the integer 3 above is this run's computed count (ChangeInCapacity -3 from a start of 6) and the service/cluster names are per-run values; only the fixed text and the rule that produces the variable parts are the requirement. Legs 1 and 2 of the check passed, so the breach-driven scale-in, the OK/ALARM alarm states and the "never-breaching metric produces no activity at all" absence are all now served correctly; the divergence is confined to the activity's status lifecycle.

## Steps to Reproduce
1. Create an ECS cluster with NO container instances, register a task definition with networkMode 'bridge' and requiresCompatibilities ['EC2'], and CreateService(desiredCount=6, launchType='EC2'). Wait until DescribeServices reports desiredCount 6.
2. RegisterScalableTarget(ServiceNamespace='ecs', ResourceId='service/<cluster>/<service>', ScalableDimension='ecs:service:DesiredCount', MinCapacity=1, MaxCapacity=40).
3. PutScalingPolicy(PolicyName=<p>, PolicyType='StepScaling', StepScalingPolicyConfiguration={'AdjustmentType':'ChangeInCapacity','MetricAggregationType':'Maximum','Cooldown':0,'StepAdjustments':[{'MetricIntervalUpperBound':0.0,'ScalingAdjustment':-3}]}); keep the returned PolicyARN.
4. PutMetricAlarm(Namespace='sim2real/behav/application-autoscaling', MetricName='NonBreachDriver', Dimensions=[{'Name':'ServiceName','Value':<service>}], Statistic='Maximum', Period=10, EvaluationPeriods=1, DatapointsToAlarm=1, Threshold=90.0, ComparisonOperator='LessThanThreshold', TreatMissingData='notBreaching', AlarmActions=[<PolicyARN>]).
5. PutMetricData Value=50.0 on that metric every 5s until DescribeServices shows the service's desiredCount has dropped below 6 (it does).
6. Call DescribeScalingActivities(ServiceNamespace='ecs', ResourceId='service/<cluster>/<service>', ScalableDimension='ecs:service:DesiredCount', IncludeNotScaledActivities=True) and read ScalingActivities[0].
   Emulator: StatusCode == 'Successful'.
   AWS: StatusCode == 'InProgress' with StatusMessage 'Successfully set desired count to <count>. Waiting for change to be fulfilled by ecs.', and it stays InProgress because the cluster has no container instances to place the tasks on.
