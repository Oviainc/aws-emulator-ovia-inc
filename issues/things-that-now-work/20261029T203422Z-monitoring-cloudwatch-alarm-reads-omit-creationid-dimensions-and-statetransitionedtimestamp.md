Issue-Source: trace-sweep-divergence
Issue-Trace: cloudwatch-put-metric-alarm-with-alarm-action-arn
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudwatch-put-metric-alarm-with-alarm-action-arn cwa-alarm-actions-limit cwa-m-of-n-metric-math-round-trip cwa-string-enum-values cwp-describe-alarms-prefix-pagination cwa-metrics-exactly-one-return-data cwa-range-operator-threshold-metric-id cwa-promql-alarm-evaluation-grammar cloudwatch-describe-alarms-for-metric-dimension-filtering cloudwatch-describe-alarms-for-metric-empty cloudwatch-describe-alarms-for-metric-no-match cloudwatch-metric-alarm-queries cloudwatch-composite-alarm-dependency-and-conflict-on-delete monitoring-composite-alarm-dependency-and-conflict
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: monitoring|DescribeAlarms|2xx->2xx|$.MetricAlarms[*].CreationId
Issue-Key: monitoring|DescribeAlarms|2xx->2xx|$.MetricAlarms[*].Dimensions
Issue-Key: monitoring|DescribeAlarms|2xx->2xx|$.MetricAlarms[*].Metrics[*].MetricStat.Metric.Dimensions
Issue-Key: monitoring|DescribeAlarms|2xx->2xx|$.MetricAlarms[*].StateReason
Issue-Key: monitoring|DescribeAlarmsForMetric|2xx->2xx|$.MetricAlarms[*]
Issue-Key: monitoring|DescribeAlarms|2xx->2xx|$.MetricAlarms[*]
Issue-Commit: 6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492

# CloudWatch alarm reads omit CreationId, Dimensions and StateTransitionedTimestamp

