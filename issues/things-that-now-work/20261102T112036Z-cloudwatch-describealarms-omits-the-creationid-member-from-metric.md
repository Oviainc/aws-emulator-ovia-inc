Issue-Source: trace-sweep-divergence
Issue-Trace: cloudwatch-p90-latency-alarm-round-trips-extended-statistic-and-omits-statistic
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudwatch-p90-latency-alarm-round-trips-extended-statistic-and-omits-statistic
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch|DescribeAlarms|2xx->2xx|$.MetricAlarms[*].CreationId
Issue-Commit: 7923db9e5c097e7a9eee0c015b735053c707d1d4

# CloudWatch DescribeAlarms omits the CreationId member from metric alarms

> _Note: this issue was identified against the code base at commit `7923db9e5c097e7a9eee0c015b735053c707d1d4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAlarms

## Description
This issue carries ONE cause.

Cause 1 of 1: DescribeAlarms omits MetricAlarm.CreationId.
  AWS: recorded behavior only for this member - the Smithy MetricAlarm structure in this checkout (com.amazonaws.cloudwatch#MetricAlarm) declares AlarmName, AlarmArn, StateReasonData, StateTransitionedTimestamp and the rest, but not CreationId; the recorded DescribeAlarms response nevertheless carries a CreationId on each MetricAlarm, formed as '<uuid>/<epoch-millis>' (for example the value seen elsewhere in the corpus, 'a7d21649-5866-4ce2-b02d-f8f27186aa4e/1788395461621'). The status is 200 in both the recording and the emulator; the only divergence is the missing member.
  Emulator: the MetricAlarms[0] object it returns has no CreationId member at all.
  Why: a client that round-trips an alarm description - reading DescribeAlarms and feeding the result back into tooling, or correlating an alarm with the PutMetricAlarm call that created it - finds the member absent and either drops the correlation or fails a strict deserialisation. The uuid and epoch-millis in any one recording are per-run values; the requirement is that the member is present on every returned MetricAlarm and has that two-part form.
  Fix: the MetricAlarm response serializer and the alarm state record that must hold a creation id assigned at PutMetricAlarm time.
  Traces: cloudwatch-p90-latency-alarm-round-trips-extended-statistic-and-omits-statistic#task#2.

Note on scope: this divergence is reported under the service token "cloudwatch" (not "monitoring"). The open issue "CloudWatch alarm reads omit CreationId, Dimensions and StateTransitionedTimestamp" (20261029T203422Z-monitoring-cloudwatch-alarm-reads-omit-creationid-dimensions-and-statetransitionedtimestamp.md) is filed under "monitoring" and does not carry this fixture's replay key, so this cluster is filed net-new and must be closed on its own key even though the underlying serializer is likely shared.

## Evidence
Expected status 200 with $.MetricAlarms[0].CreationId present (a shaped '<uuid>/<epoch-millis>' value); actual status 200 with $.MetricAlarms[0].CreationId absent (None). Replay divergence key: service=cloudwatch|DescribeAlarms|status=200->200|keys=$.MetricAlarms[0].CreationId.

## Steps to Reproduce
1. Boot the emulator and replay the wire corpus (replay_wire_corpus).
2. Fixture cloudwatch-p90-latency-alarm-round-trips-extended-statistic-and-omits-statistic, step task#2: create a metric alarm with ExtendedStatistic p90 and no Statistic, then call DescribeAlarms for it.
3. Inspect MetricAlarms[0] in the 200 response: CreationId is expected to be present in the '<uuid>/<epoch-millis>' form but is absent.
