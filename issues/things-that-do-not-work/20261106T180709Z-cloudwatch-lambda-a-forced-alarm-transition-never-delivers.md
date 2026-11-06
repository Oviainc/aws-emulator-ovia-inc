Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__lambda.xsvc-cloudwatch-3-alarm-lambda-action-delivers-alarm-payload-to-function
Issue-Key: cloudwatch__lambda.xsvc-cloudwatch-3-alarm-lambda-action-delivers-alarm-payload-to-function|_warm_up#8|Timeout|time.sleep(5.0)
Issue-Commit: f50d8ce9e37ffa71976cdbe00198995770403bcc

# cloudwatch__lambda: a forced ALARM transition never delivers the alarmData payload to the alarm's Lambda action and writes no Succeeded Action history item

> _Note: this issue was identified against the code base at commit `f50d8ce9e37ffa71976cdbe00198995770403bcc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetAlarmState

## Description
One net-new root cause.

Cause 1 of 1: an alarm whose AlarmActions name a Lambda function ARN never invokes that function when the alarm transitions to ALARM, and DescribeAlarmHistory never records the Action item for it, so the check polls to the harness's 60 s cap and is killed. Failing check id: cloudwatch__lambda.xsvc-cloudwatch-3-alarm-lambda-action-delivers-alarm-payload-to-function.

AWS: PutMetricAlarm stores up to five actions per list (Smithy: AlarmActions targets ResourceList, length max 5) and a Lambda function ARN is a valid alarm action; DescribeAlarms echoes AlarmActions and a new alarm starts in StateValue INSUFFICIENT_DATA. SetAlarmState (Smithy: required AlarmName, StateValue, StateReason; errors InvalidFormatFault 400 / ResourceNotFound 404) forces the transition and fires the alarm's actions. CloudWatch then invokes the function as the service principal lambda.alarms.cloudwatch.amazonaws.com (the grant is a lambda:AddPermission statement with that Principal and the alarm ARN as SourceArn) and hands it an event carrying source = "aws.cloudwatch", alarmArn, and alarmData with alarmName, state.value = "ALARM", previousState.value = "INSUFFICIENT_DATA" and state.reason echoing the StateReason sent to SetAlarmState. DescribeAlarmHistory for that alarm then contains an Action history item whose HistoryData records actionState "Succeeded" and whose HistorySummary names the invoked function.

Emulator: the check never completes. The replay reports only `Error: Check timed out after 60.0s`; after SetAlarmState the check polls for (a) the custom metric datapoint the handler publishes under an exact dimension set derived from the delivered payload and (b) an Action history item with actionState "Succeeded", and neither ever appears, so the check is still polling when the harness budget expires. No invocation of the Lambda action and no Action history are observable at all.

Why: a client that wires alerting through CloudWatch -> Lambda gets no notification and no audit trail. Nothing in the response to PutMetricAlarm or SetAlarmState reveals that the action is inert, so an alarm that AWS would act on silently does nothing here, and the alarm's own history — the only place AWS reports whether the action succeeded — stays empty.

Fix: CloudWatch's alarm state-transition action dispatch (the SetAlarmState / alarm-evaluation path that must invoke each ARN in AlarmActions), the alarmData event envelope it must build for a Lambda target, the authorization of that invoke against the function's resource policy for principal lambda.alarms.cloudwatch.amazonaws.com, and the alarm-history writer that must append the Action item read back by DescribeAlarmHistory. The open issues 20261105T111951Z-cloudwatch-lambda-aws-lambda-duration-statistics-never-appear.md and 20261106T151148Z-cloudwatch-lambda-a-cloudwatch-alarm-action-on-an.md cover neighbouring alarm-action checks but neither carries this check's replay key.

Traces: cloudwatch__lambda.xsvc-cloudwatch-3-alarm-lambda-action-delivers-alarm-payload-to-function (verdict mismatch; detail `Error: Check timed out after 60.0s`).

## Evidence
Expected: after SetAlarmState(AlarmName=<alarm>, StateValue="ALARM", StateReason=<reason>), the function named in AlarmActions is invoked once with an event whose source is "aws.cloudwatch", whose alarmArn is the alarm's ARN and whose alarmData carries alarmName, state.value "ALARM", previousState.value "INSUFFICIENT_DATA" and state.reason == <reason>; DescribeAlarmHistory then returns an Action item with HistoryData.actionState == "Succeeded" whose HistorySummary names that function.
Actual: the function is never invoked (no datapoint is published under the payload-derived dimensions) and DescribeAlarmHistory never returns an Action item with actionState "Succeeded"; the check is terminated by the harness with `Error: Check timed out after 60.0s`.

## Steps to Reproduce
1. Create a python Lambda function whose role grants only cloudwatch:PutMetricData and whose handler republishes the alarm event's fields as the dimensions of a custom metric.
2. PutMetricAlarm with Namespace/MetricName that never receive data, TreatMissingData "ignore" and AlarmActions = [<function ARN>]; confirm DescribeAlarms echoes the action and StateValue INSUFFICIENT_DATA.
3. lambda:AddPermission for Principal lambda.alarms.cloudwatch.amazonaws.com with SourceArn = the alarm ARN.
4. SetAlarmState(StateValue="ALARM", StateReason=<reason>).
5. Poll GetMetricStatistics for the dimension-exact datapoint and DescribeAlarmHistory for an Action item: neither ever materialises.
