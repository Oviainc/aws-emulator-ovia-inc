Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__logs__sns__sqs.metric-filter-match-drives-alarm-and-notifies cloudwatch__logs__sns__sqs.metric-filter-non-matching-pattern-no-datapoint-no-alarm
Issue-Key: cloudwatch__logs__sns__sqs.metric-filter-match-drives-alarm-and-notifies|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__logs__sns__sqs.metric-filter-non-matching-pattern-no-datapoint-no-alarm|_assert_stays#17|Timeout|time.sleep(interval_s)
Issue-Commit: ec258a82c643322f3f1b10a7ea424abc4c176038

# Metric-filter-driven alarm chain never settles: neither the matching nor the non-matching log pattern produces its observable datapoint/alarm outcome

> _Note: this issue was identified against the code base at commit `ec258a82c643322f3f1b10a7ea424abc4c176038`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricFilter

## Description
This issue carries 1 net-new root cause (two checks, the positive and its negative twin, ground it).

Cause 1 of 1: a CloudWatch Logs metric filter does not drive the end-to-end logs → metric → alarm → SNS → SQS chain.
AWS: recorded behavior only. A log event matching a metric filter's pattern increments the filter's metricTransformation metric; an alarm on that metric then transitions to ALARM and dispatches its SNS action, whose subscribed SQS queue receives the alarm notification. Its twin, a log event NOT matching the pattern, produces NO datapoint and the alarm stays out of ALARM with no notification — the negative is asserted over a window derived from the positive's measured latency, so it only means something once the positive chain works.
Emulator: both checks exhaust their 60s budget. The positive chain never produces the datapoint/alarm transition/notification, so neither it nor its negative twin can be evaluated.
Why: log-pattern alerting — the standard way errors in application logs become pages — produces nothing end to end, and the emulator cannot distinguish "the filter matched and alerted" from "nothing happened".
Fix: the metric-filter evaluation on the Logs ingestion path (publishing the transformation metric into the CloudWatch metric store) and the alarm evaluation/action dispatch that consumes it.
Traces: cloudwatch__logs__sns__sqs.metric-filter-match-drives-alarm-and-notifies, cloudwatch__logs__sns__sqs.metric-filter-non-matching-pattern-no-datapoint-no-alarm.

## Evidence
Both checks report `Error: Check timed out after 60.0s`:
  cloudwatch__logs__sns__sqs.metric-filter-match-drives-alarm-and-notifies — polls for the filter's metric datapoint, then for the alarm's ALARM state, then for the SNS notification on the subscribed SQS queue; none arrives.
  cloudwatch__logs__sns__sqs.metric-filter-non-matching-pattern-no-datapoint-no-alarm — needs the positive-control latency before it can hold its emptiness window, and never gets it.

## Steps to Reproduce
Replay the two check ids: create a log group, a metric filter with a metricTransformation, an alarm on that metric with an SNS action, and an SQS queue subscribed to the topic; PutLogEvents a matching line (then, in the twin, a non-matching line) and poll the metric, the alarm state and the queue.
