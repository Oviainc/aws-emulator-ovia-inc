Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__lambda__logs.cli-put-metric-alarm-describe-lambda-workflow
Issue-Key: cloudwatch__lambda__logs.cli-put-metric-alarm-describe-lambda-workflow|run#23|KeyError|new_states = [json.loads(h["HistoryData"])["newState"]["stateValue"] for h in history]
Issue-Commit: 048778663a26e363156c6db724999c4fb229af36

# CloudWatch DescribeAlarmHistory items omit the HistoryData member

> _Note: this issue was identified against the code base at commit `048778663a26e363156c6db724999c4fb229af36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAlarmHistory

## Description
Cause 1 of 1: alarm-history items are served without HistoryData.
AWS: each AlarmHistoryItem carries AlarmName, Timestamp, HistoryItemType, HistorySummary and HistoryData. Smithy (cloudwatch): AlarmHistoryItem.HistoryData -> HistoryData, @length 1..4095, documented as 'Data about the alarm, in JSON format.' - it is the JSON record of the configuration update or state transition.
Emulator: the items returned by DescribeAlarmHistory have no HistoryData member, so a consumer reading item['HistoryData'] raises KeyError.
Why: clients that inspect the history JSON (to learn the old/new state or the configuration diff that produced a history entry) cannot work against the emulator, and the response does not satisfy the modeled shape.
Fix: the DescribeAlarmHistory serializer/state in the cloudwatch service crate - record the JSON history payload when an alarm is created, updated or transitions, and emit it as HistoryData.
Traces: cloudwatch__lambda__logs.cli-put-metric-alarm-describe-lambda-workflow.

## Evidence
KeyError: 'HistoryData' raised while reading an item from DescribeAlarmHistory after PutMetricAlarm. Expected: each AlarmHistoryItem includes HistoryData (JSON string, length 1..4095) alongside HistorySummary/HistoryItemType.

## Steps to Reproduce
aws cloudwatch put-metric-alarm (any alarm), then aws cloudwatch describe-alarm-history --alarm-name <name> and read AlarmHistoryItems[0].HistoryData -> member absent.
