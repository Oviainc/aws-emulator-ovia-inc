Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.alarm-math-and-anomaly-if-expression-alarm-grades-transformed-value
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch.alarm-math-and-anomaly-if-expression-alarm-grades-transformed-value|_poll_until#12|AssertionError|raise AssertionError(f"{description} did not happen within {timeout_s}s; "
Issue-Commit: 1aab6ad9a4dd8a2817d61de8022dab6fc1da2f9a

# Metric-math alarm over IF() never evaluates: a Metrics[]-based alarm stays INSUFFICIENT_DATA with StateReason 'Unchecked: initial alarm creation' no matter how much data is published (1 cause)

> _Note: this issue was identified against the code base at commit `1aab6ad9a4dd8a2817d61de8022dab6fc1da2f9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricAlarm

## Description
Cause 1 of 1: an alarm defined by a `Metrics` array (a metric-math expression with `ReturnData=true`) is never evaluated, so it never leaves its creation state.

Related open issue: 20261025T020208Z-cloudwatch-alarms-never-evaluate-published-metric-data-no.md lists this check id among its trace ids, but it does NOT carry this check's current replay key — the replay suite did not mark this failure as OWNED, and the check no longer fails the way that issue records it (a 60 s harness timeout). It now fails with its own AssertionError inside its poll budget, from the Metrics[]/metric-math alarm path specifically. Filing it so the key is attached to a work item; a fixer should treat it together with that issue if the evaluator is the same component.

AWS: PutMetricAlarm (Smithy: PutMetricAlarmInput members `Metrics` -> MetricDataQueries, `Threshold`, `ComparisonOperator`, `EvaluationPeriods`, `DatapointsToAlarm`, `TreatMissingData`, `ActionsEnabled`; only modeled error LimitExceededFault, httpError 400 / awsQueryError LimitExceeded) registers an alarm whose graded series is the single `ReturnData=true` query in `Metrics`. CloudWatch then evaluates that alarm once per the query's `Period` against the datapoints PutMetricData has ingested, and the state machine leaves INSUFFICIENT_DATA for OK or ALARM. On the first real evaluation DescribeAlarms (Smithy: DescribeAlarmsOutput.MetricAlarms) reports the new `StateValue`, a `StateReason` whose fixed text begins "Threshold Crossed" (its variable parts are the datapoint count, the datapoint values, the comparison operator and the threshold from the request), a `StateReasonData` document whose `evaluatedDatapoints` carry the value of the EXPRESSION output (not of the raw input metric), and a `StateUpdatedTimestamp` later than `AlarmConfigurationUpdatedTimestamp`. The un-evaluated seed state carries the service constant StateReason "Unchecked: Initial alarm creation". For `IF(m1 > 100, 0, 1000)` with raw m1=700 the graded value is 0, so an alarm with Threshold=500 / GreaterThanThreshold must settle OK; with raw m1=50 the graded value is 1000, so it must reach ALARM, and DescribeAlarmHistory must record a StateUpdate item whose newState.stateReasonData.recentDatapoints is the expression output. The same values must be reproducible through GetMetricData over the identical `Metrics` array (Smithy: GetMetricDataInput.MetricDataQueries). Reference: AWS metric-math documentation for alarms.

Emulator: the alarm is created and reads back correctly (DescribeAlarms returns the stored `Metrics` array with `Expression` intact), but it is never evaluated. While the check publishes a datapoint every 2 s for ~40 s, DescribeAlarms keeps returning `StateValue='INSUFFICIENT_DATA'`, `StateReason='Unchecked: initial alarm creation'` (also the wrong case — AWS writes a capital "I" in "Initial"), no `StateReasonData` member at all, and `StateUpdatedTimestamp` frozen equal to `AlarmConfigurationUpdatedTimestamp`. No StateUpdate history item is ever appended. No error is returned; the alarm simply sits un-evaluated forever.

Why: a client that creates a metric-math alarm and waits for it to grade its expression waits forever. Any autoscaling, rollback, paging or remediation flow keyed on a metric-math alarm silently never triggers, and a dashboard reading StateValue/StateReason/StateReasonData sees a permanently un-evaluated alarm instead of OK or ALARM. Because `StateReasonData` is absent, a client that parses `evaluatedDatapoints` to learn which value was graded cannot distinguish "not evaluated" from "evaluated against the raw input".

Fix: the CloudWatch alarm evaluation engine — the periodic evaluator that joins stored metric datapoints to registered alarms, resolves the `Metrics` array's metric-math expression (including IF()) to the graded series, computes the m-of-n verdict, and writes StateValue/StateReason/StateReasonData/StateUpdatedTimestamp plus the alarm-history StateUpdate item — in the cloudwatch service's alarm state and its PutMetricAlarm handler. The "Unchecked: Initial alarm creation" seed literal belongs in the same place.

Traces: cloudwatch.alarm-math-and-anomaly-if-expression-alarm-grades-transformed-value.

## Evidence
Check cloudwatch.alarm-math-and-anomaly-if-expression-alarm-grades-transformed-value [mismatch]:

  AssertionError: alarm 'sim2real-behav-mathif-<id>' left INSUFFICIENT_DATA on its first real evaluation did not happen within 39.9s; last observation {... 'StateValue': 'INSUFFICIENT_DATA', 'StateReason': 'Unchecked: initial alarm creation', 'StateUpdatedTimestamp': <equal to AlarmConfigurationUpdatedTimestamp>, 'EvaluationPeriods': 1, 'DatapointsToAlarm': 1, 'Threshold': 500.0, 'ComparisonOperator': 'GreaterThanThreshold', 'TreatMissingData': 'missing', 'Metrics': [{'Id': 'm1', 'MetricStat': {'Metric': {'Namespace': '<ns>', 'MetricName': 'Load'}, 'Period': 60, 'Stat': 'Minimum'}, 'ReturnData': False}, {'Id': 'e1', 'Expression': 'IF(m1 > 100, 0, 1000)', 'Label': 'Inverted', 'ReturnData': True}]}

Expected (AWS): within a few evaluation periods StateValue='OK' (IF(m1>100,0,1000) with m1=700 grades 0, which is not > 500), StateReason beginning the fixed text "Threshold Crossed" with the request's operator/threshold and the graded datapoint values substituted, and StateReasonData.evaluatedDatapoints == [{timestamp, value: 0.0}]; then after publishing m1=50 the expression grades 1000 and the alarm reaches ALARM from OK, with a DescribeAlarmHistory StateUpdate item whose newState.stateReasonData.recentDatapoints == [1000.0].

Actual: no transition, no StateReasonData, no StateUpdate history item; StateReason stays the seed literal and is additionally miscased ('initial' vs AWS's 'Initial').

## Steps to Reproduce
1. Pick a fresh namespace NS. PutMetricData(Namespace=NS, MetricData=[{MetricName:'Load', Timestamp:<current minute>, Value:700, Unit:'Count'}]).
2. PutMetricAlarm(AlarmName=a, Threshold=500, ComparisonOperator='GreaterThanThreshold', EvaluationPeriods=1, DatapointsToAlarm=1, TreatMissingData='missing', ActionsEnabled=false, Metrics=[{Id:'m1', MetricStat:{Metric:{Namespace:NS, MetricName:'Load'}, Period:60, Stat:'Minimum'}, ReturnData:false}, {Id:'e1', Expression:'IF(m1 > 100, 0, 1000)', Label:'Inverted', ReturnData:true}]).
3. DescribeAlarms(AlarmNames=[a], AlarmTypes=['MetricAlarm']) -> StateReason is 'Unchecked: initial alarm creation' (AWS: 'Unchecked: Initial alarm creation').
4. Keep republishing Value=700 every 2 s and poll DescribeAlarms: StateValue never leaves 'INSUFFICIENT_DATA', StateReasonData is never present, StateUpdatedTimestamp never advances. AWS settles the alarm to OK (the IF output 0 is below 500).
5. Publish Value=50 and keep polling: the alarm never reaches ALARM, and DescribeAlarmHistory(AlarmName=a, HistoryItemType='StateUpdate') stays empty.
6. GetMetricData over the same Metrics array (both queries ReturnData=true) to confirm the expression itself is computable, then compare with what the alarm cited.
