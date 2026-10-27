Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ecr__ecs__logs.cloudwatch-alarm-no-data-when-ecs-service-metric-missing
Issue-Key: cloudwatch__ecr__ecs__logs.cloudwatch-alarm-no-data-when-ecs-service-metric-missing|run#15|Timeout|time.sleep(_POLL_INTERVAL_S)
Issue-Commit: d26de989ce4778e024797c8509c4713b425f0550

# An alarm over an ECS/ContainerInsights metric never reaches INSUFFICIENT_DATA and its positive control never reaches ALARM (1 cause)

> _Note: this issue was identified against the code base at commit `d26de989ce4778e024797c8509c4713b425f0550`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAlarms

## Description
One net-new root cause.

Cause 1 of 1: an alarm pointed at an ECS service dimension never produces an evaluated state, in either arm.
AWS: an alarm whose Dimensions name a non-existent ECS service receives no datapoints and settles in INSUFFICIENT_DATA — never ALARM — while an identically shaped control alarm fed real datapoints reaches ALARM with a StateReason beginning "Threshold Crossed". The two arms together are what proves missing data is handled as missing rather than as a breach. Recorded behavior: INSUFFICIENT_DATA on the no-data arm and ALARM on the control.
Emulator: the check polls both arms with declared budgets (_NEGATIVE_WINDOW_S=75.0, _CONTROL_BUDGET_S=240.0) and is killed at the harness 60 s cap — neither arm ever produces an evaluated state, so the emulator cannot be shown to treat a missing ECS service dimension as missing data rather than as a breach. This is the ECS-dimension instance of the alarm evaluation engine not running (filed for service `cloudwatch`); that issue does not carry this replay key.
Why: a client cannot tell a genuinely idle service from a mis-dimensioned alarm, and an alarm that AWS would hold at INSUFFICIENT_DATA is indistinguishable in the emulator from one that would fire.
Fix: the alarm evaluation engine's missing-data path, and AWS/ECS ContainerInsights metric publication for the control arm.
Traces: cloudwatch__ecr__ecs__logs.cloudwatch-alarm-no-data-when-ecs-service-metric-missing.

## Evidence
"Error: Check timed out after 60.0s". The assertions it never reaches: `assert state != "ALARM", "alarm fired ALARM against a dimension pair with no published data"`, `assert "INSUFFICIENT_DATA" in seen_states, f"alarm never reached INSUFFICIENT_DATA, saw {seen_states}"`, `assert control_state == "ALARM"`, `assert control_reason.startswith("Threshold Crossed")`.

## Steps to Reproduce
1. PutMetricAlarm over ECS/ContainerInsights with Dimensions naming a ClusterName/ServiceName pair that does not exist.
2. Poll DescribeAlarms for StateValue -> it never becomes INSUFFICIENT_DATA (and never anything else).
3. Create a control alarm on the same shape and PutMetricData breaching datapoints for it -> it never reaches ALARM; the check hangs to the 60 s cap.
