Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs__efs__logs.ecs-efs-volume-shared-across-two-fargate-tasks-and-readonly-mount-write-denied
Issue-Key: ecs__efs__logs.ecs-efs-volume-shared-across-two-fargate-tasks-and-readonly-mount-write-denied|_poll_until#5|Timeout|return last
Issue-Commit: b64c3d45927e2833db57dde0eaeae2a33a5b497f

# ecs__efs__logs: the shared-EFS Fargate scenario never reaches a verdict — a RunTask/log-delivery poll loop never converges and the check is killed at the 60s cap (1 cause)

> _Note: this issue was identified against the code base at commit `b64c3d45927e2833db57dde0eaeae2a33a5b497f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunTask

## Description
Cause 1 of 1: the end-to-end ECS+EFS+Logs scenario produces no verdict at all — the check is killed at the suite's 60s cap while polling, so none of the behaviour it asserts (an EFS volume shared across two Fargate tasks, and a readOnly mount refusing a write) can be observed.
AWS: recorded behavior only (this is a data-plane end-to-end scenario, not a single modelled operation). On AWS the scenario converges: ECS RunTask with launchType FARGATE and an awsvpc networkConfiguration returns a task that progresses through its lifecycle to lastStatus STOPPED with a stopCode other than TaskFailedToStart and a containers[0].exitCode; the container's stdout is delivered by the awslogs driver into the named CloudWatch Logs group under the configured stream prefix, so GetLogEvents/FilterLogEvents eventually yields the lines the container wrote (including the sentinel the check waits on); each awsvpc task is attached to its own ENI reported in the task's attachments details as networkInterfaceId; and a mountPoint with readOnly true makes the write fail inside the container with a "Read-only file system" error while reads still succeed. The scenario's polls are bounded: the check waits for the EFS file system and mount target and access point to reach `available`, for each task to reach STOPPED (interval 10s), and for the sentinel line to be DELIVERED to the log stream.
Emulator: the check never completes — it is cut off with "Check timed out after 60.0s" and reports no assertion result, so at least one of those poll loops never converges. The emulator serves the control-plane calls (CreateCluster, RegisterTaskDefinition, RunTask, the EFS creates) but never produces the terminal state the scenario waits on: a launched Fargate task that reaches STOPPED with an exit code, and/or the awslogs delivery of the container's output into the log group.
Why: a client that launches a Fargate task and waits for it to finish, or that reads the task's logs to learn what it did, waits forever: nothing ever reports completion and no log events arrive. Every downstream decision (did the task succeed, what did it write, was the read-only mount refused) is unreachable, and the whole shared-EFS claim is unverifiable against the emulator.
Fix: the ECS task lifecycle in the ecs service crate (the RunTask handler and the task record's state transitions — the task must advance to lastStatus STOPPED with stopCode/containers[].exitCode) together with the awslogs delivery path that writes a task container's output into the configured CloudWatch Logs group/stream. Several open issues describe neighbouring symptoms but none carries this check's replay key: 20261103T150504Z-ecs-task-records-omit-platform-and-sizing-fields (tasks never reach the stopped lifecycle), 20261104T034544Z-cloudwatch-ecs-ssm-xray-a-launched-fargate-task (a task's container produces no side effects), 20261104T044754Z-ecs-logs-secretsmanager-a-task-whose-container-secrets (no terminal lifecycle, no logs) and 20261104T040200Z-codebuild-ecr-ecs-elbv2-logs-the-codebuild-ecr.
Traces: ecs__efs__logs.ecs-efs-volume-shared-across-two-fargate-tasks-and-readonly-mount-write-denied.

## Evidence
Behavioral replay of simparity-behavioral-aws-v1 against the emulator built from this checkout: service ecs__efs__logs 0/1 (0%), 1 failed.

[mismatch] ecs__efs__logs.ecs-efs-volume-shared-across-two-fargate-tasks-and-readonly-mount-write-denied — Error: Check timed out after 60.0s. No assertion in the check body was reached, so the emulator returned no verdict for any of its claims.

The check's blocking waits, from its own source as the suite report prints it: _poll_until(... LifeCycleState == "available") for the EFS file system, mount target and access point; _run_and_wait() -> _poll_until(lambda: _describe_task(ecs, cluster, arn), lambda t: t["lastStatus"] == "STOPPED", timeout_s=_TASK_TIMEOUT_S, interval_s=10) for each of the three task arms A/B/C; and _stream_lines(ctx, log_group, task, sentinel), which waits until the sentinel line has been DELIVERED to the awslogs stream. One or more of these never converges, so the 60s cap fires before the first assertion (containers[0].exitCode == 0 for the writing task).

## Steps to Reproduce
1. git clone the repo; build and boot the emulator from this checkout.
2. Replay the behavioral suite simparity-behavioral-aws-v1 check ecs__efs__logs.ecs-efs-volume-shared-across-two-fargate-tasks-and-readonly-mount-write-denied. Observe "Check timed out after 60.0s" with no assertion outcome.
3. Minimal repro of the convergence gap: CreateCluster; CreateLogGroup; create an EFS file system + mount target + access point and wait for `available`; RegisterTaskDefinition with networkMode awsvpc, requiresCompatibilities [FARGATE], an efsVolumeConfiguration volume mounted at /mnt/shared and a container logConfiguration of logDriver awslogs pointing at that log group; then RunTask with launchType FARGATE, count 1, an awsvpcConfiguration and a containerOverrides command that echoes a sentinel.
4. Poll DescribeTasks for that task arn: expect lastStatus to reach STOPPED with a stopCode other than TaskFailedToStart and containers[0].exitCode set; observe that it never does within the check's budget.
5. Poll the log group for the task's stream (prefix "efs") and the sentinel line: expect the container's stdout lines to be delivered; observe no delivery.
6. Also expect each awsvpc task to report its own networkInterfaceId in attachments[].details, and a readOnly mountPoint to make an in-container append fail with "Read-only file system" while a read succeeds — neither is reachable while step 4 does not converge.