> _Note: this issue was identified against the code base at commit `6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAlarms

## Description
6 causes, one shared fault: the MetricAlarm projection used by the alarm read paths drops members the service always returns — the per-alarm creation identity, the empty Dimensions lists, and the state-transition timestamp — and in one case never updates the derived state reason.

Cause 1 of 6: DescribeAlarms after PutMetricAlarm. AWS (CreationId is recorded behavior only — it is not a modelled MetricAlarm member in the on-disk Smithy): every MetricAlarm carries a CreationId of the form '<uuid>/<epoch-millis of the create>' identifying the configuration write, and Dimensions is ALWAYS present, as an empty list for an alarm defined without dimensions (Smithy: MetricAlarm.Dimensions targets Dimensions, a list; AWS does not elide the empty list). Emulator: CreationId absent entirely and Dimensions omitted rather than emitted as []. Why: a client that diffs alarm configuration by CreationId cannot detect a re-create, and a client iterating alarm.Dimensions hits a null where AWS guarantees a list. Fix: the MetricAlarm output projection plus CreationId assignment in the PutMetricAlarm handler/state. Trace: cloudwatch-put-metric-alarm-with-alarm-action-arn#task#2.

Cause 2 of 6: DescribeAlarms for a metric-math alarm. Same missing CreationId and Dimensions, and additionally the nested Metrics[].MetricStat.Metric.Dimensions empty list is dropped. Why: the nested metric identity in the Metrics array becomes non-round-trippable — a caller cannot feed a DescribeAlarms result straight back into PutMetricAlarm. Fix: the same projection, applied recursively to MetricDataQuery/MetricStat/Metric. Trace: cwa-metrics-exactly-one-return-data#task#4.

Cause 3 of 6: DescribeAlarms for a PromQL-grammar alarm that has been evaluated. Missing CreationId, and StateReason is still the create-time 'Unchecked: Initial alarm creation' where AWS has replaced it with the evaluation's own reason for the alarm class — for a PromQL alarm that evaluated with no series breaching, 'No time series evaluated to ALARM' (fixed text for that outcome). Why: a client reading StateReason to explain the current state is shown the creation placeholder forever, so alarm triage reports a never-evaluated alarm. Fix: the alarm evaluator's StateReason assignment for query-class alarms (plus the shared projection for CreationId). Trace: cwa-promql-alarm-evaluation-grammar#task#6.

Cause 4 of 6: DescribeAlarmsForMetric. AWS returns the same full MetricAlarm shape as DescribeAlarms, including CreationId. Emulator: CreationId absent. Same projection, different operation, so it is not closed by fixing DescribeAlarms alone. Trace: cloudwatch-describe-alarms-for-metric-dimension-filtering#task#2.

Cause 5 of 6: DescribeAlarms with ChildrenOfAlarmName (the composite-children read). AWS: each returned child carries StateTransitionedTimestamp — the time the alarm's StateValue most recently changed (Smithy: MetricAlarm.StateTransitionedTimestamp, 'The date and time that the alarm's StateValue most recently changed'). Emulator: the member is absent from the abbreviated children projection. Why: a client computing how long an alarm has been in its current state has no transition time and cannot age the state. Fix: the abbreviated children/parents projection must carry StateTransitionedTimestamp. Trace: cloudwatch-composite-alarm-dependency-and-conflict-on-delete#task#4.

Cause 6 of 6: the same missing StateTransitionedTimestamp on the single-child composite-dependency read. Trace: monitoring-composite-alarm-dependency-and-conflict#task#5.

Note on values: the recorded CreationId uuids, epoch-millis and timestamps are per-run values, not requirements — only the presence and the '<uuid>/<epoch-millis>' form of CreationId and the presence of the Dimensions/StateTransitionedTimestamp members are.

## Evidence
Replay (simparity-wire-aws-v1), 6 clusters.
1. DescribeAlarms, cloudwatch-put-metric-alarm-with-alarm-action-arn#task#2: $.MetricAlarms[0].CreationId missing (expected '<uuid>/<epoch-millis>'); $.MetricAlarms[0].Dimensions missing (expected []). 7 affected steps incl. cwa-alarm-actions-limit#task#3, cwp-describe-alarms-prefix-pagination#task#3.
2. DescribeAlarms, cwa-metrics-exactly-one-return-data#task#4: CreationId missing; Dimensions missing (expected []); Metrics[0].MetricStat.Metric.Dimensions missing (expected []).
3. DescribeAlarms, cwa-promql-alarm-evaluation-grammar#task#6: CreationId missing; $.MetricAlarms[0].StateReason expected 'No time series evaluated to ALARM', got 'Unchecked: Initial alarm creation'.
4. DescribeAlarmsForMetric, cloudwatch-describe-alarms-for-metric-dimension-filtering#task#2: expected MetricAlarm includes CreationId '<uuid>/<epoch-millis>', actual response has no CreationId member. 4 affected steps.
5. DescribeAlarms, cloudwatch-composite-alarm-dependency-and-conflict-on-delete#task#4: expected each MetricAlarms entry to carry StateTransitionedTimestamp; actual entries carry only AlarmName/AlarmArn/StateValue/StateUpdatedTimestamp.
6. DescribeAlarms, monitoring-composite-alarm-dependency-and-conflict#task#5: same missing StateTransitionedTimestamp.

## Steps to Reproduce
1. Boot the emulator from mainline and replay the simparity-wire-aws-v1 corpus.
2. PutMetricAlarm a dimensionless alarm and DescribeAlarms it (cloudwatch-put-metric-alarm-with-alarm-action-arn#task#2); check for CreationId and a Dimensions: [] member.
3. Repeat for a metric-math alarm (cwa-metrics-exactly-one-return-data#task#4) and check the nested Metrics[].MetricStat.Metric.Dimensions.
4. DescribeAlarmsForMetric for the same namespace/metric (cloudwatch-describe-alarms-for-metric-dimension-filtering#task#2).
5. DescribeAlarms with ChildrenOfAlarmName for a composite alarm (cloudwatch-composite-alarm-dependency-and-conflict-on-delete#task#4 and monitoring-composite-alarm-dependency-and-conflict#task#5) and check StateTransitionedTimestamp.
6. Evaluate a PromQL alarm and DescribeAlarms it (cwa-promql-alarm-evaluation-grammar#task#6) to see StateReason still at the creation placeholder.
