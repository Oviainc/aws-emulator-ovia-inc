Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.alarm-actions-disabled-suppresses-state-triggered-action cloudwatch.alarm-composite-and-actions-ok-and-insufficient-data-functions-are-first-class cloudwatch.alarm-evaluation-engine-greater-than-operators-split-at-equal-threshold cloudwatch.alarm-evaluation-engine-less-than-operators-split-at-equal-threshold cloudwatch.alarm-evaluation-engine-statistic-maximum-alarms-where-average-does-not cloudwatch.alarm-forced-state-is-transient-real-evaluation-reclaims-it cloudwatch.alarm-m-of-n-holds-at-m-minus-one-and-fires-at-m cloudwatch.alarm-math-and-anomaly-if-expression-alarm-grades-transformed-value cloudwatch.alarm-math-and-anomaly-raw-input-breach-does-not-fire-expression-alarm cloudwatch.alarm-missing-data-and-m-of-n-period-window-slides-by-minute-and-never-evaluates-a-partial-period cloudwatch.alarm-missing-data-and-m-of-n-treat-missing-data-four-way-over-one-hole cloudwatch.alarm-unit-and-percentile-traps-low-sample-percentile-floor-is-ten-over-one-minus-p cloudwatch.alarm-unit-and-percentile-traps-standard-resolution-accepts-sub-minute-period-but-buckets-by-minute cloudwatch.cw-alarm-action-fires-to-sns cloudwatch.cw-treat-missing-data-breaching-fires-with-no-datapoints-while-notbreaching-stays-ok cloudwatch.describe-alarm-history-includes-actions-taken-telemetry cloudwatch.e2e-composite-alarm-drives-downstream-action-and-cleanup cloudwatch.e2e-metric-alarm-breach-notify-teardown cloudwatch.e2e-metric-breach-alarm-sns-notification-flow cloudwatch.fixing-the-comparison-operator-in-place-resolves-the-alarm cloudwatch.set-alarm-state-sns-notification-delivered cloudwatch.alarm-evaluation-engine-state-machine-advances-only-from-published-data
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch.alarm-actions-disabled-suppresses-state-triggered-action|_wait_for_messages#16|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: cloudwatch.alarm-composite-and-actions-ok-and-insufficient-data-functions-are-first-class|_wait_for#16|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch.alarm-evaluation-engine-greater-than-operators-split-at-equal-threshold|_drive_until#9|Timeout|time.sleep(2.0)
Issue-Key: cloudwatch.alarm-evaluation-engine-less-than-operators-split-at-equal-threshold|_drive_until#9|Timeout|time.sleep(2.0)
Issue-Key: cloudwatch.alarm-evaluation-engine-statistic-maximum-alarms-where-average-does-not|_drive_until#9|Timeout|time.sleep(2.0)
Issue-Key: cloudwatch.alarm-forced-state-is-transient-real-evaluation-reclaims-it|_poll_until#17|Timeout|time.sleep(interval)
Issue-Key: cloudwatch.alarm-m-of-n-holds-at-m-minus-one-and-fires-at-m|_poll_until#17|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch.alarm-math-and-anomaly-if-expression-alarm-grades-transformed-value|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch.alarm-math-and-anomaly-raw-input-breach-does-not-fire-expression-alarm|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch.alarm-missing-data-and-m-of-n-period-window-slides-by-minute-and-never-evaluates-a-partial-period|run#11|Timeout|time.sleep(5.0)
Issue-Key: cloudwatch.alarm-missing-data-and-m-of-n-treat-missing-data-four-way-over-one-hole|run#15|Timeout|time.sleep(5.0)
Issue-Key: cloudwatch.alarm-unit-and-percentile-traps-low-sample-percentile-floor-is-ten-over-one-minus-p|_poll_until#14|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch.alarm-unit-and-percentile-traps-standard-resolution-accepts-sub-minute-period-but-buckets-by-minute|run#12|Timeout|time.sleep(5.0)
Issue-Key: cloudwatch.cw-alarm-action-fires-to-sns|_poll_until#16|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch.cw-treat-missing-data-breaching-fires-with-no-datapoints-while-notbreaching-stays-ok|run#17|Timeout|time.sleep(15.0)
Issue-Key: cloudwatch.describe-alarm-history-includes-actions-taken-telemetry|_poll_for_history_item#12|Timeout|time.sleep(3)
Issue-Key: cloudwatch.e2e-composite-alarm-drives-downstream-action-and-cleanup|_wait_for_sqs_messages#25|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: cloudwatch.e2e-metric-alarm-breach-notify-teardown|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch.e2e-metric-breach-alarm-sns-notification-flow|_poll_alarm_state#11|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch.fixing-the-comparison-operator-in-place-resolves-the-alarm|_poll#10|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch.set-alarm-state-sns-notification-delivered|_wait_for_messages#15|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: cloudwatch.alarm-evaluation-engine-state-machine-advances-only-from-published-data|run#9|AssertionError|assert initial["StateReason"] == "Unchecked: Initial alarm creation", (
Issue-Commit: d26de989ce4778e024797c8509c4713b425f0550

