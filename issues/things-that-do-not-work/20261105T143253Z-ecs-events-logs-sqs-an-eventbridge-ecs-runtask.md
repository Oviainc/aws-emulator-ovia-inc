Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs__events__logs__sqs.xsvc-ecs-2-ecs-target-without-passrole-dead-letters-the-event-and-launches-no-task
Issue-Key: ecs__events__logs__sqs.xsvc-ecs-2-ecs-target-without-passrole-dead-letters-the-event-and-launches-no-task|run#30|Timeout|time.sleep(5)
Issue-Commit: 68ec0c9cae77cfbc133f0186a4c923306bef5402

# ecs__events__logs__sqs: an EventBridge ECS RunTask target whose role lacks iam:PassRole neither launches a task nor dead-letters the event with ERROR_CODE NO_PERMISSIONS

> _Note: this issue was identified against the code base at commit `68ec0c9cae77cfbc133f0186a4c923306bef5402`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTargets

## Description
One net-new root cause of service `ecs__events__logs__sqs`, grounded in the failing behavioral check in trace_ids. The replay report marked it neither ALREADY FILED nor OWNED. The open issue 20261104T062603Z-ecs-events-logs-sqs-a-container-oom-kill.md covers a different ECS runtime gap for this same service token and does not carry this check's replay key; it is named here so a fixer can close both together. This is not a refusal, so it does not reopen a resolved issue.

=== Cause 1 of 1: the ECS-target leg of an EventBridge rule neither launches the task nor dead-letters the undeliverable event ===
AWS: EventBridge's ECS `RunTask` target behaviour is not in the Smithy model (recorded behavior only; AWS references: 'Amazon ECS tasks as targets' and 'Event retry policy and using dead-letter queues' in the EventBridge User Guide). `PutTargets` with an `EcsParameters` target assumes the target's `RoleArn` to call `ecs:RunTask`, and that role must also hold `iam:PassRole` for the task definition's execution/task roles. Of two targets differing ONLY in `iam:PassRole`: the control target launches a task on the cluster and dead-letters nothing, while the target whose role lacks PassRole launches NOTHING and the event is sent to that target's dead-letter queue with the EventBridge DLQ message attributes — `ERROR_CODE` = `NO_PERMISSIONS`, `ERROR_MESSAGE`, and `RULE_ARN` / `TARGET_ARN` naming this rule and this target (the attribute names and the `NO_PERMISSIONS` code are fixed text; the ARNs are values from the rule and target being invoked). The dead-lettered body is the transformed input the target would have been invoked with.
Emulator: neither leg is observable. After `PutEvents` matching the rule, the control target launches no task and the no-PassRole target dead-letters nothing — the check blocks waiting first for the control task and then for the DLQ message and is killed at the harness ceiling: `Check timed out after 60.0s`. So the PassRole check is not enforced, the ECS target is not invoked, and the DLQ path for a permission-denied target delivery does not produce a message at all.
Why: a client that drives containers from events sees nothing happen and nothing fail: the task never starts, and because no DLQ message is produced there is no `ERROR_CODE`/`RULE_ARN` to diagnose from. A missing `iam:PassRole` — the single most common misconfiguration for this target type — is indistinguishable from a working rule, so the client ships a pipeline that silently does nothing.
Fix: the EventBridge target-invocation path needs an ECS `RunTask` arm that assumes the target's `RoleArn`, authorizes `ecs:RunTask` AND `iam:PassRole` for the task definition's roles, invokes RunTask on success, and on an authorization failure routes the transformed input to the target's `DeadLetterConfig` queue with the `ERROR_CODE` (`NO_PERMISSIONS`), `ERROR_MESSAGE`, `RULE_ARN` and `TARGET_ARN` message attributes.
Traces: ecs__events__logs__sqs.xsvc-ecs-2-ecs-target-without-passrole-dead-letters-the-event-and-launches-no-task.

## Evidence
ecs__events__logs__sqs.xsvc-ecs-2-ecs-target-without-passrole-dead-letters-the-event-and-launches-no-task [mismatch]:
  expected: of two ECS targets on one rule differing only in iam:PassRole, the control target launches a task on the cluster and its DLQ stays empty; the target without PassRole launches nothing and its DLQ receives the transformed input with message attributes ERROR_CODE = "NO_PERMISSIONS" and RULE_ARN / TARGET_ARN naming that rule and target.
  actual:   no task is launched by either target and no DLQ message is produced; the check spends its whole window waiting and the harness reports `Check timed out after 60.0s`.

Not reached and therefore unverified by this replay (same cause): the exact ERROR_MESSAGE text and the dead-lettered body's equality with the target's transformed input.

## Steps to Reproduce
Boot the emulator and replay ecs__events__logs__sqs.xsvc-ecs-2-ecs-target-without-passrole-dead-letters-the-event-and-launches-no-task from suite simparity-behavioral-aws-v1.

By hand:
1. ecs.CreateCluster; logs.CreateLogGroup; iam.CreateRole for the task execution role; ecs.RegisterTaskDefinition (FARGATE, awsvpc) using that execution role and the log group.
2. iam.CreateRole twice, both trusting events.amazonaws.com and both allowing ecs:RunTask on the task definition — one WITH iam:PassRole for the execution role, one WITHOUT.
3. sqs.CreateQueue twice (one DLQ per target) and allow events.amazonaws.com sqs:SendMessage on each.
4. events.PutRule with EventPattern {"source":["<source>"],"detail-type":["<detail type>"]}; events.PutTargets with two targets, each with EcsParameters (TaskDefinitionArn, LaunchType FARGATE, NetworkConfiguration), its own RoleArn from step 2, an InputTransformer, and its own DeadLetterConfig queue.
5. events.PutEvents matching the pattern.
6. ecs.ListTasks on the cluster — AWS: exactly one task, launched by the control target. Emulator: none.
7. sqs.ReceiveMessage(MessageAttributeNames=["All"]) on both DLQs — AWS: the no-PassRole target's DLQ holds the transformed input with ERROR_CODE "NO_PERMISSIONS" and RULE_ARN/TARGET_ARN naming that rule and target, and the control target's DLQ is empty. Emulator: both DLQs stay empty.
