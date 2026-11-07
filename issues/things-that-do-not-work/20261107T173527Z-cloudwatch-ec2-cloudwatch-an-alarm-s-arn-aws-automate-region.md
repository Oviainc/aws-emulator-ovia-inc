Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ec2.xsvc-cloudwatch-3-alarm-ec2-terminate-action-disabled-blocked-until-enabled
Issue-Key: cloudwatch__ec2.xsvc-cloudwatch-3-alarm-ec2-terminate-action-disabled-blocked-until-enabled|_wait_for#27|AssertionError|raise AssertionError(f"timed out after {budget_s:.0f}s waiting for {what}; last={last!r}")
Issue-Commit: edd8d27229a8bc99e308a4b32d393f13d6f32cb7

# CloudWatch: an alarm's arn:aws:automate:<region>:ec2:terminate action is never executed and no Action history item is recorded

> _Note: this issue was identified against the code base at commit `edd8d27229a8bc99e308a4b32d393f13d6f32cb7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetAlarmState

## Description
Cause 1 of 1: an alarm carrying the EC2 terminate automate action does not act on its instance when it enters ALARM, and its action attempt is not recorded.
AWS: an alarm whose AlarmActions contain `arn:aws:automate:<region>:ec2:terminate` and whose ActionsEnabled is true executes that action on each OK->ALARM transition (including one forced with SetAlarmState): the targeted instance moves to shutting-down/terminated, and DescribeAlarmHistory with HistoryItemType=Action records an item whose HistorySummary names the action and the instance id it was attempted against. An alarm with ActionsEnabled=false records no action attempt and leaves its instance running until EnableAlarmActions plus a fresh OK->ALARM transition. (Recorded behavior for the history wording; the CloudWatch model types AlarmHistoryItem.HistorySummary as free text, so the requirement is that the item exists and names the instance.)
Emulator: after SetAlarmState ALARM on the armed alarm, DescribeAlarmHistory(HistoryItemType='Action') stays empty for the full 120s budget (`last=[]`) and the instance is never terminated, so the positive control of the negative-delivery check cannot even be established. The disabled twin's behavior is therefore indistinguishable from the armed one. The open issue 20261107T064927Z-cloudwatch-ec2-an-alarm-s-ec2-reboot-automate.md reports the same gap for the `ec2:reboot` action; it does not carry this check's replay key, and the terminate action is a separate action handler.
Why: alarm-driven EC2 remediation (terminate on a failed health metric) silently does nothing, and ActionsEnabled — the switch operators rely on to arm/disarm remediation — has no observable effect either way.
Fix: the CloudWatch alarm state-transition path must dispatch `arn:aws:automate:<region>:ec2:*` actions (here TerminateInstances against the alarm's InstanceId dimension) when ActionsEnabled is true and append the corresponding Action item to the alarm's history (alarm action execution plus alarm-history state).
Traces: cloudwatch__ec2.xsvc-cloudwatch-3-alarm-ec2-terminate-action-disabled-blocked-until-enabled.

## Evidence
Expected: within the budget, DescribeAlarmHistory(AlarmName=<armed alarm>, HistoryItemType='Action') returns an item whose HistorySummary names the armed instance id, and DescribeInstances reports that instance in shutting-down/terminated.
Actual: `AssertionError: timed out after 120s waiting for sim2real-xsvc-cwterm-armed-<id>'s Action history to record its attempt; last=[]` — no action item at all, instance untouched (alarm/instance names are per-run values).

## Steps to Reproduce
1. Replay the behavioral suite and look under `### cloudwatch__ec2`.
2. Run two t3.micro instances; PutMetricAlarm twice on a custom namespace with Dimensions=[{InstanceId}], AlarmActions=['arn:aws:automate:<region>:ec2:terminate'], one with ActionsEnabled=true and one false.
3. SetAlarmState ALARM on both.
4. Poll DescribeAlarmHistory(HistoryItemType='Action') for the armed alarm — it stays empty — and DescribeInstances for the armed instance — it stays running.
