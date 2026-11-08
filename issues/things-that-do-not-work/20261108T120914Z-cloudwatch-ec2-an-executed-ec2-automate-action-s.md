Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ec2.xsvc-cloudwatch-3-alarm-ec2-terminate-action-disabled-blocked-until-enabled cloudwatch__ec2.xsvc-cloudwatch-3-alarm-ec2-reboot-action-reboots-instance-and-disabled-twin-untouched
Issue-Key: cloudwatch__ec2.xsvc-cloudwatch-3-alarm-ec2-terminate-action-disabled-blocked-until-enabled|run#28|AssertionError|assert f"'{armed_id}'" in armed_actions[0]["HistorySummary"], (
Issue-Key: cloudwatch__ec2.xsvc-cloudwatch-3-alarm-ec2-reboot-action-reboots-instance-and-disabled-twin-untouched|_wait_for#20|Timeout|time.sleep(interval_s)
Issue-Commit: 6596661b7bed7b2c68eacfc83c0e7a703969540b

# cloudwatch__ec2: an executed EC2 automate action's alarm-history item does not name the instance, and the ec2:reboot action path never converges (2 causes)

> _Note: this issue was identified against the code base at commit `6596661b7bed7b2c68eacfc83c0e7a703969540b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetAlarmState

## Description
Two net-new root causes under '### cloudwatch__ec2'. Neither check is marked OWNED by the replay report. The open issue 20261107T173527Z-cloudwatch-ec2-cloudwatch-an-alarm-s-arn-aws-automate-region.md reports an earlier stage of the terminate path (no action executed, no history item at all); the emulator has moved past that — an Action item is now recorded — and the divergence below is the wording of that item, which that issue's replay key does not cover. 20261107T064927Z-cloudwatch-ec2-an-alarm-s-ec2-reboot-automate.md likewise does not carry the reboot check's current key.

Cause 1 of 2: the Action alarm-history item for an executed arn:aws:automate:<region>:ec2:* action does not name the instance the action acted on.
AWS: when an alarm with ActionsEnabled=true transitions OK->ALARM (including a transition forced with SetAlarmState) and its AlarmActions contain arn:aws:automate:<region>:ec2:terminate, CloudWatch executes the action and appends an alarm-history item with HistoryItemType='Action' whose HistorySummary names both the action and the instance it was attempted against — the instance id appears in the summary, quoted, taken from the alarm's InstanceId dimension. Fixed text: the 'Successfully executed action' phrasing plus the action ARN; variable parts: the region in the ARN and the instance id from the alarm's dimension. Smithy: cloudwatch AlarmHistoryItem.HistorySummary is a free-text String and DescribeAlarmHistory takes HistoryItemType=Action, so the model fixes the shape and the recording fixes the wording — recorded behavior for the instance-naming requirement.
Emulator: the item is recorded but names only the ARN: HistorySummary = 'Successfully executed action arn:aws:automate:us-west-2:ec2:terminate', with no instance id anywhere in it. The check fails with 'AssertionError: the Action history item does not name instance ...: "Successfully executed action arn:aws:automate:us-west-2:ec2:terminate"'.
Why: alarm history is the audit record operators read to prove which instance an automated remediation terminated. With the target absent, two alarms on two instances produce byte-identical history, so a client cannot attribute a termination to an alarm, and a disabled-vs-armed comparison cannot be made from history at all.
Fix: the alarm action-execution path that appends the Action history item must interpolate the acted-on instance id (from the alarm's InstanceId dimension) into HistorySummary (alarm-history state / alarm action dispatch).
Traces: cloudwatch__ec2.xsvc-cloudwatch-3-alarm-ec2-terminate-action-disabled-blocked-until-enabled.

Cause 2 of 2: the alarm ec2:reboot action scenario reaches no verdict — the reboot path does not converge.
AWS: an alarm whose AlarmActions contain arn:aws:automate:<region>:ec2:reboot and whose ActionsEnabled is true reboots its target instance on each OK->ALARM transition and records the corresponding Action history item, while an ActionsEnabled=false twin forced to ALARM at the same instant leaves its instance untouched and records no Action item. Recorded behavior only; the model types the action list as strings.
Emulator: the check never produces an assertion — it ends 'Check timed out after 180.0s', exhausting the harness cap in its poll loops. Hypothesis: the ec2:reboot automate action is never dispatched (the same gap the open reboot issue describes at an earlier stage), so the reboot evidence the check waits for never appears and the loop runs to its own deadline.
Why: alarm-driven reboot remediation silently does nothing, and because it hangs rather than refuses, a client waiting for the instance to bounce blocks indefinitely instead of seeing an error.
Fix: the CloudWatch alarm state-transition path must dispatch arn:aws:automate:<region>:ec2:reboot (RebootInstances against the alarm's InstanceId dimension) when ActionsEnabled is true, and record its Action history item (alarm action dispatch plus alarm-history state).
Traces: cloudwatch__ec2.xsvc-cloudwatch-3-alarm-ec2-reboot-action-reboots-instance-and-disabled-twin-untouched.

## Evidence
Cause 1 — Expected: DescribeAlarmHistory(AlarmName=<armed alarm>, HistoryItemType='Action') returns an item whose HistorySummary names the armed instance id (the id quoted, taken from the alarm's InstanceId dimension) alongside the executed action ARN. Actual: 'AssertionError: the Action history item does not name instance \'i-00000000000000001\': \'Successfully executed action arn:aws:automate:us-west-2:ec2:terminate\'' — the summary carries the ARN only. (The instance id and region are per-run/per-lease values; the requirement is that the instance id from the request/state appears in the message.)
Cause 2 — Expected: after SetAlarmState ALARM on the armed alarm, the armed instance reboots and an Action history item is recorded, while the ActionsEnabled=false twin's instance stays running with no Action item. Actual: 'Error: Check timed out after 180.0s' — no assertion reached; the reboot evidence never appears.

## Steps to Reproduce
1. Replay the behavioral suite and read the failures under '### cloudwatch__ec2'.
2. Cause 1: run two t3.micro instances; PutMetricAlarm twice on a custom namespace with Dimensions=[{Name:'InstanceId',Value:<id>}] and AlarmActions=['arn:aws:automate:<region>:ec2:terminate'], one ActionsEnabled=true and one false; SetAlarmState ALARM on both; read DescribeAlarmHistory(HistoryItemType='Action') for the armed alarm — the recorded HistorySummary names no instance.
3. Cause 2: repeat with AlarmActions=['arn:aws:automate:<region>:ec2:reboot'] and poll for the reboot — the check never settles and hits the harness cap.
