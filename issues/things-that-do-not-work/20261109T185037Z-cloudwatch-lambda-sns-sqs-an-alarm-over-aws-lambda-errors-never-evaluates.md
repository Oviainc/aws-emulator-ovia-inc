Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__lambda__sns__sqs.lambda-errors-alarm-sns-sqs-delivery-healthy-twin-ok cloudwatch__lambda__sns__sqs.xsvc-cloudwatch-3-composite-alarm-and-rule-over-lambda-errors-and-sqs-depth-delivers-via-sns-to-sqs
Issue-Key: cloudwatch__lambda__sns__sqs.lambda-errors-alarm-sns-sqs-delivery-healthy-twin-ok|_poll#29|Timeout|time.sleep(10.0)
Issue-Key: cloudwatch__lambda__sns__sqs.xsvc-cloudwatch-3-composite-alarm-and-rule-over-lambda-errors-and-sqs-depth-delivers-via-sns-to-sqs|_poll#31|Timeout|time.sleep(10.0)
Issue-Commit: b4090e4eff86c6645a745aa23fbd4b95671a37a2

# An alarm over AWS/Lambda Errors never evaluates, so the alarm -> SNS -> SQS notification path never completes

> _Note: this issue was identified against the code base at commit `b4090e4eff86c6645a745aa23fbd4b95671a37a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAlarms

## Description
Cause 1 of 1: both checks that build an alarm on a Lambda function's AWS/Lambda Errors metric (one simple, one composite with an SQS depth alarm) run to their 900s ceiling without the alarm ever reaching ALARM and without any notification reaching the SNS-fed queue.

AWS: invoking a function that throws publishes AWS/Lambda Errors datapoints for that FunctionName; an alarm on that metric transitions to ALARM inside its evaluation periods, fires its AlarmActions on the SNS topic, and the subscribed queue receives the notification, while the healthy twin stays OK. Recorded behavior: both legs settle well inside the check's budget on real AWS (the metric leg in ~400s).
Emulator: 'Error: Check timed out after 900.0s' for both — no terminal observation at all. Hypothesis: the AWS/Lambda metric series (Errors/Invocations) is never published, so the alarm stays INSUFFICIENT_DATA and nothing downstream can happen; the suite's sibling cloudwatch__sqs checks show the same total absence of AWS/SQS series, and no alarm transition or notification was observed here either.
Why: alarm-based alerting over Lambda — the most common production guardrail — cannot be exercised: no alarm state ever changes, no action fires, and the client has no way to tell a missing metric from a healthy system.
Fix: AWS/Lambda metric publication (Errors, Invocations, Throttles) from the Lambda invoke path, and the CloudWatch alarm evaluator that consumes it, plus the composite-alarm evaluation that depends on both children.
Traces: cloudwatch__lambda__sns__sqs.lambda-errors-alarm-sns-sqs-delivery-healthy-twin-ok, cloudwatch__lambda__sns__sqs.xsvc-cloudwatch-3-composite-alarm-and-rule-over-lambda-errors-and-sqs-depth-delivers-via-sns-to-sqs.

## Evidence
[mismatch] cloudwatch__lambda__sns__sqs.lambda-errors-alarm-sns-sqs-delivery-healthy-twin-ok
Error: Check timed out after 900.0s

[mismatch] cloudwatch__lambda__sns__sqs.xsvc-cloudwatch-3-composite-alarm-and-rule-over-lambda-errors-and-sqs-depth-delivers-via-sns-to-sqs
Error: Check timed out after 900.0s

Expected: the Errors metric arrives, the alarm goes to ALARM, the SNS action fires and the subscribed queue receives the notification (the healthy twin stays OK).
Actual: no alarm transition and no notification within 900s.

## Steps to Reproduce
1. Create a Lambda function that throws and invoke it repeatedly.
2. PutMetricAlarm on AWS/Lambda Errors for that FunctionName with an SNS topic as AlarmActions; subscribe an SQS queue to the topic.
3. Poll GetMetricStatistics / DescribeAlarms and the queue — the emulator never publishes the metric, never transitions the alarm and never delivers a notification.
