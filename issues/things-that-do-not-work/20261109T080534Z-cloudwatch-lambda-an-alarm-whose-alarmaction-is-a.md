Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__lambda.xsvc-cloudwatch-3-alarm-lambda-action-delivers-alarm-payload-to-function cloudwatch__lambda.xsvc-cloudwatch-3-alarm-lambda-action-without-invoke-permission-denied
Issue-Key: cloudwatch__lambda.xsvc-cloudwatch-3-alarm-lambda-action-delivers-alarm-payload-to-function|_poll#22|Timeout|time.sleep(5.0)
Issue-Key: cloudwatch__lambda.xsvc-cloudwatch-3-alarm-lambda-action-without-invoke-permission-denied|_poll#18|Timeout|time.sleep(5.0)
Issue-Commit: 1945ee51cfaab7e95ed50a7b8bc82cc22a5254e2

# cloudwatch__lambda: an alarm whose AlarmAction is a Lambda function ARN never invokes the function on a forced ALARM transition and writes no Action history item

> _Note: this issue was identified against the code base at commit `1945ee51cfaab7e95ed50a7b8bc82cc22a5254e2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetAlarmState

## Description
One net-new root cause, grounded in 2 failing checks.

Cause 1 of 1: a metric alarm carrying a Lambda function ARN in AlarmActions never executes that action when the alarm transitions into ALARM, and DescribeAlarmHistory records no HistoryItemType=Action item for it. Failing checks: cloudwatch__lambda.xsvc-cloudwatch-3-alarm-lambda-action-delivers-alarm-payload-to-function, cloudwatch__lambda.xsvc-cloudwatch-3-alarm-lambda-action-without-invoke-permission-denied.

AWS: Smithy (com-amazonaws-cloudwatch-graniteserviceversion20100801) models PutMetricAlarm with AlarmActions as a ResourceList (@length 0..5) and SetAlarmState with AlarmName/StateValue/StateReason required (StateReason @length 0..1023), both returning an empty output; SetAlarmState's only modeled errors are InvalidFormatFault (400/InvalidFormat) and ResourceNotFound (404/ResourceNotFound). The model does not describe action execution, so the delivery contract is recorded behavior only, and the checks state it: after put_metric_alarm(..., AlarmActions=[<function ARN>]) and a lambda:InvokeFunction resource-policy grant to Principal lambda.alarms.cloudwatch.amazonaws.com scoped by SourceArn=<alarm ARN>/SourceAccount=<account>, a set_alarm_state(StateValue='ALARM', StateReason=<reason>) must asynchronously invoke the function with an event carrying source == 'aws.cloudwatch', alarmArn == the alarm's ARN, alarmData.alarmName == the alarm name, alarmData.state.value == 'ALARM', alarmData.previousState.value == the state the alarm was in before (here 'INSUFFICIENT_DATA', the state DescribeAlarms reports for a freshly created alarm), and alarmData.state.reason equal to the StateReason given verbatim; and DescribeAlarmHistory(HistoryItemType='Action') must then contain an item whose HistoryData has actionState == 'Succeeded' and whose HistorySummary names the invoked function. The negative twin adds that an alarm with NO matching resource-policy grant must instead record a Failed action and must not invoke the function, and that granting it and re-driving the alarm then delivers. The variable parts of all of this (alarm name, alarm ARN, function name, the StateReason text, the account id) come from the request and the emulator's state; the fixed parts are the 'aws.cloudwatch' source, the alarmData field names and the 'Succeeded'/'Failed' actionState vocabulary.

Emulator: nothing is delivered and nothing is recorded. Both checks run to the harness cap and report "Check timed out after 180.0s" while polling — the first waiting for the function to record the alarm event (it re-publishes the payload's fields as metric dimensions, so the read proves which payload arrived) and for the alarm's Action history to show a Succeeded item; the second waiting for the granted sibling alarm's payload, which is its positive control and barrier, so the denied arm is never even reached. Every step before the transition succeeds: PutMetricAlarm stores AlarmActions == [<function ARN>], DescribeAlarms reports StateValue 'INSUFFICIENT_DATA', AddPermission/GetPolicy report the single lambda.alarms.cloudwatch.amazonaws.com statement, and SetAlarmState itself is accepted. The divergence is that the accepted state change executes no action and appends no Action history.

Why: a client relying on AWS's behavior gets an alarm pipeline that is silently inert. Alarm-driven remediation and notification wired to a Lambda target never runs, so neither the happy path nor its permission boundary is observable: the Succeeded/Failed Action history AWS uses to diagnose a mis-scoped resource policy is simply absent, and a user cannot distinguish "action delivered" from "action denied" from "action never attempted".

Fix: the CloudWatch alarm state machine and its action dispatcher — the SetAlarmState handler must, after committing the transition, evaluate the alarm's AlarmActions, invoke a Lambda-ARN action through the Lambda data plane as principal lambda.alarms.cloudwatch.amazonaws.com with the alarm's SourceArn/SourceAccount, and append the corresponding Action item to the alarm's history state (whose actionState is Succeeded on a permitted invoke and Failed when the function's resource policy does not grant the alarm). The alarm-history store read by DescribeAlarmHistory must gain those items.

Traces: the two failing check ids above are examples of the one cause. Related but distinct open issues that do NOT carry these two checks' replay keys: 20261105T111951Z-cloudwatch-lambda-aws-lambda-duration-statistics-never-appear.md, 20261106T151148Z-cloudwatch-lambda-a-cloudwatch-alarm-action-on-an.md and 20261106T180709Z-cloudwatch-lambda-a-forced-alarm-transition-never-delivers.md.

## Evidence
Expected (AWS): set_alarm_state(AlarmName=<alarm>, StateValue='ALARM', StateReason=<reason>) on an alarm with AlarmActions=[<fn ARN>] and a lambda:InvokeFunction grant to lambda.alarms.cloudwatch.amazonaws.com (SourceArn=<alarm ARN>) invokes the function once with {source: 'aws.cloudwatch', alarmArn: <alarm ARN>, alarmData: {alarmName: <alarm>, state: {value: 'ALARM', reason: <reason verbatim>}, previousState: {value: 'INSUFFICIENT_DATA'}}}, and DescribeAlarmHistory(HistoryItemType='Action') then returns an item with HistoryData.actionState == 'Succeeded' whose HistorySummary names the function. For an alarm with no matching grant the action is recorded Failed and the function is not invoked.

Actual (emulator): no invoke and no history. Both checks fail with "Error: Check timed out after 180.0s":
- xsvc-cloudwatch-3-alarm-lambda-action-delivers-alarm-payload-to-function — times out polling for the function to record an alarm event with exactly the expected dimension set, and for the Succeeded Action history item.
- xsvc-cloudwatch-3-alarm-lambda-action-without-invoke-permission-denied — times out on its positive control (the granted sibling alarm's payload), so the denied arm never reports.

PutMetricAlarm, DescribeAlarms (StateValue 'INSUFFICIENT_DATA'), AddPermission/GetPolicy and SetAlarmState all succeed, so the gap is action execution plus Action-history recording, not alarm configuration.

## Steps to Reproduce
1. Create an IAM role Lambda can assume, granting cloudwatch:PutMetricData.
2. Create a python3.12 function with that role whose handler re-publishes the alarm event's fields (source, alarmArn, alarmData.alarmName, alarmData.state.value, alarmData.previousState.value, alarmData.state.reason) as dimensions of a custom metric, and wait for it to be Active.
3. cloudwatch put_metric_alarm(AlarmName=<a>, Namespace=<ns>, MetricName='Never', Statistic='Maximum', Period=60, EvaluationPeriods=1, Threshold=1.0, ComparisonOperator='GreaterThanThreshold', TreatMissingData='ignore', AlarmActions=[<fn ARN>]); confirm DescribeAlarms reports AlarmActions == [<fn ARN>] and StateValue 'INSUFFICIENT_DATA'.
4. lambda add_permission(FunctionName=<fn>, StatementId='cw-alarm', Action='lambda:InvokeFunction', Principal='lambda.alarms.cloudwatch.amazonaws.com', SourceArn=<alarm ARN>, SourceAccount=<account>).
5. cloudwatch set_alarm_state(AlarmName=<a>, StateValue='ALARM', StateReason=<reason>).
6. Poll GetMetricStatistics on the custom metric with the exact expected dimension set: AWS yields Sum >= 1; the emulator yields nothing. Poll DescribeAlarmHistory(AlarmName=<a>, HistoryItemType='Action'): AWS yields a Succeeded item naming the function; the emulator yields none.
7. Repeat with a second alarm that has no add_permission for its ARN: AWS records a Failed action and does not invoke; the emulator records nothing.
8. Re-replay the two check ids in trace_ids to confirm.
