Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__sns.filtered-out-and-failed-notification-metrics-separate-a-filter-from-a-failure cloudwatch__sns.ok-action-missing-sns-permission-rejected
Issue-Key: cloudwatch__sns.filtered-out-and-failed-notification-metrics-separate-a-filter-from-a-failure|_poll_until_stable#24|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__sns.ok-action-missing-sns-permission-rejected|run#25|AssertionError|assert states == ["Failed"] * len(states), (
Issue-Commit: c33038d6637516191c9271b81207503148b3a194

# cloudwatch__sns: 2 net-new causes — the AWS/SNS notification counters (NumberOfNotificationsFilteredOut / FailedNotifications) are never published, and an alarm action on a topic the alarm may not publish to is recorded as Succeeded

> _Note: this issue was identified against the code base at commit `c33038d6637516191c9271b81207503148b3a194`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricAlarm

## Description
Cause 1 of 2: the AWS/SNS per-topic notification counters that separate a filter decision from a delivery failure are never published, so the check that distinguishes them never has data and burns its 900 s budget.
AWS: a publish to a topic increments NumberOfMessagesPublished under {TopicName}; a subscription whose FilterPolicy rejects the message increments NumberOfNotificationsFilteredOut, while a subscription whose delivery fails increments NumberOfNotificationsFailed — the two are different series, so a customer can tell 'the filter dropped it' from 'the endpoint rejected it'. Recorded behavior only for the series content (these metric names are documented, not Smithy shapes).
Emulator: no AWS/SNS counter ever appears for the topic; the check reaches no verdict and is killed by the harness ceiling ('Error: Check timed out after 900.0s').
Why: with the namespace empty a customer cannot see whether messages were filtered or failed. Filter policies look identical to broken endpoints, and no SNS delivery-health alarm can fire.
Fix: the SNS publish/delivery path's metric emission — increment the AWS/SNS per-topic counters (published, filtered-out, failed) in the CloudWatch metric store as each message is evaluated and delivered.
Failing check: cloudwatch__sns.filtered-out-and-failed-notification-metrics-separate-a-filter-from-a-failure.

Cause 2 of 2: an alarm action targeting an SNS topic whose policy does not permit cloudwatch to publish is recorded as having succeeded.
AWS: when an alarm executes an OKAction/AlarmAction against a topic it is not authorized to publish to, the action FAILS: the alarm-history Action item carries actionState 'Failed' with the authorization error in its data, and no message is published. Recorded behavior only for the history record's shape (the Action item's historyData is not a Smithy shape); Smithy (cloudwatch 2010-08-01) supplies PutMetricAlarm$OKActions and DescribeAlarmHistory as the contract.
Emulator: the history item reports actionState 'Succeeded' with error null and a publishedMessage body, i.e. the dispatcher never consults the topic policy and claims success.
Why: the alarm's own audit trail lies. A customer whose topic policy is wrong believes notifications are going out, and the mis-configuration — the single most common reason alarm notifications are not received — is unreportable.
Fix: the alarm action dispatcher must authorize the publish against the target topic's resource policy and record actionState 'Failed' with the authorization error when it is denied.
Failing check: cloudwatch__sns.ok-action-missing-sns-permission-rejected.

## Evidence
Cause 1 — Expected: AWS/SNS NumberOfMessagesPublished, NumberOfNotificationsFilteredOut and NumberOfNotificationsFailed datapoints under {TopicName} separating the filtered subscription from the failing one. Actual: no datapoint read at any point; 'Error: Check timed out after 900.0s'.
Cause 2 — Expected: the Action history item for the OK action on the unpermissioned topic has actionState 'Failed' and names the authorization error. Actual: 'AssertionError: the OK action on the unpermissioned topic must be recorded as Failed, got actionState [\'Succeeded\'] with data [\'{"version":"1.0","actionState":"Succeeded","notificationResource":"arn:aws:sns:...","stateValue":"OK","error":null,"publishedMessage":"{...}"}\']' (ARNs, names and timestamps are per-run; actionState and error are the divergence).

## Steps to Reproduce
Cause 1: create a topic with two subscriptions — one with a FilterPolicy that rejects the message, one whose endpoint fails — publish, then poll AWS/SNS NumberOfMessagesPublished / NumberOfNotificationsFilteredOut / NumberOfNotificationsFailed for {TopicName}.
Cause 2: create a topic whose policy does NOT allow cloudwatch.amazonaws.com:Publish; PutMetricAlarm with OKActions=[that topic]; drive the alarm to OK with SetAlarmState; read DescribeAlarmHistory(HistoryItemType='Action') and inspect actionState.
