Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ec2.xsvc-cloudwatch-3-alarm-ec2-reboot-action-reboots-instance-and-disabled-twin-untouched
Issue-Key: cloudwatch__ec2.xsvc-cloudwatch-3-alarm-ec2-reboot-action-reboots-instance-and-disabled-twin-untouched|_wait_for#20|Timeout|time.sleep(interval_s)
Issue-Commit: 67bb1f0baca61c1b5b95e0062b2898063e06b421

# cloudwatch__ec2: an alarm's ec2:reboot automate action never reboots its instance, so the reboot-action check reaches no verdict

> _Note: this issue was identified against the code base at commit `67bb1f0baca61c1b5b95e0062b2898063e06b421`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricAlarm

## Description
Cause 1 of 1: the CloudWatch `arn:aws:automate:<region>:ec2:reboot` alarm action is never executed against the EC2 instance it names, and no Action entry is written to the alarm's history, so the check exhausts the harness budget without a verdict.

AWS: an alarm whose AlarmActions contain the EC2 automate ARN `arn:aws:automate:<region>:ec2:reboot` and whose dimensions name an InstanceId executes that action when the alarm transitions to ALARM: the instance is really rebooted (it stays in EC2 state `running` but the guest boots again, so a boot-scoped identifier printed to the serial console appears a second time) and DescribeAlarmHistory records a HistoryItemType=Action entry whose HistorySummary reports the action Succeeded. An identical alarm created with ActionsEnabled=false evaluates the same way but executes nothing, leaving its instance on its first boot and no Action entry in its history. Smithy (cloudwatch): PutMetricAlarm.AlarmActions, ActionsEnabled, SetAlarmState; DescribeAlarmHistory.HistoryItemType enum includes `Action`. The automate-ARN semantics and the Succeeded summary are recorded behavior only.

Emulator: the check is reported `mismatch` with `Error: Check timed out after 60.0s`. Neither observable ever appears: the instance is not rebooted (no second boot identifier on the console output) and no Action history entry is written for the enabled alarm, so the enabled arm never satisfies its wait and the disabled twin's "untouched" assertion is never reached.

Why: a client that uses alarm automate actions for self-healing (reboot/recover/stop/terminate) gets an alarm that goes to ALARM and does nothing, with no Action record to explain it; the ActionsEnabled flag becomes indistinguishable from an action pipeline that does not exist, so both the remediation and the suppression contract are unverifiable.

Fix: the cloudwatch alarm action executor (the component that dispatches AlarmActions on a state transition) must recognise the `arn:aws:automate:<region>:ec2:<verb>` family and call through to the ec2 instance lifecycle (RebootInstances semantics) for the alarm's InstanceId dimension, and must append the Action entry to the alarm's history for both the success and the suppressed-by-ActionsEnabled cases. Related open issue 20261029T175114Z-ec2-led-lifecycle-scenarios-never-reach-their-observable.md covers the sibling terminate-action check but does not carry this check's replay key.

Traces: cloudwatch__ec2.xsvc-cloudwatch-3-alarm-ec2-reboot-action-reboots-instance-and-disabled-twin-untouched

## Evidence
Expected: after SetAlarmState(ALARM) on an alarm whose AlarmActions = [`arn:aws:automate:<region>:ec2:reboot`] and whose Dimensions name the instance, (a) the instance's console output shows a second, distinct boot identifier while EC2 reports State=running, and (b) DescribeAlarmHistory(HistoryItemType=Action) contains an entry reporting the action Succeeded. The ActionsEnabled=false twin shows exactly one boot identifier and no Action history entry.

Actual: `[mismatch] cloudwatch__ec2.xsvc-cloudwatch-3-alarm-ec2-reboot-action-reboots-instance-and-disabled-twin-untouched — Error: Check timed out after 60.0s`. No reboot and no Action history entry is observed, so the check never reaches either assertion and the emulator yields no verdict for the enabled or the suppressed arm.

## Resolution
The alarm action executor now carries out EC2 instance actions. An alarm action ARN that names one
— `arn:aws:automate:<region>:ec2:{stop|terminate|reboot|recover}` or the Simple Workflow form the
same documentation lists — is, when an alarm whose actions are enabled transitions to the state
whose action list carries it, that EC2 operation performed in the ARN's Region on the instance the
alarm's own watched metric is dimensioned by, and the alarm's history gains one `Action` item
carrying the outcome; an alarm with `ActionsEnabled=false` still executes nothing and records
nothing. So the enabled arm's instance is really rebooted (a new guest boot appended to the console
it has written, the instance keeping its id and its `running` state) and `DescribeAlarmHistory`
reports the `Succeeded` action. See
`services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/src/state/alarm_actions/ec2_instance_actions.rs`,
covered by `tests/all/ec2_instance_action_execution.rs`.

What the recorded check additionally waits for is a console line its launch's user data prints from
inside the GUEST on every boot, which needs guest execution the emulator has no runtime for — the
remaining half of
`issues/things-that-now-work/20261107T160042Z-fis-aws-ec2-reboot-instances-experiment-never-converges.md`,
whose own reboot check reads the same kind of marker.

## Steps to Reproduce
1. Launch two identical EC2 instances in a dedicated VPC/subnet with user data (cloud-boothook) that prints the kernel boot id to the serial console on every boot.
2. PutMetricAlarm twice, one per instance, on a metric dimensioned by InstanceId with AlarmActions=[`arn:aws:automate:<region>:ec2:reboot`] — one with ActionsEnabled=true, the twin with ActionsEnabled=false.
3. SetAlarmState(StateValue=ALARM) on both alarms.
4. Poll GetConsoleOutput for a second distinct boot id and DescribeAlarmHistory(HistoryItemType=Action) for the Succeeded entry.
5. Observe that neither ever appears; the check runs to the 60s harness cap.
