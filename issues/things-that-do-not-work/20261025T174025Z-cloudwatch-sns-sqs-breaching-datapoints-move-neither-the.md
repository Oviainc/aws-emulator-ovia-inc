Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__sns__sqs.alarm-composite-and-actions-disabled-actions-transition-but-delivery-blocked
Issue-Key: cloudwatch__sns__sqs.alarm-composite-and-actions-disabled-actions-transition-but-delivery-blocked|_wait_for#35|Timeout|time.sleep(interval_s)
Issue-Commit: b5c349f78c27b93d7bc476c26ede97815cd374b3

# cloudwatch__sns__sqs: breaching datapoints move neither the actions-enabled nor the actions-disabled twin into ALARM, so the DisableAlarmActions delivery differential is unobservable (1 cause)

> _Note: this issue was identified against the code base at commit `b5c349f78c27b93d7bc476c26ede97815cd374b3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricAlarm

## Description
Cause 1 of 1: two identical high-resolution metric alarms over the same metric, differing only in ActionsEnabled, never transition on the datapoints the check publishes, and no notification reaches the SNS->SQS subscriber, so the one thing the check isolates - that DisableAlarmActions blocks DELIVERY while evaluation still TRANSITIONS the alarm - cannot be observed at all.

AWS: Smithy: PutMetricAlarm accepts Period=10 with StorageResolution=1 datapoints (high resolution), EvaluationPeriods=1, Threshold, ComparisonOperator=GreaterThanThreshold, TreatMissingData="ignore", AlarmActions=[<sns arn>]; DisableAlarmActions/EnableAlarmActions (AlarmNames required, max 100, no modelled errors) flip ActionsEnabled as reported by DescribeAlarms; DescribeAlarmHistory (HistoryItemType enum, AlarmTypes) records StateUpdate and Action items. AWS behaviour, as the check records it: a stream of datapoints above the threshold drives BOTH alarms INSUFFICIENT_DATA->ALARM, and each alarm's StateReason cites the graded datapoint value in brackets (the fixed text is AWS's "Threshold Crossed: ...datapoint [<value> (<timestamp>)] was greater than the threshold (<threshold>)" form; the value, timestamp and threshold come from the published data and the alarm). The ActionsEnabled twin delivers one SNS notification whose envelope TopicArn is the alarm action target and whose Message carries AlarmName, OldStateValue="INSUFFICIENT_DATA", NewStateValue="ALARM" and a Trigger naming the shared Namespace/MetricName. The DisableAlarmActions twin records a StateUpdate into ALARM in DescribeAlarmHistory yet delivers nothing and records no Action history item. After EnableAlarmActions, real datapoints driving it OK then ALARM again deliver its notification with OldStateValue="OK".

Emulator: the check hangs and the replay verdict is the bare "Check timed out after 60.0s". Its first barrier is "both alarms to reach ALARM from the same breaching datapoints", which re-publishes the breaching datapoint every 5 s and re-reads DescribeAlarms; it never clears, so neither twin leaves its initial state on published data, the control notification is never delivered to the queue, and no later leg (the muted twin's StateUpdate history, the absence of its Action items, or the post-EnableAlarmActions redelivery) is reached.

Why: an operator's entire alerting contract rests on this: datapoints cross a threshold, the alarm transitions, the action fires, the subscriber is notified - and ActionsEnabled is the one switch that mutes the last step without muting the first. A client that mutes an alarm and re-enables it cannot verify either half here: nothing ever fires, so "muted" and "broken" are indistinguishable, and a runbook validated against the emulator will silently fail in production.

Fix: the cloudwatch alarm evaluator must grade stored metric data (including Period=10 high-resolution series under StorageResolution=1) against the alarm's Threshold/ComparisonOperator/EvaluationPeriods/TreatMissingData and commit the state transition with AWS's StateReason text; the alarm-action dispatcher must then publish to SNS AlarmActions on that transition and record the Action item in alarm history, gated by ActionsEnabled. The state-update records must appear in DescribeAlarmHistory for the muted alarm as well as the enabled one.

Related already-open issues, named so this is not closed as a duplicate of either: 20261025T020208Z-cloudwatch-alarms-never-evaluate-published-metric-data-no.md and 20261025T053543Z-cloudwatch-sns-sqs-a-child-alarm-s-statereason.md. Neither carries this check's replay key: the first is scoped to standard-resolution evaluation and does not cover the ActionsEnabled delivery gate, and the second is about the CONTENT of a StateReason and a failed-action record on alarms that do fire.

Traces: cloudwatch__sns__sqs.alarm-composite-and-actions-disabled-actions-transition-but-delivery-blocked (mismatch, "Check timed out after 60.0s").

## Evidence
Expected (AWS): with two alarms on the same Namespace/MetricName/Dimensions (Statistic=Maximum, Period=10, EvaluationPeriods=1, Threshold=50.0, ComparisonOperator=GreaterThanThreshold, TreatMissingData="ignore", AlarmActions=[<topic>]) and DisableAlarmActions applied to one of them, repeated PutMetricData of a value above the threshold moves BOTH alarms' StateValue to "ALARM" with a StateReason citing the graded datapoint; the actions-enabled alarm delivers one SNS->SQS message with OldStateValue="INSUFFICIENT_DATA"/NewStateValue="ALARM" and a Trigger naming that Namespace/MetricName; the actions-disabled alarm has a StateUpdate-into-ALARM history item, zero Action history items and delivers nothing.

Actual (emulator): DescribeAlarms never reports StateValue="ALARM" for either alarm no matter how many breaching datapoints are published, the queue receives no notification, and the check is killed by the harness at "Check timed out after 60.0s" while still on its first barrier. ActionsEnabled is reported correctly (false for the muted twin, true for the control), which is the only leg that passes.

## Steps to Reproduce
1. sns.create_topic; sqs.create_queue; sqs.get_queue_attributes(QueueArn); sqs.set_queue_attributes with a Policy allowing sns.amazonaws.com sqs:SendMessage conditioned on aws:SourceArn=<topic>; sns.subscribe(Protocol="sqs", Endpoint=<queue arn>).
2. For key in {enabled, muted}: cloudwatch.put_metric_alarm(AlarmName=<name-key>, Namespace=<unique ns>, MetricName="latency", Dimensions=[{Name:"Run",Value:<unique>}], Statistic="Maximum", Period=10, EvaluationPeriods=1, Threshold=50.0, ComparisonOperator="GreaterThanThreshold", TreatMissingData="ignore", AlarmActions=[<topic>]).
3. cloudwatch.disable_alarm_actions(AlarmNames=[<name-muted>]); confirm DescribeAlarms reports ActionsEnabled false for it and true for <name-enabled>.
4. Loop: cloudwatch.put_metric_data(Namespace=<ns>, MetricData=[{MetricName:"latency", Dimensions:<dims>, Value:211.0, StorageResolution:1}]) every 5 s and re-read cloudwatch.describe_alarms. Expected: both reach StateValue="ALARM". Observed: neither ever leaves its initial state.
5. Poll sqs.receive_message. Expected: one message for <name-enabled> with NewStateValue="ALARM". Observed: none.
6. cloudwatch.describe_alarm_history(AlarmName=<name-muted>, HistoryItemType="StateUpdate"). Expected: a StateUpdate into ALARM. Observed: none.
