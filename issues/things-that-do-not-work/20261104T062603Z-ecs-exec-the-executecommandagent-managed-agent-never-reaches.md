Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.exec-disabled-task-session-rejected-while-enabled-streams ecs.exec-file-write-visible-to-second-session ecs.exec-interactive-stdout-round-trip ecs.exec-session-resize-propagates-to-pty ecs.exec-targets-the-named-container-not-a-sibling ecs.execute-command-fails-when-not-enabled-at-launch ecs.execute-command-on-stopped-task-rejected ecs.execute-command-rejected-without-ssm-connection ecs.execute-command-requires-enabled-flag-rejected ecs.execute-command-server-managed-session ecs.execute-command-session-credentials-round-trip ecs.execute-command-session-expiry ecs.execute-command-succeeds-when-enabled-at-runtime ecs.execute-command-without-exec-enabled-rejected ecs.e2e-run-task-execute-command-session-lifecycle
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs.exec-disabled-task-session-rejected-while-enabled-streams|_wait_for_exec_agent#11|TimeoutError|raise TimeoutError(
Issue-Key: ecs.exec-file-write-visible-to-second-session|_wait_for_exec_agent#10|TimeoutError|raise TimeoutError(
Issue-Key: ecs.exec-interactive-stdout-round-trip|_wait_for_exec_agent#9|Timeout|raise AssertionError(
Issue-Key: ecs.exec-session-resize-propagates-to-pty|_wait_for_exec_agent#7|TimeoutError|raise TimeoutError(
Issue-Key: ecs.exec-targets-the-named-container-not-a-sibling|_wait_for_exec_agents#9|TimeoutError|raise TimeoutError(
Issue-Key: ecs.execute-command-fails-when-not-enabled-at-launch|run#45|Timeout|time.sleep(5)
Issue-Key: ecs.execute-command-on-stopped-task-rejected|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Key: ecs.execute-command-rejected-without-ssm-connection|run#10|Timeout|time.sleep(2)
Issue-Key: ecs.execute-command-requires-enabled-flag-rejected|run#51|Timeout|time.sleep(5)
Issue-Key: ecs.execute-command-server-managed-session|_poll_until#27|Timeout|time.sleep(interval_s)
Issue-Key: ecs.execute-command-session-credentials-round-trip|_wait_for_exec_agent_ready#18|Timeout|raise AssertionError(f"task stopped before becoming ready for exec: {last_task.get('stoppedReason')}")
Issue-Key: ecs.execute-command-session-expiry|_poll_until#20|Timeout|time.sleep(interval_s)
Issue-Key: ecs.execute-command-succeeds-when-enabled-at-runtime|run#7|Timeout|raise AssertionError(f"task stopped before exec agent connected: {task.get('stoppedReason')}")
Issue-Key: ecs.execute-command-without-exec-enabled-rejected|run#27|Timeout|time.sleep(5)
Issue-Key: ecs.e2e-run-task-execute-command-session-lifecycle|_poll_until#18|Timeout|time.sleep(interval_s)
Issue-Commit: b64c3d45927e2833db57dde0eaeae2a33a5b497f

# ECS Exec: the ExecuteCommandAgent managed agent never reaches RUNNING, so no ExecuteCommand session can be opened, streamed, resized or refused (1 cause)

> _Note: this issue was identified against the code base at commit `b64c3d45927e2833db57dde0eaeae2a33a5b497f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteCommand

## Description
Cause 1 of 1: ECS Exec is never usable because the per-container managed agent that backs it never starts.

AWS: Smithy com.amazonaws.ecs#ManagedAgent documents that "When the execute command feature is turned on, the managed agent name is ExecuteCommandAgent", with lastStatus/lastStartedAt/reason. On real AWS, a task launched with enableExecuteCommand=true (and a task role) reports a container managedAgents entry named ExecuteCommandAgent whose lastStatus becomes RUNNING; ExecuteCommand then returns a session (sessionId, streamUrl, tokenValue) that streams the container's PTY, honours a resize, targets the named container, and is refused with InvalidParameterException when the task was not launched with execute-command enabled or has stopped.

Emulator: containers are returned with 'managedAgents': [] and the agent never appears or reaches RUNNING, so every exec check — the positive stream/round-trip ones and the negative ones that need a working control to compare against — polls for the agent or the session until the budget expires. The replayed detail is `TimeoutError: ExecuteCommandAgent on arn:aws:ecs:...:task/<cluster>/<task-id> reaching RUNNING: the run budget ended first; not graded` and, for the rest, `Check timed out after 60.0s`.

Why: ECS Exec is one of the main reasons to reach into a running task. A client gets neither a session nor a refusal; it hangs. The negative contracts (session on a task without the flag, on a stopped task, without an SSM connection, after expiry) are equally unobservable, so a consumer cannot test its error handling either.

Fix: the ECS exec/managed-agent component — the task-materialisation path must register an ExecuteCommandAgent managed agent per container when enableExecuteCommand=true and drive it to RUNNING, and the ExecuteCommand handler must mint a session against it (and refuse per the documented conditions when it is absent).

Traces: the failing check ids in trace_ids.

## Evidence
Expected (AWS): DescribeTasks shows containers[*].managedAgents=[{name:'ExecuteCommandAgent', lastStatus:'RUNNING', lastStartedAt:<ts>}]; ExecuteCommand returns a session; ExecuteCommand on a task launched without the flag is refused with InvalidParameterException.
Actual (emulator): containers carry 'managedAgents': [] indefinitely. Replay details: `TimeoutError: ExecuteCommandAgent on arn:aws:ecs:us-west-2:000000000000:task/<cluster>/<task> reaching RUNNING: the run budget ended first; not graded` (ecs.exec-disabled-task-session-rejected-while-enabled-streams, ecs.exec-file-write-visible-to-second-session, ecs.exec-session-resize-propagates-to-pty, ecs.exec-targets-the-named-container-not-a-sibling) and `Check timed out after 60.0s` for the remaining exec/execute-command checks.

## Steps to Reproduce
1. RegisterTaskDefinition with a taskRoleArn; RunTask with enableExecuteCommand=true on a Fargate cluster.
2. Poll DescribeTasks for containers[0].managedAgents: the list stays empty, no ExecuteCommandAgent ever appears or reaches RUNNING.
3. Call ExecuteCommand against the task/container: no session is returned.
4. Repeat against a task launched WITHOUT enableExecuteCommand and against a stopped task: the documented refusals are not observable either.
