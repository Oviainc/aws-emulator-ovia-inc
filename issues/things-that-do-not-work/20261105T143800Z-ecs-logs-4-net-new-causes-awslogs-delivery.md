Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs__logs.awslogs-exec-role-scoped-to-one-group-is-denied-on-another ecs__logs.deep-ecs-guest-identity-awsvpc-task-reachable-on-its-eni-ip-only-while-its-sg-allows-it ecs__logs.ecs-readonly-mount-point-write-rejected-while-the-writer-sibling-still-writes ecs__logs.svcreplace-stopped-standalone-task-is-never-relaunched-while-a-service-managed-one-is-replaced
Issue-Key: ecs__logs.awslogs-exec-role-scoped-to-one-group-is-denied-on-another|_poll#7|Timeout|time.sleep(interval_s)
Issue-Key: ecs__logs.deep-ecs-guest-identity-awsvpc-task-reachable-on-its-eni-ip-only-while-its-sg-allows-it|_poll#3|Timeout|time.sleep(interval_s)
Issue-Key: ecs__logs.ecs-readonly-mount-point-write-rejected-while-the-writer-sibling-still-writes|_poll_until#7|Timeout|time.sleep(interval_s)
Issue-Key: ecs__logs.svcreplace-stopped-standalone-task-is-never-relaunched-while-a-service-managed-one-is-replaced|_poll_until#4|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Commit: 68ec0c9cae77cfbc133f0186a4c923306bef5402

# ecs__logs: 4 net-new causes — awslogs delivery, execution-role log-resource scoping, awsvpc ENI reachability/SG enforcement and read-only mount enforcement are all unobservable

> _Note: this issue was identified against the code base at commit `68ec0c9cae77cfbc133f0186a4c923306bef5402`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunTask

## Description
Four net-new root causes. None of these four checks was marked ALREADY FILED or OWNED. The open issue 20261104T062603Z-ecs-logs-the-awslogs-driver-never-delivers-awsvpc.md covers other checks in this family but does not carry these four replay keys.

Cause 1 of 4: an execution role whose logs grant is resource-scoped to one log group is not enforced per-group.
AWS: a task whose execution role grants logs:CreateLogStream/logs:PutLogEvents only on group A delivers its output to A; the identical task pointed at group B fails to start with a ResourceInitializationError naming the logs authorization failure, and group B stays empty; widening the grant to name B too lets the same task start. The resource scope alone is the blocker.
Emulator: the check is killed with "Check timed out after 60.0s" — the permitted task never delivers to group A either, so the allowed/denied distinction and the repair are never observable.
Why: a client cannot tell an over-narrow log-group grant from a broken log driver, and a task that should have been refused at start may be reported as healthy with no logs.
Fix: the awslogs delivery path and the execution-role authorization check performed at task start, evaluated against the specific log-group resource.
Traces: ecs__logs.awslogs-exec-role-scoped-to-one-group-is-denied-on-another.

Cause 2 of 4: an awsvpc task is not reachable on its own ENI address and its security group does not decide who reaches it.
AWS: an awsvpc task answers on its ENI's private address; revoking one client's ingress rule on the task's security group drops that client's connections (they time out) while a still-authorized client keeps getting through, and re-authorizing restores delivery.
Emulator: the check is killed with "Check timed out after 60.0s" — the task never answers on its ENI address at all, so no authorized baseline exists and neither the revoke nor the re-authorize effect is observable.
Why: security-group enforcement is the control a user relies on to isolate a task. Here it can neither be demonstrated nor trusted, and a task that should be reachable is not reachable at all.
Fix: awsvpc task networking — the task must be bound to and answer on a real ENI private address (see also the ENI-id divergence filed for ec2__ecs) and EC2 security-group rules must gate traffic to it.
Traces: ecs__logs.deep-ecs-guest-identity-awsvpc-task-reachable-on-its-eni-ip-only-while-its-sg-allows-it.

Cause 3 of 4: a readOnly mount point is not enforced, and the shared-volume read path cannot be shown to work.
AWS: one volume mounted into two containers of a task is writable where the mount is read-write and refuses writes with EROFS where mountPoints[].readOnly is true, while the reader still reads the writer's payload — which proves the mount is present at the moment its write is refused.
Emulator: the check is killed with "Check timed out after 60.0s" — no container writes, reads or refuses anything, because no container process runs.
Why: readOnly is a containment guarantee. A client setting it gets no enforcement and no evidence either way.
Fix: the task-execution path's volume/mountPoint handling (mount the volume into each container and honour readOnly), plus the container stdout delivery the check reads its evidence from.
Traces: ecs__logs.ecs-readonly-mount-point-write-rejected-while-the-writer-sibling-still-writes.

Cause 4 of 4: a container's startup line is never delivered to its awslogs stream, so service-managed replacement vs standalone non-replacement cannot be graded.
AWS: after one StopTask each, a desiredCount=1 service's task is replaced by a new RUNNING task while an identical standalone RunTask task is never relaunched; both containers deliver their boot line and their SIGTERM farewell line to their awslogs streams and exit with the handler's code 42.
Emulator: the check fails with TimeoutError: "'BOOT-<token>' delivered to <log-group>/<prefix>/app/<task-id>: the run budget ended first; not graded" — the boot sentinel never arrives in the task's log stream. (The log-group name, prefix, task id and sentinel token are per-run values; the defect is that no event is ever delivered to the stream.)
Why: a client that uses container logs as the liveness signal for a replacement never sees a new task come up, and cannot distinguish "service replaced my task" from "nothing happened".
Fix: the awslogs delivery path for container stdout (stream creation plus PutLogEvents), and the service-scheduler replacement of a stopped service-managed task.
Traces: ecs__logs.svcreplace-stopped-standalone-task-is-never-relaunched-while-a-service-managed-one-is-replaced.

## Evidence
Cause 1 — Expected: output in the granted log group; the same task pointed at a non-granted group fails to start (ResourceInitializationError) and that group stays empty; widening the grant lets it start. Actual: `Check timed out after 60.0s`; nothing is delivered to the granted group either.
Cause 2 — Expected: the awsvpc task answers on its ENI private address; revoking a client's SG ingress makes that client time out while an allowed client still succeeds. Actual: `Check timed out after 60.0s`; the task never answers at all.
Cause 3 — Expected: the read-write mount accepts a write, the readOnly mount refuses it with EROFS, and the reader reads the writer's payload. Actual: `Check timed out after 60.0s`; no container write, read or refusal occurs.
Cause 4 — Expected: the boot line appears in the task's awslogs stream, the stopped service task is replaced and the standalone task is not. Actual: TimeoutError: "'BOOT-<token>' delivered to <log-group>/<prefix>/app/<task-id>: the run budget ended first; not graded".

## Steps to Reproduce
Cause 1: create two log groups; give the execution role logs:CreateLogStream/logs:PutLogEvents on group A only; RunTask with awslogs→A, then the identical task with awslogs→B; poll both groups and the task's start outcome.
Cause 2: create a VPC/subnet/SG, RunTask an awsvpc task that serves a probe, connect from two clients, revoke one client's SG ingress, then re-authorize it.
Cause 3: RegisterTaskDefinition with one volume mounted into two containers (one mountPoint readOnly=true), RunTask, and read each container's write/read result from its log stream.
Cause 4: create a desiredCount=1 service and an identical standalone RunTask task sharing a log group; StopTask each; poll the log group for the replacement task's boot line and poll ListTasks for a relaunch.
