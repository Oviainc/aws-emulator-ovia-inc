Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__dynamodb__lambda.skipped-records-metric-sums-one-per-skipped-insert cloudwatch__dynamodb__lambda.skipped-records-alarm-fires-on-six-skips-in-one-period cloudwatch__dynamodb__lambda.skipped-records-alarm-holds-ok-at-exactly-five-skips cloudwatch__dynamodb__lambda.skipped-records-metric-is-not-published-when-the-role-lacks-put-metric-data
Issue-Key: cloudwatch__dynamodb__lambda.skipped-records-metric-sums-one-per-skipped-insert|_await_invocation#8|Timeout|time.sleep(4)
Issue-Key: cloudwatch__dynamodb__lambda.skipped-records-alarm-fires-on-six-skips-in-one-period|_await_invocation#13|Timeout|time.sleep(4)
Issue-Key: cloudwatch__dynamodb__lambda.skipped-records-alarm-holds-ok-at-exactly-five-skips|_await_invocation#9|Timeout|time.sleep(4)
Issue-Key: cloudwatch__dynamodb__lambda.skipped-records-metric-is-not-published-when-the-role-lacks-put-metric-data|_await_invocation#5|Timeout|time.sleep(4)
Issue-Commit: b4ead8169e48e6047949699a772a41fd75a6bc4d

# Custom CloudWatch metrics published from a Lambda never materialise, so the skipped-records metric and its alarm never settle

> _Note: this issue was identified against the code base at commit `b4ead8169e48e6047949699a772a41fd75a6bc4d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricData

## Description
Cause 1 of 1: a stream-processing Lambda that calls cloudwatch:PutMetricData once per skipped record must make that metric readable through GetMetricStatistics/GetMetricData, and an alarm on it must transition on threshold and hold OK below it.
AWS: recorded behavior only — the recording writes N DynamoDB items, has the ESM-driven handler publish one datapoint per skipped insert, then reads the metric sum back and reads the alarm's StateValue, all inside the suite's 60s budget. The negative twin proves that with no cloudwatch:PutMetricData grant the metric stays absent.
Emulator: all four checks are cut off with "Error: Check timed out after 60.0s". Nothing the scenario polls for — the datapoints, the alarm transition to ALARM, or the alarm holding OK — ever appears, so the poll never terminates.
Why: a client that instruments a stream consumer with a custom metric and alarms on it sees an empty metric and an alarm stuck in its initial state, and any wait-for-datapoint loop hangs instead of failing fast; the "role lacks PutMetricData" negative control is indistinguishable from the granted case.
Fix: the end-to-end path these depend on — the DynamoDB-stream event-source-mapping delivery that runs the handler, the CloudWatch PutMetricData ingestion into metric state, and the alarm evaluator that reads it.
Traces: cloudwatch__dynamodb__lambda.skipped-records-metric-sums-one-per-skipped-insert, .skipped-records-alarm-fires-on-six-skips-in-one-period, .skipped-records-alarm-holds-ok-at-exactly-five-skips, .skipped-records-metric-is-not-published-when-the-role-lacks-put-metric-data.

## Evidence
Expected (AWS): the custom metric's Sum equals the number of skipped inserts; the alarm goes to ALARM at six skips in one period and stays OK at five; with no PutMetricData grant the metric has no datapoints.
Actual (emulator): all four checks end with 'Error: Check timed out after 60.0s' — no datapoints and no alarm state change ever appear.

## Steps to Reproduce
1. Create a DynamoDB table with a stream and an event-source mapping to a Lambda whose role may call cloudwatch:PutMetricData.
2. Write items that the handler skips; the handler publishes one datapoint per skip.
3. Poll GetMetricStatistics for the metric Sum, and DescribeAlarms for an alarm on that metric. Observe neither ever reports anything within 60s.
