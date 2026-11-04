Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ecs__ssm.e2e-diagnose-cloudwatch-alarm-firing-lifecycle cloudwatch__ecs__ssm.fargate-task-role-without-ssm-get-parameter-is-denied-the-latency-flag
Issue-Key: cloudwatch__ecs__ssm.e2e-diagnose-cloudwatch-alarm-firing-lifecycle|phase#11|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Key: cloudwatch__ecs__ssm.fargate-task-role-without-ssm-get-parameter-is-denied-the-latency-flag|_poll#30|Timeout|time.sleep(interval_s)
Issue-Commit: b64c3d45927e2833db57dde0eaeae2a33a5b497f

# cloudwatch__ecs__ssm: a deployed service-latency alarm is not describable, and a task role denied ssm:GetParameter never produces an observable denial (2 causes)

> _Note: this issue was identified against the code base at commit `b64c3d45927e2833db57dde0eaeae2a33a5b497f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAlarms

## Description
Cause 1 of 2: an alarm created during the deploy phase cannot be read back.
AWS: after the alarm is created (template- or API-driven), DescribeAlarms(AlarmNames=[<name>]) returns exactly that one alarm.
Emulator: DescribeAlarms returns an empty list, so the e2e lifecycle aborts in its first phase.
Why: any diagnose/remediate workflow that keys off an alarm it just created cannot proceed.
Fix: the alarm-creation write path (and, where the alarm comes from a stack, the CloudFormation AWS::CloudWatch::Alarm provider) must publish into the CloudWatch alarm store that DescribeAlarms reads.
Trace: cloudwatch__ecs__ssm.e2e-diagnose-cloudwatch-alarm-firing-lifecycle.

Cause 2 of 2: a Fargate task whose task role lacks ssm:GetParameter never shows the denial.
AWS: the container's GetParameter call is refused with AccessDeniedException, and the granted twin succeeds — the difference is observable from the task's own behaviour.
Emulator: the check polls until the 60s budget expires; neither the denial nor the control's success is observable because the task workload never runs and no IAM evaluation happens on the SSM call.
Fix: the ECS task runtime (so the container executes) and SSM's IAM authorization for the task-role principal.
Trace: cloudwatch__ecs__ssm.fargate-task-role-without-ssm-get-parameter-is-denied-the-latency-flag.

## Evidence
1. `AssertionError: [phase 1/6 deploy] alarm 'sim2real-flint-<suffix>-service-latency' should be described exactly once, got []` (the alarm name is per-run).
2. `Error: Check timed out after 60.0s`.

## Steps to Reproduce
1. Create a CloudWatch alarm on an ECS service metric (template or PutMetricAlarm), then DescribeAlarms by that name -> [].
2. Create two Fargate tasks, one with a task role granting ssm:GetParameter and one without, have each call GetParameter, and compare -> neither outcome is observable within the budget.
