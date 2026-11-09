Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__sns.eventbridge-rule-target-lambda-arn-invoke events__lambda__sns.eventbridge-rule-fanout-lambda-sns-simultaneous events__lambda__sns.eventbridge-fanout-lambda-denied-sns-still-delivered events__lambda__sns.eventbridge-fanout-both-targets-denied-no-delivery events__lambda__sns.eventbridge-sns-target-missing-topic-policy-not-published events__lambda__sns.events-rule-notification-lambda-destination-delivery events__lambda__sns.sns-lambda-subscription-notification-delivery events__lambda__sns.sns-lambda-subscription-missing-permission-not-invoked
Issue-Key: events__lambda__sns.eventbridge-rule-target-lambda-arn-invoke|_wait_for_log_marker#11|Timeout|time.sleep(interval_s)
Issue-Key: events__lambda__sns.eventbridge-rule-fanout-lambda-sns-simultaneous|_wait_for_log_marker#14|Timeout|time.sleep(interval_s)
Issue-Key: events__lambda__sns.eventbridge-fanout-lambda-denied-sns-still-delivered|_wait_for_log_marker#23|Timeout|time.sleep(interval_s)
Issue-Key: events__lambda__sns.eventbridge-fanout-both-targets-denied-no-delivery|_wait_for_log_marker#8|Timeout|time.sleep(interval_s)
Issue-Key: events__lambda__sns.eventbridge-sns-target-missing-topic-policy-not-published|_wait_for_matching_message#15|Timeout|resp = sqs.receive_message(
Issue-Key: events__lambda__sns.events-rule-notification-lambda-destination-delivery|_find_marker_in_logs#18|Timeout|time.sleep(interval_s)
Issue-Key: events__lambda__sns.sns-lambda-subscription-notification-delivery|_find_marker_in_logs#14|Timeout|time.sleep(interval_s)
Issue-Key: events__lambda__sns.sns-lambda-subscription-missing-permission-not-invoked|_find_marker_in_logs#19|Timeout|time.sleep(interval_s)
Issue-Commit: b4ead8169e48e6047949699a772a41fd75a6bc4d

# EventBridge rules and SNS subscriptions never deliver to their Lambda or SNS targets

> _Note: this issue was identified against the code base at commit `b4ead8169e48e6047949699a772a41fd75a6bc4d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
Cause 1 of 1: an EventBridge rule whose targets are a Lambda and an SNS topic must invoke both on a matching event (and only the permitted one when a grant is missing), and an SNS topic with a Lambda subscription must invoke the function on Publish.
AWS: recorded behavior only — each of these eight recordings wires the rule/subscription, grants lambda:InvokeFunction to events.amazonaws.com or sns.amazonaws.com (and the topic policy for the SNS target), drives PutEvents or Publish, and then reads a per-run marker out of the target function's /aws/lambda/<name> log group or out of the topic's downstream queue; on AWS every one lands inside the suite's 60s budget, and each negative twin observes the absence only after its positive control has fired.
Emulator: all eight checks are cut off with "Error: Check timed out after 60.0s". PutRule/PutTargets/Subscribe/AddPermission/PutEvents/Publish are all accepted, but no target is ever invoked and no marker ever appears, so every poll runs out the budget.
Why: EventBridge fan-out and SNS-to-Lambda delivery are entirely inert while every control-plane call reports success; because the "denied" twins also only time out, a client cannot distinguish a missing resource policy or topic policy from a dead delivery path.
Fix: the EventBridge PutEvents dispatcher (rule matching, per-target delivery, independent per-target failure) and the SNS Publish delivery path to Lambda subscriptions, together with the resource-policy / topic-policy evaluation on those deliveries.
Traces: events__lambda__sns.eventbridge-rule-target-lambda-arn-invoke, .eventbridge-rule-fanout-lambda-sns-simultaneous, .eventbridge-fanout-lambda-denied-sns-still-delivered, .eventbridge-fanout-both-targets-denied-no-delivery, .eventbridge-sns-target-missing-topic-policy-not-published, .events-rule-notification-lambda-destination-delivery, .sns-lambda-subscription-notification-delivery, .sns-lambda-subscription-missing-permission-not-invoked.

## Evidence
Expected (AWS): the matching event invokes the Lambda target and publishes to the SNS target; a Publish on a topic with a Lambda subscription invokes the function; the denied legs deliver only to the permitted target.
Actual (emulator): all eight checks end with 'Error: Check timed out after 60.0s' — no target invocation or publication is ever observed.

## Steps to Reproduce
1. PutRule with an event pattern; PutTargets naming a Lambda and an SNS topic; AddPermission for events.amazonaws.com and set the topic policy.
2. PutEvents with a matching event and poll the function's log group and the topic's downstream queue for a marker. Observe neither appears.
3. Subscribe a Lambda to an SNS topic, AddPermission for sns.amazonaws.com, Publish, and poll the function's logs. Observe no invocation within 60s.
