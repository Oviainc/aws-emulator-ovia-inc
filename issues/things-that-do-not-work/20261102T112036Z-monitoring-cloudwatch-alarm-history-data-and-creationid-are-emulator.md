Issue-Source: trace-sweep-divergence
Issue-Trace: cloudwatch-latency-alarm-history-query-mistakes
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudwatch-latency-alarm-history-query-mistakes cloudwatch-rshealth-operator-fix-mistakes cloudwatch-alarm-firing-describe-filter-mistakes cloudwatch-albos-alarm-lookup-and-state-mistakes cloudwatch-sftp-traj-alarm-reference-reads cloudwatch-sftp-traj-add-alarm-actions cloudwatch-alarms-for-metric-percentile-lookup cloudwatch-rshealth-alarm-lookup-by-metric cloudwatch-s3tfa-traj-describe-alarms-for-metric-mistakes-then-the-hit
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: monitoring|DescribeAlarmHistory|2xx->2xx|$.AlarmHistoryItems[*].HistoryData
Issue-Key: monitoring|DescribeAlarmHistory|2xx->2xx|$.AlarmHistoryItems[*].HistorySummary
Issue-Key: monitoring|DescribeAlarmHistory|2xx->2xx|$.AlarmHistoryItems[*]
Issue-Key: monitoring|DescribeAlarms|2xx->2xx|$.MetricAlarms[*].StateReasonData
Issue-Key: monitoring|DescribeAlarmsForMetric|2xx->2xx|$.MetricAlarms[*].CreationId
Issue-Key: monitoring|DescribeAlarmsForMetric|2xx->2xx|$.MetricAlarms[*].StateReasonData
Issue-Related: 20261029T203422Z-monitoring-cloudwatch-alarm-reads-omit-creationid-dimensions-and-statetransitionedtimestamp.md
Issue-Commit: 7923db9e5c097e7a9eee0c015b735053c707d1d4

# CloudWatch alarm history data and CreationId are emulator-authored or absent