# CloudWatch alarms never evaluate published metric data: no state transition, no action, no SNS notification, no Action history (2 causes)

> _Note: this issue was identified against the code base at commit `d26de989ce4778e024797c8509c4713b425f0550`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricAlarm

## Description
Two net-new root causes in the CloudWatch alarm evaluation engine. This is the pure-`cloudwatch` service instance of the symptom already filed for other service tokens (20261024T030130Z-cloudwatch-rds-cloudwatch-metric-alarms-never-evaluate-or-fire-their.md, 20261024T115115Z-cloudwatch-redshift-..., 20261025T002357Z-cloudwatch-events-sqs-...); none of those carries the replay keys below.

Cause 1 of 2: no metric alarm ever advances its state from published datapoints.
AWS: PutMetricAlarm (Smithy: PutMetricAlarmInput — Namespace/MetricName/Statistic/Period/EvaluationPeriods/Threshold/ComparisonOperator, plus DatapointsToAlarm, TreatMissingData, EvaluationWindow, WarmUpConfiguration, EvaluationInterval; only modeled error LimitExceededFault) registers an alarm that CloudWatch then evaluates once per Period against the datapoints PutMetricData ingested for that metric+dimension set. The state machine moves INSUFFICIENT_DATA -> OK/ALARM, DescribeAlarms reports the new StateValue with a StateReason beginning "Threshold Crossed", DescribeAlarmHistory records StateUpdate and Action items, and the configured AlarmActions/OKActions/InsufficientDataActions are invoked (SNS publish). SetAlarmState forces a state that a subsequent real evaluation reclaims. Composite alarms (PutCompositeAlarm, AlarmRule over ALARM()/OK()/INSUFFICIENT_DATA() functions) derive their state from their children. Recorded behavior: every check below publishes data and then polls DescribeAlarms / the SNS-backed queue / DescribeAlarmHistory for the resulting transition.
Emulator: no transition ever occurs. 21 checks publish their data and then poll until the harness 60 s cap and are killed — the alarm never leaves its creation state, no action fires, no SNS message is delivered, and no Action item appears in DescribeAlarmHistory. This holds for every evaluation axis independently: comparison-operator boundaries, Statistic=Maximum vs Average, m-of-n DatapointsToAlarm, TreatMissingData (breaching/notBreaching/ignore/missing), percentile/low-sample handling, high- and standard-resolution period bucketing, metric-math/IF-expression alarms, composite alarms, SetAlarmState, ActionsEnabled=false suppression, and in-place reconfiguration of ComparisonOperator.
Why: a client that creates an alarm and waits for it to fire waits forever. Every autoscaling, rollback, paging and remediation flow built on CloudWatch alarms silently never triggers, and a dashboard or runbook reading StateValue/StateReason/DescribeAlarmHistory sees an alarm that looks permanently un-evaluated instead of OK or ALARM.
Fix: the alarm evaluation engine — the periodic evaluator that joins stored metric datapoints to registered alarms, computes the m-of-n verdict, writes StateValue/StateReason/StateUpdatedTimestamp, appends alarm-history items and dispatches alarm actions — in the cloudwatch service's alarm state and its PutMetricAlarm/PutCompositeAlarm/SetAlarmState handlers.
Traces: cloudwatch.alarm-actions-disabled-suppresses-state-triggered-action, cloudwatch.alarm-composite-and-actions-ok-and-insufficient-data-functions-are-first-class, cloudwatch.alarm-evaluation-engine-greater-than-operators-split-at-equal-threshold, cloudwatch.alarm-evaluation-engine-less-than-operators-split-at-equal-threshold, cloudwatch.alarm-evaluation-engine-statistic-maximum-alarms-where-average-does-not, cloudwatch.alarm-forced-state-is-transient-real-evaluation-reclaims-it, cloudwatch.alarm-m-of-n-holds-at-m-minus-one-and-fires-at-m, cloudwatch.alarm-math-and-anomaly-if-expression-alarm-grades-transformed-value, cloudwatch.alarm-math-and-anomaly-raw-input-breach-does-not-fire-expression-alarm, cloudwatch.alarm-missing-data-and-m-of-n-period-window-slides-by-minute-and-never-evaluates-a-partial-period, cloudwatch.alarm-missing-data-and-m-of-n-treat-missing-data-four-way-over-one-hole, cloudwatch.alarm-unit-and-percentile-traps-low-sample-percentile-floor-is-ten-over-one-minus-p, cloudwatch.alarm-unit-and-percentile-traps-standard-resolution-accepts-sub-minute-period-but-buckets-by-minute, cloudwatch.cw-alarm-action-fires-to-sns, cloudwatch.cw-treat-missing-data-breaching-fires-with-no-datapoints-while-notbreaching-stays-ok, cloudwatch.describe-alarm-history-includes-actions-taken-telemetry, cloudwatch.e2e-composite-alarm-drives-downstream-action-and-cleanup, cloudwatch.e2e-metric-alarm-breach-notify-teardown, cloudwatch.e2e-metric-breach-alarm-sns-notification-flow, cloudwatch.fixing-the-comparison-operator-in-place-resolves-the-alarm, cloudwatch.set-alarm-state-sns-notification-delivered.

