Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs__events__logs__sqs.ecs-container-oom-kill-reason-matches-eventbridge-task-state-change-detail ecs__events__logs__sqs.xsvc-ecs-2-ecs-target-without-passrole-dead-letters-the-event-and-launches-no-task
Issue-Key: ecs__events__logs__sqs.ecs-container-oom-kill-reason-matches-eventbridge-task-state-change-detail|_poll_until#5|Timeout|time.sleep(interval_s)
Issue-Key: ecs__events__logs__sqs.xsvc-ecs-2-ecs-target-without-passrole-dead-letters-the-event-and-launches-no-task|run#30|Timeout|found = _find_proof_line(ctx, log_group, published, marker=_GOOD)
Issue-Commit: b64c3d45927e2833db57dde0eaeae2a33a5b497f

# ecs__events__logs__sqs: a container OOM kill produces no task-state-change detail and a PassRole-less ECS target never dead-letters the event (2 causes)

> _Note: this issue was identified against the code base at commit `b64c3d45927e2833db57dde0eaeae2a33a5b497f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
Cause 1 of 2: an OOM-killed container produces no ECS Task State Change event to compare against.
AWS: when a container exceeds its memory limit the task stops, DescribeTasks reports the OOM stoppedReason, and the matching 'ECS Task State Change' event delivered to the SQS target carries the same reason in its detail.
Emulator: the check polls until the 60s budget expires — the task never runs or stops, and no task-state-change event is ever published to the bus, so the SQS queue stays empty.
Fix: the ECS task runtime (enforce the container memory limit and record the OOM stop reason) and the ECS task-state-change event publication into EventBridge.
Trace: ecs__events__logs__sqs.ecs-container-oom-kill-reason-matches-eventbridge-task-state-change-detail.

Cause 2 of 2: an ECS target whose role cannot pass the task role neither launches a task nor dead-letters the event.
AWS: EventBridge's RunTask invocation fails, so after the retry policy is exhausted the event is delivered to the rule target's dead-letter queue and no task is launched.
Emulator: the check polls until the 60s budget expires — no task, and nothing arrives on the DLQ.
Fix: the EventBridge target dispatch for ECS must evaluate iam:PassRole, treat the failure as an invocation failure, and apply the target's DeadLetterConfig.
Trace: ecs__events__logs__sqs.xsvc-ecs-2-ecs-target-without-passrole-dead-letters-the-event-and-launches-no-task.

## Evidence
Both checks end with `Error: Check timed out after 60.0s`. Expected (AWS): cause 1 — an SQS message whose body is an ECS Task State Change event whose detail's stoppedReason matches the OOM reason DescribeTasks reports; cause 2 — the original event body on the DLQ and an empty ListTasks.

## Steps to Reproduce
1. RunTask a container that allocates beyond its memory limit; subscribe an SQS queue to an 'ECS Task State Change' rule; poll the queue and DescribeTasks -> neither reports an OOM.
2. PutTargets an ECS target with a RoleArn lacking iam:PassRole and a DeadLetterConfig queue; PutEvents a matching event; poll the DLQ and ListTasks -> both empty.
