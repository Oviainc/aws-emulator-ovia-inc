Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ecs__sns__sqs.obs-ecs-1-alarm-notification-withheld-from-queue-without-sns-policy
Issue-Key: cloudwatch__ecs__sns__sqs.obs-ecs-1-alarm-notification-withheld-from-queue-without-sns-policy|_poll_until#26|Timeout|time.sleep(delay)
Issue-Commit: d26de989ce4778e024797c8509c4713b425f0550

# An ECS RunningTaskCount alarm never fires, so SNS fan-out to a queue with and without an SNS grant cannot be differentiated (1 cause)

> _Note: this issue was identified against the code base at commit `d26de989ce4778e024797c8509c4713b425f0550`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetAlarmState

## Description
One net-new root cause.

Cause 1 of 1: the alarm never transitions, so the SNS-policy differential is never exercised.
AWS: an alarm on an ECS service's ECS/ContainerInsights RunningTaskCount metric transitions OK->ALARM and fans one notification to both SQS subscribers of its SNS topic at once; the queue whose policy grants sns.amazonaws.com sqs:SendMessage receives a payload naming AlarmName, NewStateValue=ALARM and Trigger.MetricName=RunningTaskCount, while the queue with no such grant stays at depth 0 until its policy is repaired, after which it too receives one. Recorded behavior: both arms are observed, including the repair.
Emulator: the check polls for StateValue=ALARM with a declared 420 s budget and is killed at the harness 60 s cap — the alarm never transitions, so no notification is ever produced and neither the withheld arm nor the repaired arm can be distinguished from a system that simply delivers nothing. This is the ECS-metric instance of the alarm evaluation engine not running (filed for service `cloudwatch`); that issue does not carry this replay key.
Why: a client cannot verify that a queue missing its SNS grant is the reason a notification was withheld, because nothing is delivered either way — a silent misconfiguration becomes indistinguishable from correct operation.
Fix: the alarm evaluation engine and its action dispatch to SNS, plus AWS/ECS ContainerInsights metric publication for the monitored service.
Traces: cloudwatch__ecs__sns__sqs.obs-ecs-1-alarm-notification-withheld-from-queue-without-sns-policy.

## Evidence
"Error: Check timed out after 60.0s". The wait that never resolves: `_poll_until(lambda: _alarm_state(cw, alarm), lambda s: s == "ALARM", timeout_s=420.0, ...)`; the assertions beyond it include `assert payload.get("AlarmName") == alarm and payload.get("NewStateValue") == "ALARM"`, `assert _bad_depth() == 0, f"the queue with no SNS grant holds {_bad_depth()} message(s)"`, `assert repaired is not None, "the repaired queue never received a notification within 300s"`, `assert trigger.get("MetricName") == "RunningTaskCount"`.

## Steps to Reproduce
1. Create an ECS cluster/service with Container Insights enabled; create an SNS topic with two SQS subscribers, one queue with an sns.amazonaws.com sqs:SendMessage policy and one with none.
2. PutMetricAlarm over ECS/ContainerInsights RunningTaskCount with that topic as AlarmAction.
3. Poll DescribeAlarms for StateValue=ALARM -> never occurs; no message reaches either queue; the check hangs to the 60 s cap.
