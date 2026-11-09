Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__lambda__sns__sqs.lambda-errors-alarm-sns-sqs-delivery-healthy-twin-ok cloudwatch__lambda__sns__sqs.xsvc-cloudwatch-3-composite-alarm-and-rule-over-lambda-errors-and-sqs-depth-delivers-via-sns-to-sqs
Issue-Key: cloudwatch__lambda__sns__sqs.lambda-errors-alarm-sns-sqs-delivery-healthy-twin-ok|_poll#29|Timeout|time.sleep(10.0)
Issue-Key: cloudwatch__lambda__sns__sqs.xsvc-cloudwatch-3-composite-alarm-and-rule-over-lambda-errors-and-sqs-depth-delivers-via-sns-to-sqs|_poll#31|Timeout|last = fetch()
Issue-Commit: c33038d6637516191c9271b81207503148b3a194

# cloudwatch__lambda__sns__sqs: an alarm over AWS/Lambda Errors never fires and its SNS->SQS delivery never happens, so both the single-alarm and composite-alarm chains run out their budget

> _Note: this issue was identified against the code base at commit `c33038d6637516191c9271b81207503148b3a194`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricAlarm

## Description
Cause 1 of 1: a metric alarm over AWS/Lambda Errors (alone, and as a child of a composite alarm combined with an SQS queue-depth alarm) never transitions, so no SNS publish and no SQS delivery is ever observed and both checks are killed by the 900 s harness ceiling.

AWS: invoking a failing function increments AWS/Lambda Errors under {FunctionName}; an alarm over it reaches ALARM, executes its SNS AlarmAction, and the subscribed SQS queue receives the notification envelope (Version/AlarmName/NewStateValue/Trigger...), while the healthy twin stays OK. A composite alarm whose AlarmRule ORs that child with an ApproximateNumberOfMessagesVisible alarm transitions when either child does and publishes through its own action. Recorded behavior for the Lambda/SQS series content; Smithy (cloudwatch 2010-08-01) supplies PutMetricAlarm/PutCompositeAlarm and the action contract.

Emulator: neither check reaches a verdict inside its budget — no alarm transition and no queue delivery is observed at any point ('Error: Check timed out after 900.0s' for both). The underlying source series are the AWS/Lambda Errors family and the AWS/SQS depth family, which this sweep also reports as never published (see the cloudwatch__lambda and cloudwatch__sqs issues filed alongside this one).

Why: the canonical 'alert me when my function errors' and 'alert me when either my function errors or my queue backs up' pipelines produce nothing. A customer gets silence from a system that looks configured, and composite alarms over cross-service children are unusable.

Fix: the alarm evaluator and its action dispatcher, together with the Lambda/SQS metric emission that feeds them — the alarm must evaluate published datapoints for these namespaces and execute its SNS action so the subscription delivers to SQS.

Traces: cloudwatch__lambda__sns__sqs.lambda-errors-alarm-sns-sqs-delivery-healthy-twin-ok, cloudwatch__lambda__sns__sqs.xsvc-cloudwatch-3-composite-alarm-and-rule-over-lambda-errors-and-sqs-depth-delivers-via-sns-to-sqs.

## Evidence
Expected: the errors alarm reaches ALARM, publishes to SNS, and the subscribed SQS queue receives the alarm notification; the healthy twin stays OK. For the composite: the composite transitions on its child and delivers through its own SNS action.
Actual: no transition and no delivery within the run — 'Error: Check timed out after 900.0s' for both checks.

## Steps to Reproduce
1. Create a failing Lambda function and a healthy twin.
2. Create an SNS topic subscribed by an SQS queue.
3. PutMetricAlarm over AWS/Lambda Errors for each function with AlarmActions=[topic]; and PutCompositeAlarm whose AlarmRule ORs the errors alarm with an AWS/SQS ApproximateNumberOfMessagesVisible alarm.
4. Invoke the failing function repeatedly and send messages to the queue.
5. Poll DescribeAlarms and ReceiveMessage: no alarm leaves its initial state and nothing is delivered.