Cause 2 of 2: the initial StateReason of a freshly created alarm is miscased.
AWS: a metric alarm that has not been evaluated yet reports StateValue=INSUFFICIENT_DATA with the fixed StateReason literal "Unchecked: Initial alarm creation" (capital I). The string is a service-produced constant, not derived from the request, and clients and the console match on it to distinguish "never evaluated" from "evaluated, no data".
Emulator: DescribeAlarms returns StateReason 'Unchecked: initial alarm creation' (lowercase i).
Why: a client that distinguishes a never-evaluated alarm by this exact literal misclassifies every newly created alarm.
Fix: the constant used when seeding alarm state in the PutMetricAlarm handler / alarm state representation.
Traces: cloudwatch.alarm-evaluation-engine-state-machine-advances-only-from-published-data.

## Evidence
Cause 1 — all 21 checks: "Error: Check timed out after 60.0s" while polling for the state transition / action. Representative in-check waits that never resolve: `raise AssertionError(f"{what} never happened while the check kept publishing; ...")`, `raise AssertionError(f"no SNS alarm notification for {alarm_name} arrived within {timeout_s}s")`, `raise AssertionError(f"no {item_type} alarm history item ever appeared for {alarm_name}")`, `raise AssertionError(f"expected {expected_count} messages, got {len(out)} in {timeout_s}s")`.

Cause 2 — cloudwatch.alarm-evaluation-engine-state-machine-advances-only-from-published-data:
  assert initial["StateReason"] == "Unchecked: Initial alarm creation"
  AssertionError: initial StateReason was 'Unchecked: initial alarm creation'

## Steps to Reproduce
1. PutMetricAlarm(AlarmName=a, Namespace=NS, MetricName=M, Statistic=Average, Period=60, EvaluationPeriods=1, Threshold=80, ComparisonOperator=GreaterThanThreshold, AlarmActions=[an SNS topic ARN]).
2. DescribeAlarms(AlarmNames=[a]) -> observe StateReason 'Unchecked: initial alarm creation' (AWS: 'Unchecked: Initial alarm creation').
3. PutMetricData into NS/M with Value=99 repeatedly for several periods.
4. Poll DescribeAlarms(AlarmNames=[a]) for StateValue=ALARM, DescribeAlarmHistory for an Action item, and the SQS queue subscribed to the topic for a notification: none of the three ever occurs.
