Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__sns__sqs.alarm-composite-and-actions-each-action-list-fires-only-on-its-own-transition cloudwatch__sns__sqs.deep-cloudwatch-1-cw-warmup-withholds-alarm-and-notification-until-expiry cloudwatch__sns__sqs.notifications-filtered-out-and-delivered-are-two-sided-for-complementary-subscriptions
Issue-Key: cloudwatch__sns__sqs.alarm-composite-and-actions-each-action-list-fires-only-on-its-own-transition|run#24|AssertionError|assert trig.get(field) == want, f"{old}->{new} Trigger.{field} = {trig.get(field)!r}, expected {want!r}"
Issue-Key: cloudwatch__sns__sqs.deep-cloudwatch-1-cw-warmup-withholds-alarm-and-notification-until-expiry|run#36|AssertionError|assert gated0.get("EvaluationState") == _IN_WARM_UP, (
Issue-Key: cloudwatch__sns__sqs.notifications-filtered-out-and-delivered-are-two-sided-for-complementary-subscriptions|_poll_until#33|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Commit: c33038d6637516191c9271b81207503148b3a194

# cloudwatch__sns__sqs: 3 net-new causes — the alarm notification's Trigger.Statistic is title-cased, EvaluationState is null instead of IN_WARM_UP during warm-up, and AWS/SNS NumberOfMessagesPublished is never published

> _Note: this issue was identified against the code base at commit `c33038d6637516191c9271b81207503148b3a194`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAlarms

## Description
Cause 1 of 3: the alarm-notification payload renders Trigger.Statistic in the API's title case instead of the upper-case token AWS puts on the wire.
AWS: in the JSON message CloudWatch publishes to SNS for a state change, Trigger.Statistic is the UPPER-CASE statistic token — 'MAXIMUM', 'AVERAGE', 'SUM' — alongside 'StatisticType': 'Statistic'; the title-cased form ('Maximum') is the PutMetricAlarm/DescribeAlarms API spelling (Smithy cloudwatch 2010-08-01: Statistic enum SampleCount/Average/Sum/Minimum/Maximum), not the notification spelling. Recorded behavior for the notification envelope.
Emulator: the notification for an INSUFFICIENT_DATA->OK transition carries Trigger.Statistic 'Maximum'.
Why: consumers parse that payload (Lambda/SQS subscribers, chatops bridges, incident tooling) and match on the documented token. A title-cased value breaks exact-match routing and any code that treats the field as an enum.
Fix: the alarm notification serializer — emit the statistic in its notification (upper-case) form, keeping the API shape's title case on DescribeAlarms.
Failing check: cloudwatch__sns__sqs.alarm-composite-and-actions-each-action-list-fires-only-on-its-own-transition.

Cause 2 of 3: an alarm inside its warm-up window reports no EvaluationState.
AWS: an alarm created with a WarmUpConfiguration reports EvaluationState 'IN_WARM_UP' while the warm-up is in force (a value the Smithy enum does not declare — AWS serves it anyway) and withholds both the ALARM transition and its notification until the warm-up expires. Smithy (cloudwatch 2010-08-01): PutMetricAlarm$WarmUpConfiguration exists; the in-warm-up state string is recorded behavior.
Emulator: EvaluationState is None at creation for the gated arm, so a caller cannot tell a warming-up alarm from one that is simply being evaluated.
Why: warm-up exists so a newly created alarm does not page on startup noise. Without the state a customer cannot tell why an alarm is not transitioning, and tooling that waits for warm-up to end has nothing to poll.
Fix: the alarm state/serializer component — populate EvaluationState with 'IN_WARM_UP' while a WarmUpConfiguration is active (do not clamp the value to the modelled enum).
Failing check: cloudwatch__sns__sqs.deep-cloudwatch-1-cw-warmup-withholds-alarm-and-notification-until-expiry.

Cause 3 of 3: AWS/SNS NumberOfMessagesPublished is never published, so the anchor that makes filtered-out and delivered counts comparable never appears.
AWS: each successful Publish to a topic increments NumberOfMessagesPublished under {TopicName}; the per-subscription NumberOfNotificationsDelivered and NumberOfNotificationsFilteredOut then sum to it for complementary filter policies.
Emulator: NumberOfMessagesPublished for the topic stays 0 for 300 s after two publishes, so the two-sided comparison is unreachable.
Why: SNS throughput is unobservable and the delivered/filtered split cannot be reconciled against what was published, so a customer cannot prove messages were not silently dropped.
Fix: the SNS Publish path's metric emission — increment AWS/SNS NumberOfMessagesPublished (and the per-subscription delivered/filtered counters) in the CloudWatch metric store.
Failing check: cloudwatch__sns__sqs.notifications-filtered-out-and-delivered-are-two-sided-for-complementary-subscriptions.

## Evidence
Cause 1 — Expected Trigger.Statistic 'MAXIMUM' in the SNS notification. Actual: 'AssertionError: INSUFFICIENT_DATA->OK Trigger.Statistic = \'Maximum\', expected \'MAXIMUM\''.
Cause 2 — Expected EvaluationState 'IN_WARM_UP' for an alarm inside its warm-up. Actual: 'AssertionError: the gated arm reports EvaluationState None at creation; real AWS reports \'IN_WARM_UP\' for an alarm inside its warm-up (a value the Smithy enum does not declare)'.
Cause 3 — Expected AWS/SNS NumberOfMessagesPublished for the topic to reach 2. Actual: 'AssertionError: AWS/SNS NumberOfMessagesPublished for TopicName=<topic> reached 2 — the anchor that makes the two metrics below comparable not met within 300s. Last result: 0'.

## Steps to Reproduce
Cause 1: PutMetricAlarm(Statistic='Maximum') with an SNS AlarmAction/OKAction subscribed by SQS; drive a transition; read the received message's Message.Trigger.Statistic.
Cause 2: PutMetricAlarm with a WarmUpConfiguration and read DescribeAlarms' EvaluationState immediately after creation.
Cause 3: create a topic with two complementary filter-policy subscriptions, Publish twice, then poll AWS/SNS NumberOfMessagesPublished for {TopicName}.
