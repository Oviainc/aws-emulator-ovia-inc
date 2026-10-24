Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__logs__sns__sqs.deep-cloudwatch-2-cwlogalarm-zero-match-count-is-missing-not-zero cloudwatch__logs__sns__sqs.metric-filter-default-value-fills-only-ingested-periods
Issue-Key: cloudwatch__logs__sns__sqs.deep-cloudwatch-2-cwlogalarm-zero-match-count-is-missing-not-zero|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__logs__sns__sqs.metric-filter-default-value-fills-only-ingested-periods|_wait_states#10|Timeout|time.sleep(10)
Issue-Commit: 483d650f1d2fec6203edc8ec91bc65f69df3bd2d

# cloudwatch__logs__sns__sqs: a log-metric-filter alarm pipeline produces no datapoints, so neither the m-of-n breach arm nor the defaultValue backfill arm ever becomes observable (1 cause)

> _Note: this issue was identified against the code base at commit `483d650f1d2fec6203edc8ec91bc65f69df3bd2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricFilter

## Description
One root cause. Cause count: 1.

=== Cause 1 of 1: the CloudWatch Logs metric-filter -> CloudWatch metric -> alarm -> SNS/SQS chain emits nothing ===
AWS: PutMetricFilter installs an ingest-time rule; each PutLogEvents event matching filterPattern publishes a datapoint for the transformation's namespace/metric, and a metricTransformation with defaultValue publishes that value for ingested periods in which the pattern did NOT match (while periods with no ingestion at all stay MISSING, not zero). A metric alarm over that metric then transitions on the m-of-n breach count and notifies its SNS topic, whose message lands in the subscribed SQS queue.
Emulator: no datapoint from the metric filter ever appears, so the alarm never leaves its initial state and no notification is ever published. Both checks poll (returning on the first successful reading, so one correct iteration would end them) and instead run to the suite's 60 s cap.
Why: this is the standard log-based alerting topology. A client gets a filter that exists, an alarm that exists and a topic that exists, with no signal flowing between them — and crucially cannot distinguish "zero matches" (which AWS reports as MISSING, or as defaultValue once a period has ingestion) from "nothing is wired", which is exactly what these two checks grade.
Fix: the metric-filter evaluation and emission stage of the logs ingest path (PutLogEvents), and the CloudWatch metric store/alarm evaluator that reads it. Same underlying emission gap as the `logs` metric-filter issue; filed separately because this group's service token differs and these two checks additionally grade the alarm/notification arm.
Traces: cloudwatch__logs__sns__sqs.deep-cloudwatch-2-cwlogalarm-zero-match-count-is-missing-not-zero, cloudwatch__logs__sns__sqs.metric-filter-default-value-fills-only-ingested-periods.

## Evidence
cloudwatch__logs__sns__sqs.deep-cloudwatch-2-cwlogalarm-zero-match-count-is-missing-not-zero: 'Check timed out after 60.0s'. The check polls describe_alarms / alarm history (_poll_until with timeout_s=300 and 240, interval 10s, returning on the first reading that satisfies the predicate) and awaits the alarm notification payload on SQS (_await_payload, timeout_s=180) — none of those predicates was ever satisfied, so no metric datapoint and no alarm transition occurred.
cloudwatch__logs__sns__sqs.metric-filter-default-value-fills-only-ingested-periods: 'Check timed out after 60.0s'. The defaultValue=0 datapoints for ingested-but-non-matching periods never appeared, so the "MISSING vs zero" distinction could not be read at all.
Expected: a datapoint per matching event (plus defaultValue datapoints for ingested non-matching periods), the alarm reaching ALARM on the m-of-n breach count with a StateReason beginning 'Threshold Crossed', and the notification delivered to the SQS queue.

## Steps to Reproduce
CreateLogGroup + CreateLogStream; PutMetricFilter with metricTransformations=[{metricName, metricNamespace, metricValue:'1', defaultValue:0}] and a filterPattern; cloudwatch PutMetricAlarm over that namespace/metric with EvaluationPeriods>1 and DatapointsToAlarm<EvaluationPeriods, AlarmActions=[<SNS topic subscribed by an SQS queue>]; PutLogEvents enough matching lines to breach; poll cloudwatch GetMetricStatistics / DescribeAlarms and sqs ReceiveMessage — no datapoint, no state change, no notification.
