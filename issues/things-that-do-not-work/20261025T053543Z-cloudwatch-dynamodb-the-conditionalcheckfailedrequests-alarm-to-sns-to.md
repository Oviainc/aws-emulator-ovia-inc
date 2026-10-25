Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__dynamodb.obs-dynamodb-2-ccf-alarm-notifies-sqs-via-sns
Issue-Key: cloudwatch__dynamodb.obs-dynamodb-2-ccf-alarm-notifies-sqs-via-sns|_await_state#33|Timeout|time.sleep(interval_s)
Issue-Commit: 35f0669fbed72946ad7349f4657cbc8fac9f7c2b

# cloudwatch__dynamodb: the ConditionalCheckFailedRequests alarm-to-SNS-to-SQS notification check never completes — it hangs to the 60 s harness cap (1 cause)

> _Note: this issue was identified against the code base at commit `35f0669fbed72946ad7349f4657cbc8fac9f7c2b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricAlarm

## Description
=== Cause 1 of 1: an alarm over AWS/DynamoDB ConditionalCheckFailedRequests never reaches a notified state ===
AWS: recorded behavior only. Measured on real AWS (us-west-2): 12 refused conditional PutItem calls produce ConditionalCheckFailedRequests Sum=12 in the AWS/DynamoDB namespace for the table; an alarm with Sum >= 10 over one 60 s period settles to OK about 25 s after PutMetricAlarm (TreatMissingData=notBreaching, no datapoints), reaches ALARM about 121 s after the refusal burst, and the SNS -> SQS notification lands within a few seconds of the state change. Smithy: MetricAlarm.TreatMissingData documents the notBreaching arm; the metric itself is a DynamoDB-published series, not a modeled member.
Emulator: the check reaches none of those waypoints inside the suite's 60 s cap and is cut with 'Check timed out after 60.0s' - no assertion is reached, so the first missing waypoint is not isolated by the replay. Its own budgets are far larger (240 s to OK, 420 s to ALARM, 180 s to delivery), and sibling checks in this suite that only need alarm evaluation do finish inside the cap, so the pipeline is stalled rather than merely slow. Hypothesis: DynamoDB publishes no ConditionalCheckFailedRequests datapoints, so the alarm has nothing to grade - the same missing-publication family as the open issue 20261025T020208Z-cloudwatch-dynamodb-dynamodb-transactwriteitems-consumedcapacity-omits-writecapacityunits-and-no-aws.md, which reports that no AWS/DynamoDB table metrics are published at all; that issue does not carry this check's replay key, and this check additionally grades the alarm-to-SNS-to-SQS leg on top of the metric.
Why: the canonical DynamoDB alerting recipe - alarm on conditional-write failures and page through SNS - cannot be exercised or validated at all against the emulator; a client sees an alarm that stays in its initial state forever and a queue that never receives a notification.
Fix: DynamoDB's CloudWatch metric publication for conditional-write refusals (the PutItem/condition-failure path's metric emission) and, once datapoints exist, the alarm evaluator's action dispatch to SNS.
Traces: cloudwatch__dynamodb.obs-dynamodb-2-ccf-alarm-notifies-sqs-via-sns

## Evidence
Replayed behavioral suite (simparity-behavioral-ga-prod-v1).

[mismatch] cloudwatch__dynamodb.obs-dynamodb-2-ccf-alarm-notifies-sqs-via-sns
Error: Check timed out after 60.0s

Check's own measured budgets (from its source): _OK_TIMEOUT_S = 240, _ALARM_TIMEOUT_S = 420, _DELIVERY_TIMEOUT_S = 180, with the comment "MEASURED us-west-2 2026-10-01: OK ~25 s after PutMetricAlarm (notBreaching, no datapoints), ALARM ~121 s after the 12-refusal burst, SNS->SQS delivery within a few seconds of the change." _REFUSALS = 12, _THRESHOLD = 10.0, _PERIOD_S = 60.

## Steps to Reproduce
1. Clone the repo, boot the emulator, replay the behavioral suite.
2. Create a DynamoDB table and issue 12 PutItem calls with a ConditionExpression that fails (each must answer ConditionalCheckFailedException).
3. GetMetricStatistics for Namespace=AWS/DynamoDB, MetricName=ConditionalCheckFailedRequests, TableName=<table>, Statistic=Sum, Period=60 - no datapoints appear.
4. PutMetricAlarm Sum >= 10 over that metric with an SNS AlarmAction fanning out to an SQS queue, then poll DescribeAlarms and ReceiveMessage.
5. Re-replay cloudwatch__dynamodb.obs-dynamodb-2-ccf-alarm-notifies-sqs-via-sns.
