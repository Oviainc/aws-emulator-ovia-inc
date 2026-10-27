Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.alarm-forced-state-is-transient-real-evaluation-reclaims-it
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch.alarm-forced-state-is-transient-real-evaluation-reclaims-it|_poll_until#25|Timeout|time.sleep(interval)
Issue-Commit: 21bca675a34dab10ee37fe0445fa59a838d6e2c4

# SetAlarmState's forced alarm state is never reclaimed: the alarm never reaches an evaluated baseline, so the forced ALARM is durable and TreatMissingData never decides the destination (1 cause)

> _Note: this issue was identified against the code base at commit `21bca675a34dab10ee37fe0445fa59a838d6e2c4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetAlarmState

## Description
Cause 1 of 1: a client-forced alarm state is treated as the durable truth because no evaluator ever owns StateValue, so an alarm never reaches an evaluated baseline and a forced state is never taken back.

Related open issues that do NOT carry this check's replay key (the replay report left this check unowned, so it is filed here and the fixer should treat it as the SetAlarmState/transience facet of the same engine): 20261025T020208Z-cloudwatch-alarms-never-evaluate-published-metric-data-no.md and 20261025T053543Z-cloudwatch-alarm-evaluation-and-metric-read-fidelity-statereason.md.

AWS: Smithy (com.amazonaws.cloudwatch#GraniteServiceVersion20100801) models SetAlarmState with required AlarmName (@length 1..255), StateValue (enum), StateReason (@length 0..1023) and optional StateReasonData (@length 0..4000), returning Unit, with only InvalidFormatFault (400) and ResourceNotFound (404) modeled. The model says nothing about the lifetime of the state it writes, so for the lifetime this is recorded behavior only: the recording shows SetAlarmState is a TRANSIENT override. CloudWatch's own periodic evaluator re-derives StateValue from the datapoints PutMetricData ingested for the alarm's metric and overwrites both StateValue and StateReason, and the destination it computes depends on what the evaluator can see and on TreatMissingData: a metric with an in-band datapoint is re-derived to OK with a StateReason prefixed 'Threshold Crossed' naming the datapoint count it graded ('1 out of the last 1 datapoints'); a never-written metric with TreatMissingData='missing' is re-derived to INSUFFICIENT_DATA; a never-written metric with TreatMissingData='ignore' is NOT re-derived at all and keeps the forced ALARM with the client's verbatim reason. DescribeAlarmHistory (HistoryItemType=StateUpdate) records the client-forced transition with the client's stateReason and NO stateReasonData, and the evaluator's reclaiming transition with a stateReasonData carrying threshold, recentDatapoints and evaluatedDatapoints. The recording also shows the pre-force baseline is itself an evaluated OK ('Threshold Crossed' reason), not the creation state.

Emulator: the check never gets past its first step. After PutMetricData publishes an in-band value and PutMetricAlarm registers the alarm, DescribeAlarms never reports an evaluated state at all, so the poll for the OK baseline runs until the harness kills the check: 'Error: Check timed out after 60.0s'. Nothing in the emulator ever re-derives StateValue, so a state written by SetAlarmState is permanent, no ALARM->OK (or ALARM->INSUFFICIENT_DATA) StateUpdate history item is ever appended, and the 'ignore' arm cannot be distinguished from the 'missing' arm because neither is ever evaluated.

Why: a client that uses SetAlarmState to drive a test, a drill or a manual override, and then relies on CloudWatch to take the state back once real data is flowing, is left with an alarm pinned forever in the forced state. Conversely, a client that reads StateValue/StateReason/DescribeAlarmHistory to decide whether a state was client-forced or evaluator-derived cannot tell the two apart: there is never a 'Threshold Crossed' reason, never a stateReasonData decision record, and never a transition back. Autoscaling, rollback, paging and remediation flows that force a state to test a pipeline and then expect normal operation to resume stay wedged, and TreatMissingData has no observable effect on any alarm.

Fix: the alarm evaluation engine in the cloudwatch service — the periodic evaluator that joins stored metric datapoints to registered alarms, computes the m-of-n verdict under TreatMissingData, overwrites StateValue/StateReason/StateReasonData/StateUpdatedTimestamp (including over a state previously written by SetAlarmState) and appends StateUpdate history items — together with the SetAlarmState handler, which must write a transient override rather than an authoritative state, and with DescribeAlarmHistory's StateUpdate serialization (client-forced item carries no stateReasonData; the evaluated item carries threshold/recentDatapoints/evaluatedDatapoints).

Traces: cloudwatch.alarm-forced-state-is-transient-real-evaluation-reclaims-it.

## Evidence
Failing check cloudwatch.alarm-forced-state-is-transient-real-evaluation-reclaims-it, verdict mismatch, detail 'Error: Check timed out after 60.0s'.

The check hangs in its first poll, before the force is even applied:
  baseline = _poll_until(lambda: _describe(cw, alarm_name), lambda a: a["StateValue"] == "OK", timeout_s=300.0, interval_s=15.0, description="alarm ... reached OK from a published 1.0 datapoint", on_iteration=_publish)
with the exhaustion message 'the forced ALARM on <alarm> was RECLAIMED to OK by CloudWatch's own evaluator while 1.0 was published against a threshold of 10.0, with no further set_alarm_state call' for the later arm.

Expected (recorded on real AWS, per the check's own measurements): the published 1.0 drives the alarm to OK with StateReason prefixed 'Threshold Crossed' containing '1 out of the last 1 datapoints'; SetAlarmState(ALARM, 'sim2real: forced') then takes effect verbatim; with no further control-plane call the evaluator reclaims it to OK (~30 s), the never-written TreatMissingData='missing' sibling is reclaimed to INSUFFICIENT_DATA with StateReason 'Insufficient Data: 1 datapoint was unknown.' (~70 s), and the TreatMissingData='ignore' sibling is NOT reclaimed and still reports ALARM with the verbatim forced reason. DescribeAlarmHistory StateUpdate items read, oldest first, [(INSUFFICIENT_DATA, OK), (OK, ALARM), (ALARM, OK)], the OK->ALARM item carrying no stateReasonData key.

Actual: no evaluated state is ever reached, so no baseline OK, no reclaim, and no ALARM->OK StateUpdate item exists; the check is killed at the 60 s harness cap.

## Steps to Reproduce
1. PutMetricData(Namespace=NS, MetricData=[{MetricName=M, Timestamp=<current minute floor>, Value=1.0, Unit=Count}]).
2. PutMetricAlarm(AlarmName=a, Namespace=NS, MetricName=M, Statistic=Maximum, Period=60, EvaluationPeriods=1, DatapointsToAlarm=1, Threshold=10.0, ComparisonOperator=GreaterThanThreshold, TreatMissingData='missing', ActionsEnabled=false). Repeat for a second alarm over a never-written metric with TreatMissingData='missing' and a third with TreatMissingData='ignore'.
3. Poll DescribeAlarms(AlarmNames=[a], AlarmTypes=['MetricAlarm']) while re-publishing 1.0 each iteration: AWS reaches StateValue=OK with StateReason prefixed 'Threshold Crossed'. The emulator never leaves its creation state (the poll never resolves).
4. SetAlarmState(AlarmName=a, StateValue='ALARM', StateReason='sim2real: forced') on all three alarms and make no further control-plane call to them.
5. Keep publishing 1.0 and poll DescribeAlarms: AWS reclaims a to OK (reason rewritten, StateUpdatedTimestamp advanced), reclaims the never-written 'missing' alarm to INSUFFICIENT_DATA with 'Insufficient Data: 1 datapoint was unknown.', and leaves the 'ignore' alarm in the forced ALARM. The emulator leaves all three in ALARM with the forced reason indefinitely.
6. DescribeAlarmHistory(AlarmName=a, HistoryItemType='StateUpdate', ScanBy='TimestampDescending'): AWS has three items ending in ALARM->OK whose newState.stateReasonData names threshold 10.0 and evaluatedDatapoints [1.0], and whose OK->ALARM item has no stateReasonData. The emulator records no reclaiming item.
