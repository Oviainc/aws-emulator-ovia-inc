Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs__events__logs.xsvc-ecs-2-ecs-target-runs-fargate-task-whose-awslogs-carry-the-event-token ecs__events__logs.xsvc-ecs-2-ecs-target-role-without-passrole-launches-no-task-until-passrole-is-granted
Issue-Key: ecs__events__logs.xsvc-ecs-2-ecs-target-runs-fargate-task-whose-awslogs-carry-the-event-token|run#26|Timeout|time.sleep(5)
Issue-Key: ecs__events__logs.xsvc-ecs-2-ecs-target-role-without-passrole-launches-no-task-until-passrole-is-granted|_await#25|Timeout|time.sleep(5)
Issue-Commit: 68ec0c9cae77cfbc133f0186a4c923306bef5402

# ecs__events__logs: 2 net-new causes — an EventBridge ECS target never launches a Fargate task (no awslogs output, no PassRole-gated distinction)

> _Note: this issue was identified against the code base at commit `68ec0c9cae77cfbc133f0186a4c923306bef5402`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTargets

## Description
Two net-new root causes; neither check was marked ALREADY FILED or OWNED by the replay report.

Cause 1 of 2: an EventBridge rule with an ECS Fargate target never launches a task, so neither the task's awslogs output nor its rule-derived DescribeTasks representation exists.
AWS: when a matching event arrives, EventBridge assumes the target's RoleArn and calls ECS RunTask with the target's EcsParameters; the InputTransformer-rendered event token reaches the container's overridden command, so the task's awslogs stream contains a line naming that token, and DescribeTasks reports the task with the rule-derived startedBy, launchType=FARGATE and the overridden command (Smithy: ECS RunTask input carries overrides: TaskOverride, launchType, startedBy, networkConfiguration). Recorded behavior only for delivery timing.
Emulator: the check is killed with "Check timed out after 60.0s" while waiting for the launched task — no task appears, no awslogs line carries the event token, and no startedBy/launchType/command representation is observable.
Why: event-driven ECS workloads silently do nothing. A client whose rule should run a task on every event sees the rule accepted and the target stored, but no task, no logs and no failure to diagnose.
Fix: the EventBridge rule-dispatch path for ECS targets (assume the target role, call RunTask with EcsParameters, render InputTransformer output into the overrides) plus the ECS task-execution/awslogs delivery the launched task depends on.
Traces: ecs__events__logs.xsvc-ecs-2-ecs-target-runs-fargate-task-whose-awslogs-carry-the-event-token.

Cause 2 of 2: the iam:PassRole gate on an EventBridge ECS target is not represented — neither the authorized control nor the unauthorized target produces a distinguishable outcome.
AWS: two ECS targets on one rule differing only in whether their role carries iam:PassRole behave differently: every event launches the control target's Fargate task, and the target whose role lacks iam:PassRole launches nothing; granting exactly iam:PassRole makes that same target launch a task for a post-repair event.
Emulator: the check is killed with "Check timed out after 60.0s" — the control target never launches a task either, so authorized and unauthorized targets are indistinguishable (both launch nothing) and the repair cannot be observed.
Why: a client cannot tell a missing iam:PassRole grant (a real, common misconfiguration) from a broken dispatcher, and after fixing the grant gets no confirmation that it worked.
Fix: the EventBridge ECS-target dispatch path must evaluate iam:PassRole on the target role before calling RunTask and must launch the task when the grant is present; on top of Cause 1.
Traces: ecs__events__logs.xsvc-ecs-2-ecs-target-role-without-passrole-launches-no-task-until-passrole-is-granted.

## Evidence
Expected (AWS): a matching event launches a Fargate task whose awslogs line names the InputTransformer-rendered event token, with DescribeTasks reporting the rule-derived startedBy, launchType=FARGATE and the overridden command; of two targets differing only in iam:PassRole, the control launches a task on every event and the other launches none until iam:PassRole is granted.
Actual (emulator): both checks are terminated by the harness with `Check timed out after 60.0s`; no task is ever launched by either target, so no awslogs line, no task representation and no authorized-vs-unauthorized difference is observable.

## Steps to Reproduce
1. Create an ECS cluster, a Fargate task definition using the awslogs driver, a CloudWatch Logs group, and two IAM roles that differ only in whether they carry iam:PassRole.
2. PutRule with a matching event pattern, then PutTargets with two ECS targets (EcsParameters + InputTransformer), one per role.
3. PutEvents a matching event.
4. Poll ListTasks/DescribeTasks and the log group → no task is launched by either target and no log line carries the event token; both checks time out at 60s.
