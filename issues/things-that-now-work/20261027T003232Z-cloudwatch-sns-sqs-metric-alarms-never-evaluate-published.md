Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__sns__sqs.alarm-composite-and-actions-disabled-actions-transition-but-delivery-blocked
Issue-Key: cloudwatch__sns__sqs.alarm-composite-and-actions-disabled-actions-transition-but-delivery-blocked|_wait_for#35|Timeout|time.sleep(interval_s)
Issue-Commit: d220b5129ac92467b2bf094da95d24c404aff436

# cloudwatch__sns__sqs: metric alarms never evaluate published datapoints, so the DisableAlarmActions differential (transition still happens, delivery blocked) can never be observed (1 cause)

> _Note: this issue was identified against the code base at commit `d220b5129ac92467b2bf094da95d24c404aff436`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricAlarm

## Description
One net-new root cause (Cause 1 of 1).

Cause 1 of 1: two alarms watching the same metric with the same SNS action never transition from datapoints published with PutMetricData, so the one behavior under test — that DisableAlarmActions blocks DELIVERY while real evaluation STILL transitions the alarm, and EnableAlarmActions restores delivery on the next real transition — cannot occur at all.
  AWS: recorded behavior only for the evaluator (the Smithy model describes PutMetricAlarm/DisableAlarmActions/EnableAlarmActions configuration, not evaluation). The recording shows: with Namespace=<ns>, MetricName=latency, Dimensions=[{Run,<id>}], Statistic=Maximum, Period=10, EvaluationPeriods=1, Threshold=50, ComparisonOperator=GreaterThanThreshold, TreatMissingData=ignore and AlarmActions=[topic], a stream of breaching datapoints (211.0) moves BOTH alarms to ALARM — DescribeAlarmHistory records a StateUpdate item for the muted alarm too — while only the alarm that kept ActionsEnabled produces a notification on the subscribed queue (its payload's Trigger names the shared Namespace/MetricName/Dimensions); the muted alarm gets no notification and no Action history item. After EnableAlarmActions on the muted alarm, driving it OK (2.0) then ALARM again delivers its notification with OldStateValue "OK".
  Emulator: no evaluation occurs. Neither alarm transitions on the published datapoints, so no notification is delivered for the enabled (positive control) alarm either, and the muted alarm's absence proves nothing. The check never reaches its assertions and the replay reports `Check timed out after 60.0s`. (Note for the fixer: the check's own budget is 900 s with 240 s steps, well above the suite's 60 s cap, so the timeout alone does not localise the step — but the positive control, the first thing it waits on, is an ALARM notification that never arrives.)
  Why: a client cannot use CloudWatch alarms for anything real: alarms accept configuration and then never fire, so ActionsEnabled/DisableAlarmActions is indistinguishable from the general silence, no alarm-driven automation (SNS fan-out, pagers, rollbacks) ever runs, and DescribeAlarmHistory records neither StateUpdate nor Action items.
  Fix: the alarm evaluation path in the cloudwatch service crate — datapoints accepted by PutMetricData must drive the alarm state machine per Period/EvaluationPeriods/Statistic/ComparisonOperator/TreatMissingData, transitions must be recorded in DescribeAlarmHistory (StateUpdate), and the action dispatch must be gated by ActionsEnabled only (so a muted alarm transitions but publishes nothing, and re-enabling restores publication on the next transition).
  Related open issues that do NOT carry this check's replay key: 20261025T020208Z-cloudwatch-alarms-never-evaluate-published-metric-data-no.md and 20261024T030130Z-cloudwatch-rds-cloudwatch-metric-alarms-never-evaluate-or-fire-their.md describe the same family of silence under different service tokens/checks, and 20261026T031713Z-cloudwatch-2-net-new-root-causes-putmetricalarm-refuses.md covers an unrelated PutMetricAlarm refusal.
  Traces: cloudwatch__sns__sqs.alarm-composite-and-actions-disabled-actions-transition-but-delivery-blocked (mismatch, timed out at the harness cap).

## Evidence
Cause 1 of 1 — expected vs actual.
  Expected (AWS, recorded): after put_metric_data of breaching values (211.0 > Threshold 50) for the shared metric, DescribeAlarms shows BOTH the `enabled` and the `muted` alarm in ALARM; the subscribed SQS queue receives exactly the `enabled` alarm's notification, whose Trigger names the shared Namespace/MetricName/Dimensions; the `muted` alarm has a StateUpdate but no Action item in DescribeAlarmHistory and produces no message within the measured silence window; after EnableAlarmActions plus a real OK->ALARM cycle, the `muted` alarm's ALARM notification IS delivered with OldStateValue "OK".
  Actual (emulator): no state transition is observed on either alarm, so the positive control's ALARM notification never arrives on the queue; the check does not complete and is reported as `Check timed out after 60.0s`.

## Steps to Reproduce
1. Create an SNS topic and an SQS queue subscribed to it (queue policy allowing sns.amazonaws.com).
2. cloudwatch:PutMetricAlarm twice (names `enabled` and `muted`) with the same Namespace/MetricName/Dimensions, Statistic=Maximum, Period=10, EvaluationPeriods=1, Threshold=50, ComparisonOperator=GreaterThanThreshold, TreatMissingData=ignore, AlarmActions=[topic].
3. cloudwatch:DisableAlarmActions(AlarmNames=[`muted`]).
4. cloudwatch:PutMetricData for that metric with value 211.0 repeatedly for ~2 periods.
5. Poll DescribeAlarms, DescribeAlarmHistory and the queue. AWS: both alarms ALARM, only `enabled`'s notification on the queue, no Action item for `muted`. Emulator: neither alarm leaves its initial state and the queue stays empty.
6. cloudwatch:EnableAlarmActions(AlarmNames=[`muted`]), drive 2.0 then 211.0, and expect `muted`'s ALARM notification with OldStateValue "OK".
  Replay: cloudwatch__sns__sqs.alarm-composite-and-actions-disabled-actions-transition-but-delivery-blocked (allow the check its declared 900 s budget, not the 60 s suite cap).