> _Note: this issue was identified against the code base at commit `7923db9e5c097e7a9eee0c015b735053c707d1d4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAlarmHistory

## Description
This issue bundles SIX causes sharing one fault: CloudWatch alarm READ payloads are assembled from the emulator's own view of an alarm rather than from AWS's documented serialisation - the alarm-history JSON blobs use the wrong member names, key order and envelope, the per-item summary omits the alarm name, and the alarm structures omit CreationId. There are 6 causes to close.

Related open issue: "CloudWatch alarm reads omit CreationId, Dimensions and StateTransitionedTimestamp" (20261029T203422Z-monitoring-cloudwatch-alarm-reads-omit-creationid-dimensions-and-statetransitionedtimestamp.md) owns the CreationId/Dimensions/StateTransitionedTimestamp omission on SOME DescribeAlarms clusters. It does NOT carry the replay keys for the six clusters below - the DescribeAlarmHistory HistoryData/HistorySummary causes, and the DescribeAlarms/DescribeAlarmsForMetric clusters listed here - so they are filed net-new and must be closed on their own replay keys.

Cause 1 of 6: DescribeAlarmHistory 'Create' HistoryData and HistorySummary.
  AWS: Smithy: AlarmHistoryItem.HistoryData is documented as "Data about the alarm, in JSON format" (@length 1..4095) and HistorySummary as "A summary of the alarm history, in text format" (@length 1..255); the model does not fix the JSON's internal shape, so this is recorded behavior. The recording shows AWS's Create item as {"version":"1.0","type":"Create","createdAlarm":{...}} where the nested alarm object (a) carries alarmConfigurationUpdatedTimestamp and stateValue/stateUpdatedTimestamp rendered as ISO-8601 with a +0000 offset, (b) spells the OK action list "okactions" (all lower case), and (c) orders each dimension object as {"value":...,"name":...}. The HistorySummary is the fixed text 'Alarm "<alarmName>" created', with the alarm name taken from the request/state.
  Emulator: emits {"version":"1.0","type":"Create","createdAlarm":{...}} with a different member set and order - it spells the list "okActions", orders dimensions {"name":...,"value":...}, and omits alarmConfigurationUpdatedTimestamp, stateValue and stateUpdatedTimestamp from the created-alarm snapshot. HistorySummary is the constant "Alarm created", with the alarm name dropped.
  Why: HistoryData is a JSON document clients parse; a client reading createdAlarm.stateValue or createdAlarm.okactions gets undefined, and a client listing history for several alarms cannot tell them apart because every summary is the same string. The specific alarm name and timestamps in any one recording are per-run values; the requirement is the member names, the nesting and the summary template.
  Fix: DescribeAlarmHistory's HistoryData/HistorySummary serializer for ConfigurationUpdate/Create items.
  Traces: cloudwatch-latency-alarm-history-query-mistakes#task#6.

Cause 2 of 6: DescribeAlarmHistory 'Update' HistoryData omits originalUpdatedFields and emits a non-AWS originalAlarm.
  AWS: recorded behavior only. AWS's Update item is {"version":"1.0","type":"Update","updatedAlarm":{...},"originalUpdatedFields":{...}} - the second key is originalUpdatedFields and contains ONLY the members that changed (in the recording: alarmConfigurationUpdatedTimestamp and comparisonOperator). The updatedAlarm snapshot again carries alarmConfigurationUpdatedTimestamp, stateValue, stateUpdatedTimestamp and "okactions", and renders the statistic member as extendedStatistic where the alarm uses one.
  Emulator: emits "originalAlarm" - a full copy of the previous alarm - instead of "originalUpdatedFields", and again uses "okActions", omits the state/timestamp members, and writes "statistic":"Average" where the recording has "extendedStatistic".
  Why: the whole value of the Update history item is the delta; a client auditing "what changed" must diff two full snapshots itself, and a client keyed on originalUpdatedFields finds nothing.
  Fix: DescribeAlarmHistory's HistoryData serializer for update items - record and emit the changed-member delta.
  Traces: cloudwatch-rshealth-operator-fix-mistakes#task#8.

Cause 3 of 6: DescribeAlarmHistory StateUpdate HistoryData shape.
  AWS: recorded behavior only; the fixture asserts the item is shaped, and the emulator's blob does not match the recorded shape for a state transition item ({"version":"1.0","oldState":{...},"newState":{...}} with the nested stateReasonData).
  Emulator: produces a StateUpdate blob the shape assertion rejects.
  Why: as cause 1 - clients parse HistoryData; a shape that differs from AWS's breaks any consumer reading oldState/newState.
  Fix: DescribeAlarmHistory's HistoryData serializer for StateUpdate items.
  Traces: cloudwatch-alarm-firing-describe-filter-mistakes#task#6 (plus cloudwatch-albos-alarm-lookup-and-state-mistakes#task#8, cloudwatch-sftp-traj-alarm-reference-reads#task#5).

Cause 4 of 6: DescribeAlarms omits MetricAlarm.CreationId and returns a StateReasonData that fails the recorded shape.
  AWS: recorded behavior only for CreationId - the Smithy MetricAlarm structure in this checkout does not declare it, but every recorded DescribeAlarms response carries a CreationId on each MetricAlarm, formed as '<uuid>/<epoch-millis>'. StateReasonData is modelled (@length 0..4000, "An explanation for the alarm state, in JSON format") and the recording asserts its shape.
  Emulator: CreationId is absent from MetricAlarms[0] entirely, and the StateReasonData blob it returns does not satisfy the recorded shape.
  Why: a client that round-trips an alarm description, or that uses CreationId to correlate an alarm with the call that created it, finds the member missing; a client parsing StateReasonData for the evaluated datapoints reads a differently shaped document. The uuid and epoch in any one recording are per-run values - the requirement is that the member is present and in that form.
  Fix: the DescribeAlarms response serializer and the alarm state record that backs CreationId/StateReasonData.
  Traces: cloudwatch-alarm-firing-describe-filter-mistakes#task#4 (plus cloudwatch-albos-alarm-lookup-and-state-mistakes#task#4, #task#6, cloudwatch-sftp-traj-add-alarm-actions#task#5, #task#7, cloudwatch-sftp-traj-alarm-reference-reads#task#2).

Cause 5 of 6: DescribeAlarmsForMetric omits MetricAlarm.CreationId and returns a non-conforming StateReasonData.
  AWS: as cause 4 - every recorded DescribeAlarmsForMetric MetricAlarm carries CreationId and a shaped StateReasonData.
  Emulator: CreationId absent; StateReasonData fails the shape assertion.
  Why: the same alarm read through DescribeAlarmsForMetric is missing the same member, so the defect is in the shared alarm serializer rather than in one operation.
  Fix: the shared MetricAlarm serializer used by DescribeAlarmsForMetric.
  Traces: cloudwatch-alarm-firing-describe-filter-mistakes#task#9 (plus cloudwatch-sftp-traj-alarm-reference-reads#task#8).

Cause 6 of 6: DescribeAlarmsForMetric omits MetricAlarm.CreationId on the percentile-lookup path.
  AWS: as cause 4; the recorded response carries CreationId on MetricAlarms[0].
  Emulator: CreationId absent.
  Why: same client impact; listed separately because it is its own replay cluster and will not be exercised by the others.
  Fix: the shared MetricAlarm serializer.
  Traces: cloudwatch-alarms-for-metric-percentile-lookup#task#4 (plus cloudwatch-rshealth-alarm-lookup-by-metric#task#5, #task#6, cloudwatch-s3tfa-traj-describe-alarms-for-metric-mistakes-then-the-hit#task#8).

## Evidence
Cause 1 - DescribeAlarmHistory Create item: expected HistoryData "{\"version\":\"1.0\",\"type\":\"Create\",\"createdAlarm\":{...,\"alarmConfigurationUpdatedTimestamp\":\"<ISO8601+0000>\",...,\"stateValue\":\"INSUFFICIENT_DATA\",\"stateUpdatedTimestamp\":\"<ISO8601+0000>\",\"actionsEnabled\":false,\"alarmActions\":[],\"insufficientDataActions\":[],\"okactions\":[]}}" and HistorySummary 'Alarm "<alarmName>" created'; actual HistoryData uses \"okActions\", omits alarmConfigurationUpdatedTimestamp/stateValue/stateUpdatedTimestamp and reorders the dimension members, and HistorySummary is the constant "Alarm created". Status 200 both.
Cause 2 - DescribeAlarmHistory Update item: expected the second top-level key "originalUpdatedFields" carrying only the changed members (alarmConfigurationUpdatedTimestamp, comparisonOperator) and an updatedAlarm carrying stateValue/stateUpdatedTimestamp/"okactions"/extendedStatistic; actual emits "originalAlarm" with a full previous snapshot, "okActions", "statistic":"Average", and no state members.
Cause 3 - DescribeAlarmHistory StateUpdate item: expected a shaped HistoryData; actual blob fails the shape assertion.
Cause 4 - DescribeAlarms: expected $.MetricAlarms[0].CreationId present and a shaped $.MetricAlarms[0].StateReasonData; actual CreationId None and a StateReasonData that fails the shape assertion. Status 200 both.
Cause 5 - DescribeAlarmsForMetric: expected $.MetricAlarms[0].CreationId present and a shaped StateReasonData; actual CreationId None, StateReasonData non-conforming.
Cause 6 - DescribeAlarmsForMetric: expected $.MetricAlarms[0].CreationId present; actual None.

## Steps to Reproduce
1. Boot the emulator and replay the wire corpus (replay_wire_corpus).
2. Cause 1: fixture cloudwatch-latency-alarm-history-query-mistakes, step task#6 - PutMetricAlarm then DescribeAlarmHistory and inspect AlarmHistoryItems[0].HistoryData and .HistorySummary.
3. Cause 2: fixture cloudwatch-rshealth-operator-fix-mistakes, step task#8 - update an existing alarm, then DescribeAlarmHistory and inspect the Update item's HistoryData for originalUpdatedFields.
4. Cause 3: fixture cloudwatch-alarm-firing-describe-filter-mistakes, step task#6 - drive a state transition then DescribeAlarmHistory and inspect the StateUpdate HistoryData.
5. Cause 4: fixture cloudwatch-alarm-firing-describe-filter-mistakes, step task#4 - DescribeAlarms and inspect MetricAlarms[0].CreationId and .StateReasonData.
6. Cause 5: fixture cloudwatch-alarm-firing-describe-filter-mistakes, step task#9 - DescribeAlarmsForMetric and inspect MetricAlarms[0].CreationId and .StateReasonData.
7. Cause 6: fixture cloudwatch-alarms-for-metric-percentile-lookup, step task#4 - DescribeAlarmsForMetric and inspect MetricAlarms[0].CreationId.
