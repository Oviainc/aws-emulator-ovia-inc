Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ssm.xsvc-cloudwatch-3-alarm-ssm-opsitem-action-creates-opsitem-and-disabled-twin-blocked
Issue-Key: cloudwatch__ssm.xsvc-cloudwatch-3-alarm-ssm-opsitem-action-creates-opsitem-and-disabled-twin-blocked|_poll#13|AssertionError|raise AssertionError(f"timed out after {budget_s:.0f}s waiting for {what}; last={last!r}")
Issue-Commit: c1cb63d17f24ad84276bf0784afa9b177e8d858c

# cloudwatch__ssm: an alarm's SSM OpsItem action is never executed and no Action history item is recorded

> _Note: this issue was identified against the code base at commit `c1cb63d17f24ad84276bf0784afa9b177e8d858c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetAlarmState

## Description
Cause 1 of 1: a CloudWatch alarm whose AlarmActions entry is an SSM OpsItem ARN (arn:aws:ssm:<region>:<account>:opsitem:<severity>#CATEGORY=<category>) never performs the action when the alarm enters ALARM, and DescribeAlarmHistory never records the attempt. Failing check: cloudwatch__ssm.xsvc-cloudwatch-3-alarm-ssm-opsitem-action-creates-opsitem-and-disabled-twin-blocked.

AWS: PutMetricAlarm accepts an OpsItem ARN in AlarmActions. When an alarm with ActionsEnabled=true transitions into ALARM (including a transition forced by SetAlarmState), CloudWatch executes every entry of AlarmActions and records the attempt in the alarm's history as an AlarmHistoryItem whose HistoryItemType is Action (Smithy, cloudwatch: DescribeAlarmHistory input member HistoryItemType targets the HistoryItemType enum, output AlarmHistoryItems; AlarmHistoryItem carries HistoryItemType/HistorySummary/HistoryData). For an opsitem action, Systems Manager creates an OpsItem whose Severity and Category are the ones encoded in the ARN and whose Title names the alarm. An alarm created with ActionsEnabled=false performs no action on a forced transition and records no Action item; after EnableAlarmActions a fresh OK -> ALARM transition does perform it. Nothing in either model constrains the history wording, so the requirement here is the presence and type of the record, not one run's text.

Emulator: after the alarm is driven into ALARM, DescribeAlarmHistory for that alarm returns an empty AlarmHistoryItems list — polled for 60s, the last observation is []. No Action-type history item is ever written and no OpsItem is created, so the positive arm of the check cannot be graded and the ActionsEnabled=false negative arm has no positive control. Replay detail: 'AssertionError: timed out after 60s waiting for <alarm-name>'s Action history to record its attempt; last=[]' (the alarm name is a per-run generated value, not part of the requirement).

Why: a client that wires alarms to Systems Manager OpsItems sees an alarm that reaches ALARM and silently does nothing. Operational runbooks that key off the created OpsItem, and audits that read the alarm's Action history to prove whether an action fired, both read 'nothing happened' from an alarm AWS would have acted on — and the opposite assertion (an action correctly suppressed while ActionsEnabled=false) is indistinguishable from this defect.

Fix: the cloudwatch alarm state-transition/action-dispatch component — the path that evaluates a state change (and the SetAlarmState handler) must dispatch each AlarmActions entry, honour ActionsEnabled, and append an Action-type item to the alarm's history state; the SSM opsitem action target must create the corresponding OpsItem with the Severity/Category parsed from the action ARN. Related but distinct open issues for other action kinds: 20261107T173527Z-cloudwatch-ec2-cloudwatch-an-alarm-s-arn-aws-automate-region.md (ec2:terminate automate action) and 20261106T180709Z-cloudwatch-lambda-a-forced-alarm-transition-never-delivers.md (Lambda action); neither covers the SSM opsitem action or this check.

Traces: cloudwatch__ssm.xsvc-cloudwatch-3-alarm-ssm-opsitem-action-creates-opsitem-and-disabled-twin-blocked.

## Evidence
Expected (AWS): PutMetricAlarm(AlarmActions=[arn:aws:ssm:<region>:<account>:opsitem:3#CATEGORY=Availability], ActionsEnabled=True) then a forced OK -> ALARM transition yields (a) at least one DescribeAlarmHistory item with HistoryItemType='Action' for that alarm, and (b) an OpsItem with Severity '3', Category 'Availability' and a Title naming the alarm. The twin alarm with ActionsEnabled=False yields no Action item and no OpsItem until EnableAlarmActions plus a fresh OK -> ALARM transition.

Actual (emulator): DescribeAlarmHistory(AlarmName=<alarm>, HistoryItemType='Action') returns AlarmHistoryItems=[] indefinitely; no OpsItem is created. Replay output: 'AssertionError: timed out after 60s waiting for sim2real-xsvc-cwssm-armed-<suffix>'s Action history to record its attempt; last=[]' (alarm name is per-run).

## Steps to Reproduce
1. PutMetricAlarm with Namespace=<unique>, MetricName=Never, Statistic=Maximum, Period=60, EvaluationPeriods=1, Threshold=1.0, ComparisonOperator=GreaterThanThreshold, TreatMissingData=ignore, ActionsEnabled=True and AlarmActions=['arn:aws:ssm:<region>:<account-id>:opsitem:3#CATEGORY=Availability'].
2. Drive the alarm into ALARM (SetAlarmState with StateValue='ALARM').
3. Poll DescribeAlarmHistory(AlarmName=<alarm>, HistoryItemType='Action'): AWS records an Action item within seconds; the emulator returns an empty list for the full 60s budget.
4. List OpsItems created since the start of the run: AWS has one for the alarm, the emulator has none.
5. Repeat with ActionsEnabled=False, then EnableAlarmActions and a fresh OK -> ALARM: on AWS the action fires only after the re-enable; on the emulator neither arm produces anything, so the negative arm has no positive control.
