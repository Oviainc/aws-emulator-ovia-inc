Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__dynamodb.obs-dynamodb-2-ccf-alarm-notifies-sqs-via-sns
Issue-Key: cloudwatch__dynamodb.obs-dynamodb-2-ccf-alarm-notifies-sqs-via-sns|_await_state#33|AssertionError|raise AssertionError(
Issue-Commit: 545841f07feb57b92e392d55db412e521f655a7b

# CloudWatch alarm on AWS/DynamoDB ConditionalCheckFailedRequests never leaves OK: no datapoints are ever evaluated, so the SNS->SQS notification never fires

> _Note: this issue was identified against the code base at commit `545841f07feb57b92e392d55db412e521f655a7b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricAlarm

## Description
Cause 1 of 1: the metric->alarm->action loop never closes for DynamoDB-emitted metrics.

AWS: `PutMetricAlarm` on `AWS/DynamoDB` `ConditionalCheckFailedRequests` with a TableName dimension evaluates each period against the datapoints DynamoDB publishes for that table. Once enough refused conditional writes are issued, the alarm transitions to `ALARM` with a StateReason of the form `Threshold Crossed: <n> out of the last <n> datapoints [<value> (<timestamp>)] was greater than the threshold (<threshold>) (minimum <n> datapoint for OK -> ALARM transition).`, and its AlarmActions are executed — publishing to the SNS topic, which fans out to the subscribed SQS queue. Smithy (cloudwatch): PutMetricAlarm input AlarmActions/MetricName/Namespace/Dimensions; DescribeAlarms output `MetricAlarm.StateValue` enum OK|ALARM|INSUFFICIENT_DATA with `StateReason`.

Emulator: the alarm stays `OK` for the whole 420s window with StateReason `Threshold Crossed: no datapoints were received for 1 period and 1 missing datapoint was treated as [NonBreaching].` — no `ConditionalCheckFailedRequests` datapoint is ever delivered to the alarm's evaluator even though the check issued the refusals, so the alarm never transitions and the SNS publish and SQS delivery never happen.

Why: alarm-driven automation is the whole point of the metric plane. A client that builds an alarm -> SNS -> SQS/Lambda remediation path observes a permanently healthy system while the condition it alarms on is occurring; nothing is notified and nothing remediates.

Fix: publication of AWS/DynamoDB metrics from the DynamoDB request path into the CloudWatch metric store, and the CloudWatch alarm evaluator/action dispatcher that must read them and execute AlarmActions on transition.

Traces: cloudwatch__dynamodb.obs-dynamodb-2-ccf-alarm-notifies-sqs-via-sns. Related already-filed issues cover the metric-publication side for other checks (20261102T120041Z-cloudwatch-dynamodb-dynamodb-publishes-no-aws-dynamodb-cloudwatch.md, 20261029T180026Z-cloudwatch-dynamodb-no-aws-dynamodb-cloudwatch-metrics-are-published-and.md); neither carries this check's replay key nor the alarm-transition/action half of the loop.

## Evidence
Expected: after N refused conditional writes, DescribeAlarms shows StateValue 'ALARM' with a "Threshold Crossed" StateReason naming the breaching datapoints, and the SNS->SQS subscriber receives the notification.
Actual: AssertionError: alarm '<alarm-name>' never reached 'ALARM' within 420s; last state 'OK' reason 'Threshold Crossed: no datapoints were received for 1 period and 1 missing datapoint was treated as [NonBreaching].' (the alarm name is a per-run value).

## Steps to Reproduce
1. Create a table, an SNS topic and an SQS queue subscribed to the topic.
2. cloudwatch:PutMetricAlarm on AWS/DynamoDB ConditionalCheckFailedRequests, Dimensions TableName=<table>, with the topic as AlarmActions.
3. Issue a batch of PutItem calls with a condition that fails.
4. Poll DescribeAlarms and the queue: the alarm stays OK with "no datapoints were received" and nothing is delivered.
