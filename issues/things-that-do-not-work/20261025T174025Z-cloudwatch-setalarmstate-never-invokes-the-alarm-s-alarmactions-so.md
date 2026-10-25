Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.alarm-actions-disabled-suppresses-state-triggered-action
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch.alarm-actions-disabled-suppresses-state-triggered-action|_drain_queue#27|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Commit: b5c349f78c27b93d7bc476c26ede97815cd374b3

# SetAlarmState never invokes the alarm's AlarmActions, so a forced OK->ALARM delivers no SNS notification (1 cause)

> _Note: this issue was identified against the code base at commit `b5c349f78c27b93d7bc476c26ede97815cd374b3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetAlarmState

## Description
Cause 1 of 1: a state change forced with SetAlarmState does not run the alarm's configured actions, so nothing is published to the alarm's SNS AlarmActions target and the check that measures ActionsEnabled suppression can never reach the flag it is about.

AWS: Smithy: SetAlarmState(AlarmName required @length 1-255, StateValue enum, StateReason required @length 0-1023, StateReasonData @length 0-4000; errors InvalidFormatFault 400 / ResourceNotFound 404). Its modelled documentation is explicit about the effect: "When the updated state differs from the previous value, the action configured for the appropriate state is invoked. For example, if your alarm is configured to send an Amazon SNS message when an alarm is triggered, temporarily changing the alarm state to ALARM sends an SNS message." So for a metric alarm whose AlarmActions names an SNS topic, a forced transition into ALARM publishes one message to that topic; an SQS subscriber receives an SNS envelope whose TopicArn is the alarm's action target and whose Message is the alarm notification JSON carrying AlarmName (the name from the PutMetricAlarm request), NewStateValue "ALARM", OldStateValue (the state the alarm left) and a Trigger naming the alarm's Namespace/MetricName. Smithy also models DisableAlarmActions / EnableAlarmActions (AlarmNames required, max 100, no modelled errors), which flip ActionsEnabled in DescribeAlarms and gate exactly that delivery, and DescribeAlarmHistory, where the invocation is recorded as an Action history item.

Emulator: the check never gets past its leg 0 positive control, which runs before ActionsEnabled is touched at all: PutMetricAlarm with AlarmActions=[topic] is accepted, SetAlarmState(StateValue="ALARM") returns 200, and then no message ever arrives on the SQS queue subscribed to that topic. The check sits in ReceiveMessage until the harness kills it, and the replay verdict is the bare "Check timed out after 60.0s" with no leg completed. No SNS publish is attributable to the alarm, so there is no Action delivery and consequently nothing for DisableAlarmActions to suppress.

Why: SetAlarmState is the documented way to test an alarm's wiring before trusting it in production. A client that forces ALARM and waits for the notification waits forever, and concludes its topic policy, subscription or action list is broken when the emulator simply never dispatched the action. Every downstream contract built on that dispatch - the notification payload's AlarmName/NewStateValue/OldStateValue/Trigger fields, the Action item in DescribeAlarmHistory, and the whole ActionsEnabled suppression differential - is unreachable.

Fix: the SetAlarmState handler in the cloudwatch service, and the alarm-action dispatcher it must call: a state change (forced or evaluated) whose new state differs from the old one has to run the matching action list (AlarmActions / OKActions / InsufficientDataActions), publish to SNS ARNs in it, and record an Action item in alarm history. The handler must also honour ActionsEnabled as the gate on that dispatch rather than as a stored-only field.

Related already-open issues, named so this is not closed as a duplicate of either: 20261024T030130Z-cloudwatch-rds-cloudwatch-metric-alarms-never-evaluate-or-fire-their.md and 20261025T020208Z-cloudwatch-alarms-never-evaluate-published-metric-data-no.md both describe alarms that never EVALUATE published data. This cause is on the path that needs no evaluator at all - the transition is supplied by SetAlarmState - so a fix confined to the evaluator leaves this check failing. Separately, 20261025T020208Z-cloudwatch-disablealarmactions-stub.md records that DisableAlarmActions is still an unimplemented scaffold; that is the NEXT blocker for legs 1 and 2 of this same check and is already filed, so it is not re-filed here.

Traces: cloudwatch.alarm-actions-disabled-suppresses-state-triggered-action (mismatch, "Check timed out after 60.0s").

## Evidence
Expected (AWS, per the modelled SetAlarmState documentation): PutMetricAlarm(AlarmName=<name>, Namespace=<ns>, MetricName=Errors, Statistic=Average, Period=60, EvaluationPeriods=1, Threshold=1.0, ComparisonOperator=GreaterThanThreshold, AlarmActions=[<topic arn>], TreatMissingData=missing) then SetAlarmState(AlarmName=<name>, StateValue=ALARM, StateReason=<any>) publishes one message to <topic arn>; the SQS queue subscribed to that topic receives an envelope with TopicArn=<topic arn> whose Message parses to JSON with AlarmName=<name> and NewStateValue="ALARM".

Actual (emulator): SetAlarmState returns 200 and no message is ever delivered to the subscribed queue. The check's baseline wait (90 s budget) never satisfies expected_count=1 and the replay ends as "Check timed out after 60.0s" at the harness cap, before DisableAlarmActions is ever called. No Action history item exists for the alarm either.

## Steps to Reproduce
1. sqs.create_queue; sqs.get_queue_attributes(QueueArn); sns.create_topic.
2. sqs.set_queue_attributes with a Policy allowing sns.amazonaws.com sqs:SendMessage on the queue ARN, conditioned on aws:SourceArn = the topic ARN.
3. sns.subscribe(TopicArn=<topic>, Protocol="sqs", Endpoint=<queue arn>).
4. cloudwatch.put_metric_alarm(AlarmName=<name>, Namespace=<unique ns>, MetricName="Errors", Statistic="Average", Period=60, EvaluationPeriods=1, Threshold=1.0, ComparisonOperator="GreaterThanThreshold", AlarmActions=[<topic>], TreatMissingData="missing").
5. cloudwatch.set_alarm_state(AlarmName=<name>, StateValue="ALARM", StateReason="baseline").
6. Poll sqs.receive_message on the queue. Expected: one message within seconds whose Message JSON has AlarmName=<name>, NewStateValue="ALARM". Observed: no message; the poll runs to its deadline.
7. cloudwatch.describe_alarm_history(AlarmName=<name>, HistoryItemType="Action"). Expected: one Action item for the dispatch. Observed: none.
