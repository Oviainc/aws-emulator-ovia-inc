Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__rds.rds-alarm-action-blocked-by-topic-policy-denying-cloudwatch
Issue-Key: cloudwatch__rds.rds-alarm-action-blocked-by-topic-policy-denying-cloudwatch|run#32|AssertionError|assert blocked.get("actionState") == "Failed", (
Issue-Commit: dbe9ca7a8a80909c811e91f1b58fc3cbb14634ed

# An alarm action to an SNS topic whose policy explicitly denies cloudwatch.amazonaws.com sns:Publish is recorded as Succeeded instead of Failed (topic resource policy never evaluated)

> _Note: this issue was identified against the code base at commit `dbe9ca7a8a80909c811e91f1b58fc3cbb14634ed`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAlarmHistory

## Description
One net-new root cause in CloudWatch's alarm-action execution path.

Cause 1 of 1: when a metric alarm fires its AlarmActions, the target SNS topic's resource policy is not evaluated, so a publish that AWS refuses is recorded as a successful action.

AWS: recorded behavior only — Smithy: cloudwatch#AlarmHistoryItem.HistoryData is 'Data about the alarm, in JSON format' and cloudwatch#DescribeAlarmHistory with HistoryItemType=Action returns one item per executed action; the item's JSON shape (version/actionState/notificationResource/stateValue/error/publishedMessage) is not modeled. Recorded against real AWS: an alarm action aimed at a topic whose policy contains an explicit Deny of sns:Publish for the service principal cloudwatch.amazonaws.com produces an Action history item with actionState 'Failed', an 'error' naming the authorization failure ('not authorized to perform: SNS:Publish' and the topic ARN), and NO publishedMessage member; the queue subscribed to that topic receives nothing. The sibling action to a topic with the default policy, executed in the same evaluation, is 'Succeeded' and is delivered. Removing the Deny makes a fresh alarm's action to the same topic succeed and deliver — so the Deny is the only thing suppressing delivery.
Emulator: the alarm evaluates and both actions execute, but the action to the explicitly denying topic is recorded as {'version': '1.0', 'actionState': 'Succeeded', 'notificationResource': '<denied topic ARN>', 'stateValue': 'ALARM'} — no 'error', and the publish is actually performed. The Deny statement itself IS stored (GetTopicAttributes reads it back), so the gap is the authorization decision on the asynchronous publish CloudWatch makes, not policy persistence.
Why: a client that uses an SNS topic policy to control which principals may publish gets no signal that CloudWatch's publish was refused: the alarm history reports success, so monitoring/alerting that watches for Failed alarm actions never fires, and a notification AWS would have suppressed is delivered to every subscriber of the denied topic. Negative authorization tests against the emulator pass when the real deployment would silently stop notifying.
Fix: the alarm-action execution component must evaluate the target SNS topic's resource policy for the cloudwatch.amazonaws.com service principal before publishing (equivalently, SNS's Publish authorization must be applied to the internal publish CloudWatch makes), suppress the publish on an explicit Deny, and record the action in alarm history with actionState 'Failed', an error naming the SNS:Publish authorization failure and the topic ARN, and no publishedMessage.
Traces: cloudwatch__rds.rds-alarm-action-blocked-by-topic-policy-denying-cloudwatch.

Related open issues (neither carries this check's replay key, and both describe the opposite symptom of alarms not evaluating/firing at all, whereas here the alarm fires and the authorization step is what is missing): 20261024T030130Z-cloudwatch-rds-cloudwatch-metric-alarms-never-evaluate-or-fire-their.md and 20261025T020208Z-cloudwatch-sns-sqs-cloudwatch-alarm-actions-never-fire-to-sns-so.md.

## Evidence
Replayed check cloudwatch__rds.rds-alarm-action-blocked-by-topic-policy-denying-cloudwatch (verdict: mismatch).

Assertion that failed (check line 236):
  AssertionError: an alarm action to a topic denying cloudwatch.amazonaws.com sns:Publish must FAIL; got {'version': '1.0', 'actionState': 'Succeeded', 'notificationResource': 'arn:aws:sns:us-west-2:000000000000:sim2real-xsvc-deny-1fd242abc9e8', 'stateValue': 'ALARM'}

Setup, as replayed: two SNS topics subscribed to one SQS queue; the second topic's policy has an appended statement {Sid: DenyCloudWatchPublish, Effect: Deny, Principal: {Service: cloudwatch.amazonaws.com}, Action: sns:Publish, Resource: <that topic>} and GetTopicAttributes reads the statement back. One metric alarm over AWS/RDS DatabaseConnections with Dimensions=[DBInstanceIdentifier=<an identifier describe_db_instances reports DBInstanceNotFound for>], TreatMissingData=breaching, AlarmActions=[allowed topic, denied topic] — no SetAlarmState is used.

Expected: DescribeAlarmHistory(HistoryItemType='Action') yields two items; the allowed topic's item actionState 'Succeeded' (and its notification arrives in the queue), the denied topic's item actionState 'Failed' with an error containing 'not authorized to perform: SNS:Publish' and the denied topic ARN, no publishedMessage, and no notification from the denied topic in the queue.
Actual: the denied topic's item is actionState 'Succeeded' with no error member; the policy Deny has no effect on the publish.

## Steps to Reproduce
1. Boot the emulator and replay the suite; re-run check cloudwatch__rds.rds-alarm-action-blocked-by-topic-policy-denying-cloudwatch.
2. Minimal repro: sns:CreateTopic twice; subscribe one SQS queue to both topics (and grant sns.amazonaws.com sqs:SendMessage on the queue).
3. On the second topic, SetTopicAttributes(AttributeName='Policy') with the default policy plus an explicit Deny of sns:Publish for Principal {Service: cloudwatch.amazonaws.com} on that topic; confirm with GetTopicAttributes.
4. cloudwatch:PutMetricAlarm over Namespace='AWS/RDS', MetricName='DatabaseConnections', Dimensions=[{DBInstanceIdentifier, <nonexistent instance id>}], Period=60, EvaluationPeriods=1, Threshold=1.0, ComparisonOperator='GreaterThanOrEqualToThreshold', TreatMissingData='breaching', AlarmActions=[allowed topic, denied topic].
5. Poll cloudwatch:DescribeAlarmHistory(AlarmName=..., HistoryItemType='Action', AlarmTypes=['MetricAlarm']) until an item exists for each topic ARN, and parse HistoryData. The denied topic's item reports actionState 'Succeeded'; sqs:ReceiveMessage also shows the denied topic's notification delivered.
