Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ecs__ssm__xray.e2e-diagnose-xray-service-latency-cause-lifecycle
Issue-Key: cloudwatch__ecs__ssm__xray.e2e-diagnose-xray-service-latency-cause-lifecycle|phase#12|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Commit: fb003e0a3fdd2a31392ea928ed9540deb9fe4c34

# cloudwatch__ecs__ssm__xray: an AWS::CloudWatch::Alarm in a CREATE_COMPLETE stack does not exist in CloudWatch, so DescribeAlarms answers an empty MetricAlarms list (1 cause)

> _Note: this issue was identified against the code base at commit `fb003e0a3fdd2a31392ea928ed9540deb9fe4c34`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAlarms

## Description
One net-new root cause behind 1 failing check.

Cause 1 of 1: a stack that declares an AWS::CloudWatch::Alarm reaches CREATE_COMPLETE and its Outputs resolve the alarm's Ref to the alarm name the template mints, but the alarm is never written to CloudWatch state, so DescribeAlarms for that exact name returns no MetricAlarm at all.

AWS: CloudFormation provisions an AWS::CloudWatch::Alarm by calling PutMetricAlarm, and Ref on that resource returns the alarm's name. Immediately after the stack settles CREATE_COMPLETE, DescribeAlarms with AlarmNames=[<that name>] and AlarmTypes=["MetricAlarm"] returns exactly one MetricAlarm whose Namespace, MetricName, ExtendedStatistic, Period, EvaluationPeriods, Threshold, ComparisonOperator, Dimensions and TreatMissingData are the values the template declared (here: the template's Namespace parameter, MetricName "Time", ExtendedStatistic "p90", Period from the AlarmPeriod parameter, EvaluationPeriods 3, Threshold 15000, ComparisonOperator "GreaterThanThreshold", TreatMissingData "notBreaching"). Smithy (cloudwatch): DescribeAlarms.AlarmNames @length 0..100, output members MetricAlarms/CompositeAlarms/LogAlarms — a named alarm that exists is listed in MetricAlarms; there is no error for a missing alarm, the list simply comes back empty, which is why this surfaces as silently absent state rather than a refusal. The alarm NAME, period and namespace in any one run are minted per run and are not part of the requirement; the requirement is that the alarm the stack declared is describable with the template's declared properties.

Emulator: the stack is created, reaches CREATE_COMPLETE, and DescribeStacks returns the full expected Output set including ServiceLatencyAlarmName. DescribeAlarms(AlarmNames=[<that output value>], AlarmTypes=["MetricAlarm"]) then answers MetricAlarms: [] — the check fails in its first phase with `[phase 1/6 deploy] alarm '<token>-service-latency' should be described exactly once, got []`. CloudFormation therefore reports a provisioned alarm resource that CloudWatch does not hold.

Why: a client that deploys its monitoring as a stack gets a stack that claims success while no alarm exists. Every downstream read fails or is meaningless: DescribeAlarms/DescribeAlarmHistory find nothing, SetAlarmState and alarm actions have no target, the alarm never evaluates or fires, and an operator diagnosing latency from the alarm (the whole scenario here: alarm -> metric statistics -> X-Ray trace) cannot even begin. The divergence is silent — there is no error anywhere to tell the client its monitoring was never created.

Fix: the CloudFormation resource provider for AWS::CloudWatch::Alarm in the cloudformation service crate must apply the declared properties to CloudWatch's own alarm state (the same state PutMetricAlarm writes and DescribeAlarms reads) rather than only recording a logical/physical id for Ref, and the stack delete must remove it again. Open issues 20261104T062603Z-cloudwatch-ecs-ssm-a-deployed-service-latency-alarm.md, 20261102T120041Z-cloudwatch-dynamodb-lambda-an-aws-cloudwatch-alarm-declared.md and 20261108T001650Z-cloudwatch-logs-transfer-an-aws-cloudwatch-alarm-created.md describe the same shape of defect for other service headings and do not carry this check's replay key.

Traces: cloudwatch__ecs__ssm__xray.e2e-diagnose-xray-service-latency-cause-lifecycle (phase 1/6 deploy).

## Evidence
Expected (AWS): after CreateStack of a template whose ServiceLatencyAlarm is an AWS::CloudWatch::Alarm (Namespace=<FlintNamespace param>, MetricName=Time, Dimensions Camera_name=ALL / Operation=FrameQuartzLag, ExtendedStatistic=p90, Period=<AlarmPeriod param>, EvaluationPeriods=3, Threshold=15000, ComparisonOperator=GreaterThanThreshold, TreatMissingData=notBreaching) reaches CREATE_COMPLETE, DescribeAlarms(AlarmNames=[Ref ServiceLatencyAlarm], AlarmTypes=["MetricAlarm"]) returns exactly 1 MetricAlarm carrying those properties.

Actual (emulator): the stack reaches CREATE_COMPLETE and DescribeStacks returns all nine expected Outputs including ServiceLatencyAlarmName, but DescribeAlarms for that name returns MetricAlarms: []. Check verdict: AssertionError: [phase 1/6 deploy] alarm '<minted token>-service-latency' should be described exactly once, got [].

## Steps to Reproduce
1. CreateStack with a template declaring an AWS::CloudWatch::Alarm (AlarmName via Fn::Sub on a parameter, ExtendedStatistic p90, Period 30, EvaluationPeriods 3, Threshold 15000, ComparisonOperator GreaterThanThreshold, TreatMissingData notBreaching) and an Output whose Value is {"Ref": "<that alarm>"}.
2. Poll DescribeStacks until the stack reaches CREATE_COMPLETE.
3. Read the alarm name from the stack's Outputs.
4. Call DescribeAlarms(AlarmNames=[<that name>], AlarmTypes=["MetricAlarm"]).
5. Expected: exactly one MetricAlarm with the template's declared properties. Observed: MetricAlarms is empty.
