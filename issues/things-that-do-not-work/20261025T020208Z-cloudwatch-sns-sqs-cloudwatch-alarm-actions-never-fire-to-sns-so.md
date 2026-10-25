Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__sns__sqs.alarm-actions-disabled-suppresses-notification cloudwatch__sns__sqs.alarm-composite-and-actions-and-rule-waits-for-both-real-children cloudwatch__sns__sqs.cw-alarm-action-to-deleted-topic-is-recorded-as-failed-and-delivers-nothing
Issue-Key: cloudwatch__sns__sqs.alarm-actions-disabled-suppresses-notification|_await_payload#11|Timeout|resp = sqs.receive_message(
Issue-Key: cloudwatch__sns__sqs.alarm-composite-and-actions-and-rule-waits-for-both-real-children|_wait_for#21|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__sns__sqs.cw-alarm-action-to-deleted-topic-is-recorded-as-failed-and-delivers-nothing|_wait_for_messages#6|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Commit: d26de989ce4778e024797c8509c4713b425f0550

# CloudWatch alarm actions never fire to SNS, so ActionsEnabled suppression, composite AND-rule waiting and failed-delivery-to-a-deleted-topic are all unobservable (1 cause)

> _Note: this issue was identified against the code base at commit `d26de989ce4778e024797c8509c4713b425f0550`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricAlarm

## Description
One net-new root cause.

Cause 1 of 1: no alarm action is ever dispatched to SNS, in any of its three contract arms.
AWS: an alarm that transitions to ALARM invokes its AlarmActions, publishing to the named SNS topic and onward to its SQS subscribers. Three distinguishing behaviors ride on that: (a) with ActionsEnabled=false the state still transitions but the notification is withheld, so the queue stays empty while DescribeAlarms shows ALARM; (b) a composite alarm with an AND AlarmRule waits for BOTH real children to be in ALARM before notifying, and notifies exactly once when the second arrives; (c) an AlarmAction naming a topic that has since been deleted is recorded in DescribeAlarmHistory as a FAILED action and delivers nothing. Recorded behavior: all three are observed.
Emulator: all three checks publish their data and then poll the SNS-backed queue / DescribeAlarms / DescribeAlarmHistory and are killed at the harness 60 s cap. Because nothing is ever delivered and no state advances, an emulator that withholds a notification correctly is indistinguishable from one that cannot deliver at all — the suppression and composite-wait arms have no positive control. This is the SNS/SQS instance of the alarm evaluation and action-dispatch engine not running (filed for service `cloudwatch`); that issue does not carry these replay keys.
Why: alarm-to-SNS is the primary CloudWatch notification path; every page, ticket, webhook and automation hung off it silently never fires, and the failed-delivery record that would tell an operator a topic is gone is never written either.
Fix: alarm action dispatch — the evaluator's invocation of AlarmActions/OKActions/InsufficientDataActions, the ActionsEnabled gate, composite AlarmRule derivation, and the FAILED-action history record for an unresolvable target — in the cloudwatch alarm state and its SNS publish path.
Traces: cloudwatch__sns__sqs.alarm-actions-disabled-suppresses-notification, cloudwatch__sns__sqs.alarm-composite-and-actions-and-rule-waits-for-both-real-children, cloudwatch__sns__sqs.cw-alarm-action-to-deleted-topic-is-recorded-as-failed-and-delivers-nothing.

## Evidence
All three checks: "Error: Check timed out after 60.0s" while polling the SNS-subscribed SQS queue and DescribeAlarms/DescribeAlarmHistory for the action outcome.

## Steps to Reproduce
1. CreateTopic + Subscribe an SQS queue (with the sns.amazonaws.com sqs:SendMessage policy).
2. PutMetricAlarm with that topic as AlarmAction and ActionsEnabled=false; PutMetricData breaching datapoints -> poll DescribeAlarms for ALARM and the queue for emptiness: neither resolves.
3. PutCompositeAlarm with AlarmRule "ALARM(a) AND ALARM(b)" over two real metric alarms; drive a then b into ALARM -> no single notification ever arrives.
4. PutMetricAlarm whose AlarmAction names a deleted topic; drive it into ALARM -> no FAILED action item appears in DescribeAlarmHistory.
