Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__rds.rds-alarm-action-blocked-by-topic-policy-denying-cloudwatch
Issue-Key: cloudwatch__rds.rds-alarm-action-blocked-by-topic-policy-denying-cloudwatch|_poll_until#26|Timeout|time.sleep(interval_s)
Issue-Commit: 768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5

# CloudWatch metric alarms never evaluate or fire their actions, so no alarm notification is delivered and no Action history is recorded

> _Note: this issue was identified against the code base at commit `768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAlarmHistory

## Description
One cause. AWS: a MetricAlarm created with TreatMissingData='breaching' on a metric that receives no datapoints transitions to ALARM on CloudWatch's own evaluation clock, with no SetAlarmState call. On that transition CloudWatch executes each entry of AlarmActions and records one Action item per action in DescribeAlarmHistory (HistoryItemType='Action'), whose parsed HistoryData names the notificationResource and whether the action Succeeded or Failed — a Failed item for an SNS topic whose resource policy denies cloudwatch.amazonaws.com sns:Publish names the SNS:Publish authorization failure and carries no publishedMessage, while the allowed topic's item is Succeeded and its notification reaches the subscribed queue. Recorded behavior only for the HistoryData field names.

Emulator: nothing happens. The alarm is created but never transitions, so no action is ever executed, no Action history item is ever recorded, and no notification reaches the queue; the check polls for the allowed topic's notification until the harness's wall clock expires ('Check timed out after 60.0s'). Because no action ever fires, the authorization decision the check is about (the denied topic must record Failed and deliver nothing) is never made at all — the correct-looking absence at the denied topic is produced by the alarm never firing, not by the Deny.

Why: every alarm-driven workflow is inert. A client that creates an alarm and waits for the state change, for the SNS notification, or for the Action history that tells it whether the notification was delivered, waits forever; and an emulator that later fires actions without consulting the target topic's resource policy would deliver to a topic AWS refuses.

Fix: the CloudWatch alarm evaluation path — alarms must be evaluated against their metric data (honouring TreatMissingData), must transition state, must execute AlarmActions through the target service while evaluating that target's resource policy, and must record one Action history item per action with its Succeeded/Failed outcome.

Traces: cloudwatch__rds.rds-alarm-action-blocked-by-topic-policy-denying-cloudwatch.

## Evidence
cloudwatch__rds.rds-alarm-action-blocked-by-topic-policy-denying-cloudwatch — verdict 'mismatch', 'Check timed out after 60.0s'. The check creates two SNS topics (one with the default policy, one with an explicit Deny of cloudwatch.amazonaws.com sns:Publish), subscribes one SQS queue to both, creates ONE MetricAlarm on AWS/RDS DatabaseConnections with TreatMissingData='breaching' and AlarmActions=[allowed topic, denied topic], and then polls (budget _ALARM_BUDGET_S=600 / _DELIVERY_BUDGET_S=300) for the allowed topic's notification in the queue and for DescribeAlarmHistory Action items. Neither ever appears, so the barrier poll runs to the harness timeout.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. sns: CreateTopic twice; on the second, SetTopicAttributes with a policy containing an explicit Deny of sns:Publish for Principal Service cloudwatch.amazonaws.com. sqs: CreateQueue and subscribe it to both topics.
3. cloudwatch: PutMetricAlarm on Namespace AWS/RDS, MetricName DatabaseConnections, Dimensions [{DBInstanceIdentifier: <an identifier naming no instance>}], TreatMissingData='breaching', EvaluationPeriods 1, AlarmActions [both topic ARNs].
4. Poll DescribeAlarms for StateValue and DescribeAlarmHistory with HistoryItemType='Action', and ReceiveMessage on the queue: the alarm never leaves INSUFFICIENT_DATA, no Action history item is recorded, and no notification is delivered.
