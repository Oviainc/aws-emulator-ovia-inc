Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs__events__logs.xsvc-ecs-2-ecs-target-role-without-passrole-launches-no-task-until-passrole-is-granted ecs__events__logs.xsvc-ecs-2-ecs-target-runs-fargate-task-whose-awslogs-carry-the-event-token
Issue-Key: ecs__events__logs.xsvc-ecs-2-ecs-target-role-without-passrole-launches-no-task-until-passrole-is-granted|_await#25|Timeout|hit = _find_proof_line(ctx, log_group, tokens_fn(), marker=marker)
Issue-Key: ecs__events__logs.xsvc-ecs-2-ecs-target-runs-fargate-task-whose-awslogs-carry-the-event-token|run#26|Timeout|found = _find_proof_line(ctx, log_group, published)
Issue-Commit: b64c3d45927e2833db57dde0eaeae2a33a5b497f

# ecs__events__logs: an EventBridge ECS target never launches a task, so neither the iam:PassRole gate nor the launched task's awslogs output is observable (1 cause)

> _Note: this issue was identified against the code base at commit `b64c3d45927e2833db57dde0eaeae2a33a5b497f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
Cause 1 of 1: a matched event never results in a running ECS task, so nothing downstream of the EventBridge-to-ECS target can be observed.

AWS: a PutEvents entry matching a rule with an ECS target makes EventBridge assume the target RoleArn and call RunTask. If the role lacks iam:PassRole for the task/execution role, no task is launched (and the failure is observable); once PassRole is granted, the task launches and its container's stdout — including the event token the rule passed through — appears in the configured CloudWatch Logs stream.

Emulator: both checks poll until the 60s budget expires. No task is launched in either arm, so the denied arm cannot be distinguished from the granted one and no awslogs event ever carries the event token.

Why: the event-driven ECS pattern (a rule that runs a task) cannot be validated at all, and the iam:PassRole requirement — the usual reason this pattern fails in production — is invisible.

Fix: the EventBridge target dispatch for ECS (assume the target role, evaluate iam:PassRole, call RunTask), plus the ECS task runtime and its awslogs delivery so the launched task's output is observable.

Traces: ecs__events__logs.xsvc-ecs-2-ecs-target-role-without-passrole-launches-no-task-until-passrole-is-granted, ecs__events__logs.xsvc-ecs-2-ecs-target-runs-fargate-task-whose-awslogs-carry-the-event-token.

## Evidence
Expected (AWS): without iam:PassRole, ListTasks for the rule's cluster stays empty and the failure is reported; with it, a task runs and GetLogEvents shows the event token in the task's awslogs stream.
Actual (emulator): `Error: Check timed out after 60.0s` on both checks — no task, no log event, no refusal.

## Steps to Reproduce
1. PutRule + PutTargets with an ECS target whose RoleArn lacks iam:PassRole; PutEvents a matching event; poll ListTasks -> empty, with no observable failure.
2. Grant iam:PassRole and repeat; poll ListTasks and the awslogs group for the event token -> neither appears.
