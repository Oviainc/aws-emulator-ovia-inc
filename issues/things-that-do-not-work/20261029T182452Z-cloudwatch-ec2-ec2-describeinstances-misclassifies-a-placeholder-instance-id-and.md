Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ec2.alarm-ec2-action-requires-instance-metric cloudwatch__ec2.xsvc-cloudwatch-3-alarm-ec2-terminate-action-disabled-blocked-until-enabled
Issue-Key: cloudwatch__ec2|DescribeInstances|InvalidInstanceID.NotFound
Issue-Key: cloudwatch__ec2.xsvc-cloudwatch-3-alarm-ec2-terminate-action-disabled-blocked-until-enabled|_wait_for#27|Timeout|time.sleep(interval_s)
Issue-Commit: ec258a82c643322f3f1b10a7ea424abc4c176038

# EC2 DescribeInstances misclassifies a placeholder instance id, and an EC2 terminate alarm action never becomes observable once enabled

> _Note: this issue was identified against the code base at commit `ec258a82c643322f3f1b10a7ea424abc4c176038`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstances

## Description
This issue bundles 2 net-new root causes. Both must be closed before submitting.

Cause 1 of 2: DescribeInstances answers `InvalidInstanceID.NotFound` for an id the recorded AWS behaviour rejects as `InvalidInstanceID.Malformed`.
AWS: recorded behavior only (the ec2 model declares the error shapes but not which id forms map to which). For the placeholder id form this check builds — `i-` followed by 17 hex characters taken from a UUID — the recorded refusal is error code `InvalidInstanceID.Malformed` with the message `Invalid id: "<instanceId>"` (fixed text `Invalid id: ` plus the id from the request, in double quotes).
Emulator: error code `InvalidInstanceID.NotFound` with the message `The instance ID '<instanceId>' does not exist`.
Why: a caller distinguishing "this id is syntactically unusable, fix the caller" from "this id is well-formed but gone, re-resolve it" takes the wrong branch; the two codes drive different retry behaviour. Note the first half of this check — PutMetricAlarm refusing an `arn:aws:automate:<region>:ec2:stop` action on a metric with no InstanceId dimension, with ValidationError and the message `Invalid use of EC2 action. An EC2 action can only be used if the alarm is monitoring an EC2 instance.` — already passes; only the id classification diverges.
Fix: the DescribeInstances handler's instance-id parsing/validation and its error mapping.
Traces: cloudwatch__ec2.alarm-ec2-action-requires-instance-metric.

Cause 2 of 2: an EC2 terminate alarm action does not become observable when ActionsEnabled flips from false to true.
AWS: recorded behavior only. An alarm carrying an `arn:aws:automate:<region>:ec2:terminate` action with ActionsEnabled=false dispatches nothing when it breaches; after EnableAlarmActions the same breach dispatches the action, and the twin alarm that stays disabled remains untouched.
Emulator: the check exhausts its 60s budget — the awaited post-enable effect never arrives, so the disabled→enabled gate cannot be observed.
Why: the ActionsEnabled gate on EC2 automate actions is unverifiable; a customer cannot test that disabling actions truly blocks a terminate, nor that enabling them restores it.
Fix: the alarm action dispatcher's handling of `arn:aws:automate:` pseudo-ARNs and the ActionsEnabled gate in the alarm state engine.
Traces: cloudwatch__ec2.xsvc-cloudwatch-3-alarm-ec2-terminate-action-disabled-blocked-until-enabled.

## Evidence
Cause 1 — cloudwatch__ec2.alarm-ec2-action-requires-instance-metric:
  AssertionError: resolving the placeholder instance id i-05fab949dc1640878 raised ('InvalidInstanceID.NotFound', "The instance ID 'i-05fab949dc1640878' does not exist"), expected ('InvalidInstanceID.Malformed', 'Invalid id: "i-05fab949dc1640878"')
(The id value itself is generated per run — `i-` + uuid4().hex[:17]; the requirement is the code/message template, not this literal id.)

Cause 2 — cloudwatch__ec2.xsvc-cloudwatch-3-alarm-ec2-terminate-action-disabled-blocked-until-enabled: `Error: Check timed out after 60.0s`, with no post-enable action effect observed.

## Steps to Reproduce
Replay the two check ids. The first creates a dimensionless AWS/EC2 alarm with an ec2:stop action (must be refused), then calls DescribeInstances with a generated `i-<17 hex>` id and asserts the error code and message. The second creates two alarms carrying ec2:terminate actions, one with ActionsEnabled=false, drives a breach, calls EnableAlarmActions, and polls for the action effect on the enabled alarm only.
