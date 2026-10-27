Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.alarm-evaluation-engine-state-machine-advances-only-from-published-data
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch.alarm-evaluation-engine-state-machine-advances-only-from-published-data|run#54|AssertionError|assert _ts(rd2["queryDate"]) >= stamps[-1] + timedelta(seconds=_PERIOD), (
Issue-Commit: 9f8def9a9a2ff2fc6658ced2c20cec21fb3f6c7d

# CloudWatch alarm StateReasonData.queryDate precedes the end of the newest evaluated period (evaluator stamps/grades an unclosed bucket)

> _Note: this issue was identified against the code base at commit `9f8def9a9a2ff2fc6658ced2c20cec21fb3f6c7d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAlarmHistory

## Description
One net-new root cause in the CloudWatch alarm evaluator's machine-readable reason object. It is reached only AFTER a real evaluation runs and after the reason sentence and the StateReasonData keys are already correct in this run, so it is NOT covered by the open issues 20261025T053543Z-cloudwatch-alarm-evaluation-and-metric-read-fidelity-statereason.md (StateReason wording / missing recentDatapoints, evaluatedDatapoints[].timestamp, statistic keys) or 20261025T020208Z-cloudwatch-alarms-never-evaluate-published-metric-data-no.md (no evaluation at all): in this run the alarm did transition INSUFFICIENT_DATA -> OK -> ALARM -> OK from published datapoints, the 'Threshold Crossed: k out of the last n datapoints [...]' sentence parsed, version/statistic/period/threshold/recentDatapoints/evaluatedDatapoints were all present and correct, and startDate equalled the oldest evaluated bucket. What diverges is queryDate.

Cause 1 of 1: StateReasonData.queryDate is earlier than the end of the newest evaluated period.

AWS: recorded behavior only — the Smithy model types the field as opaque JSON (com.amazonaws.cloudwatch#StateReasonData: string @length 0..4000, 'An explanation for the alarm state, in JSON format'; the same document is returned as AlarmHistoryItem.HistoryData -> HistoryData @length 1..4095, 'Data about the alarm, in JSON format', inside the StateUpdate item of DescribeAlarmHistory). The recorded object for an evaluation-driven StateUpdate carries version '1.0', statistic, period, threshold, recentDatapoints, evaluatedDatapoints[] (timestamp/value/sampleCount), startDate and queryDate, where startDate is the timestamp of the OLDEST evaluated bucket and queryDate is the instant at which the evaluator ran the query that produced the transition. Because CloudWatch grades only periods that have closed, queryDate is always at or after the end of the newest evaluated period: queryDate >= max(evaluatedDatapoints[].timestamp) + period. That ordering is the requirement; the concrete instants are per-run values.

Emulator: on the OK -> ALARM StateUpdate of a high-resolution alarm (Period=10, EvaluationPeriods=DatapointsToAlarm=3, Statistic=Maximum, ComparisonOperator=GreaterThanThreshold, TreatMissingData=missing, StorageResolution=1 datapoints), queryDate is stamped strictly BEFORE the end of the newest bucket it lists in evaluatedDatapoints. Observed verbatim in this run: queryDate '2026-10-27T20:19:10.702+0000' while the newest evaluated datapoint's 10-second bucket ends at that bucket's start + 10 s, i.e. after the stated query instant (the specific timestamp is a per-run value; the violated invariant is the ordering). Hypothesis: the evaluator stamps queryDate with the wall clock of the trigger that is still inside the bucket it then grades — i.e. it either grades a period that has not closed yet, or it writes queryDate before reading the window it reports.

Why: a client that reconciles an alarm decision against the metric data cannot. queryDate is the only watermark saying 'this is the data the evaluator had'; when it precedes the end of the newest graded bucket, incident enrichment, dashboards and audit tooling compute a negative evaluation latency, conclude the evaluator read data from the future, or re-query a period that was never complete — and the same off-by-one means an alarm can fire on a partial period, where AWS would wait for the period to close.

Fix: the alarm evaluator's StateReasonData serializer and its evaluation-window clock in the cloudwatch service crate (the component that writes MetricAlarm.StateReasonData and the StateUpdate HistoryData): stamp queryDate with the time the evaluation query actually ran, and restrict the graded window to closed periods so that queryDate >= newest evaluated bucket + period always holds.

Traces: cloudwatch.alarm-evaluation-engine-state-machine-advances-only-from-published-data.

## Evidence
Replayed behavioral suite (simparity-behavioral-ga-prod-v1): 186/390 passed.

[mismatch] cloudwatch.alarm-evaluation-engine-state-machine-advances-only-from-published-data
  File check_cloudwatch_alarm_evaluation_engine_state_machine_advances_only_from_published_data.py, line 189, in run
    assert _ts(rd2["queryDate"]) >= stamps[-1] + timedelta(seconds=_PERIOD), (
  AssertionError: queryDate '2026-10-27T20:19:10.702+0000' precedes the end of the newest evaluated bucket

All earlier assertions of the same check passed in this run, which bounds the defect: the three StateUpdate transitions were [(INSUFFICIENT_DATA, OK), (OK, ALARM), (ALARM, OK)], every HistorySummary read 'Alarm updated from <old> to <new>', HistoryData version was '1.0', the StateReason matched '^Threshold Crossed: 3 out of the last 3 datapoints [<value> (dd/mm/yy HH:MM:SS), ...] were greater than the threshold (50.0) (minimum 3 datapoints for OK -> ALARM transition).$', recentDatapoints was [87.0, 87.0, 87.0], the 3 evaluatedDatapoints were consecutive 10-second buckets this check had published 87.0 into, and startDate equalled the oldest of them.

Expected (ordering invariant, not a literal value): _ts(queryDate) >= max(_ts(evaluatedDatapoints[].timestamp)) + period.
Actual: queryDate < that instant.

## Steps to Reproduce
1. Clone the repo and boot the emulator, then replay the behavioral suite.
2. PutMetricAlarm with Namespace=<unique>, MetricName=Load, Statistic=Maximum, Period=10, EvaluationPeriods=3, DatapointsToAlarm=3, Threshold=50, ComparisonOperator=GreaterThanThreshold, TreatMissingData=missing, ActionsEnabled=false.
3. PutMetricData in-range values (12.0, StorageResolution=1) into the current and three preceding 10-second buckets and poll DescribeAlarms until the alarm leaves INSUFFICIENT_DATA for OK.
4. From the next fresh 10-second bucket onward, PutMetricData a breaching value (87.0) into the CURRENT bucket only, once per bucket, until DescribeAlarms reports ALARM.
5. DescribeAlarmHistory(AlarmName=<alarm>, HistoryItemType=StateUpdate, ScanBy=TimestampAscending), json.loads the HistoryData of the OK -> ALARM item and read newState.stateReasonData.
6. Compare queryDate against max(evaluatedDatapoints[].timestamp) + 10 s: queryDate is earlier, where AWS stamps it at or after that instant.
7. Re-replay cloudwatch.alarm-evaluation-engine-state-machine-advances-only-from-published-data to confirm the fix.
