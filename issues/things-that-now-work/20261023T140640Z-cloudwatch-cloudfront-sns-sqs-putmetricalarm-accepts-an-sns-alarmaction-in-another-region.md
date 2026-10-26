Issue-Severity: MEDIUM
Issue-Commit: a59989a9f63c403b66485a4e93f54ab6df22c2a6

# PutMetricAlarm accepts an SNS AlarmAction in another region instead of refusing it with ValidationError

> _Note: this issue was identified against the code base at commit `a59989a9f63c403b66485a4e93f54ab6df22c2a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricAlarm

## Description
Cause 1 of 1: CloudWatch does not validate that an alarm's actions are co-regional with the alarm, so a cross-region SNS topic ARN is accepted.
AWS: recorded behavior only — PutMetricAlarm whose AlarmActions name an SNS topic in a region other than the alarm's own region is refused with 400 `ValidationError`, and the message is built from the request: "Invalid region <action region> specified. Only <alarm region> is supported." (here the alarm lives in us-east-1 because AWS/CloudFront metrics are published there, and the topic is in us-west-2). The refusal is pre-effect: no alarm is created, and on an update the existing alarm's actions are unchanged.
Emulator: both the create and the update are accepted (no error raised), so the alarm is stored with the foreign-region action.
Why: a client gets an alarm that can never notify — the action target is in a region CloudWatch will not publish to — and the misconfiguration AWS reports synchronously is reported never; the pre-effect guarantee (nothing created on a refused call) is also lost.
Fix: AlarmActions/OKActions/InsufficientDataActions region validation in the PutMetricAlarm handler of the cloudwatch crate, emitting the ValidationError envelope and leaving alarm state untouched.
Traces: cloudwatch__cloudfront__sns__sqs.cloudfront-4xx-alarm-cross-region-sns-action-rejected.

## Evidence
"AssertionError: the call was ACCEPTED but real AWS rejects it" — raised by the check's _error() helper around cw.put_metric_alarm(AlarmActions=[foreign_topic], **alarm) for an AWS/CloudFront 4xxErrorRate alarm created in us-east-1 with an SNS topic whose ARN contains ':sns:us-west-2:'. The check's expected refusal is code 'ValidationError' with message "Invalid region us-west-2 specified. Only us-east-1 is supported.", followed by describe_alarms returning [] (nothing created) and the same assertion for an update that adds the foreign action beside a valid one.

## Steps to Reproduce
1. Create an SNS topic in us-west-2 and another in us-east-1.
2. In us-east-1, PutMetricAlarm on Namespace='AWS/CloudFront', MetricName='4xxErrorRate' with AlarmActions=[<us-west-2 topic ARN>] -> expect 400 ValidationError 'Invalid region us-west-2 specified. Only us-east-1 is supported.' and DescribeAlarms to show no such alarm; observe the call succeed.
3. Create the alarm with the co-regional action, then PutMetricAlarm again with AlarmActions=[<us-east-1 topic>, <us-west-2 topic>] -> expect the same refusal with the stored actions unchanged; observe acceptance.
