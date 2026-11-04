Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.container-workdir-user-and-ulimit-apply-inside-the-running-container ecs.container-depends-on-success-gates-dependent-start ecs.container-depends-on-complete-admits-nonzero-exit-unlike-success ecs.essential-container-exit-stops-task-and-sidecar ecs.run-task-container-override-exit-code ecs.run-task-large-environment-override-payload-round-trip ecs.run-task-task-role-override-replaces-task-definition-role ecs.task-role-credentials-endpoint-serves-task-role ecs.e2e-run-task-to-completion-and-cluster-cleanup ecs.e2e-service-deploy-scale-drain-teardown ecs.run-task-blocked-by-iam-policy-rejected
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs.container-workdir-user-and-ulimit-apply-inside-the-running-container|_poll_until#17|Timeout|last = probe()
Issue-Key: ecs.container-depends-on-success-gates-dependent-start|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Key: ecs.container-depends-on-complete-admits-nonzero-exit-unlike-success|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Key: ecs.essential-container-exit-stops-task-and-sidecar|_poll_until#15|Timeout|return last
Issue-Key: ecs.run-task-container-override-exit-code|_poll_until#18|Timeout|time.sleep(interval_s)
Issue-Key: ecs.run-task-large-environment-override-payload-round-trip|_wait_task_stopped#19|Timeout|return
Issue-Key: ecs.run-task-task-role-override-replaces-task-definition-role|_poll_until#20|Timeout|time.sleep(interval_s)
Issue-Key: ecs.task-role-credentials-endpoint-serves-task-role|_poll_until#38|Timeout|return last
Issue-Key: ecs.e2e-run-task-to-completion-and-cluster-cleanup|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Key: ecs.e2e-service-deploy-scale-drain-teardown|_poll_until#30|TimeoutError|raise TimeoutError("the polled condition: the run budget ended first; not graded")
Issue-Key: ecs.run-task-blocked-by-iam-policy-rejected|_poll_until#27|Timeout|time.sleep(interval_s)
Issue-Commit: b64c3d45927e2833db57dde0eaeae2a33a5b497f

# ECS containers never execute: no in-container behaviour (command, exit code, overrides, dependsOn, essential-exit, task-role credentials) is ever observable (1 cause)

> _Note: this issue was identified against the code base at commit `b64c3d45927e2833db57dde0eaeae2a33a5b497f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunTask

## Description
Cause 1 of 1: a task's containers are recorded but never run, so nothing the container itself would produce ever materialises.

AWS: a RunTask-launched task transitions PENDING -> RUNNING -> STOPPED; Smithy com.amazonaws.ecs#Task documents pullStartedAt/pullStoppedAt/startedAt/executionStoppedAt/stoppedAt/stopCode/stoppedReason and Container.exitCode, which AWS fills in from the real container's execution. containerOverrides (command, environment, cpu, memory) and the task-definition workingDirectory/user/ulimits take effect inside the container, dependsOn conditions SUCCESS/COMPLETE gate a dependent container's start, an essential container's exit stops the whole task, and the task metadata/credentials endpoint serves the taskRoleArn's credentials to the process.

Emulator: the task record is created and (when it advances at all) reports lastStatus without any container execution behind it. Every check that asserts on something only a running container can produce — the container's own stdout/exit status, an injected environment override being visible to the process, a dependent container starting after its dependency succeeded, a sidecar being stopped when the essential container exits, the task-role credentials endpoint answering — polls until the 60s budget expires with the condition never met. Containers are returned with lastStatus PENDING, no exitCode, and no network bindings.

Why: the emulator can be used to exercise the ECS control plane but not the workload. Any consumer that launches a task to do work (a batch job, a migration, a CI step, a smoke test) observes a task that is accepted and never does anything, and a consumer that reads exitCode/stoppedReason to decide success gets neither.

Fix: the ECS task runtime — the task-materialisation path in the RunTask/StartTask/service-scheduler handlers must actually start the container workload (applying containerOverrides and the task-definition runtime settings), then drive and record the container/task lifecycle (pull timestamps, RUNNING, exitCode, stopCode/stoppedReason) and expose the task-role credentials endpoint.

Traces: the failing check ids in trace_ids.

## Evidence
Expected (AWS): the task reaches lastStatus=RUNNING, its containers run, and DescribeTasks eventually reports the container's exitCode and the task's stopCode/stoppedReason; a dependent container starts only after its dependsOn condition is met; the credentials endpoint returns the task role's credentials.
Actual (emulator): every such check ends with `Error: Check timed out after 60.0s`, e.g. ecs.container-workdir-user-and-ulimit-apply-inside-the-running-container, ecs.container-depends-on-success-gates-dependent-start, ecs.essential-container-exit-stops-task-and-sidecar, ecs.run-task-container-override-exit-code, ecs.task-role-credentials-endpoint-serves-task-role. `ecs.e2e-service-deploy-scale-drain-teardown` -> TimeoutError: the polled condition: the run budget ended first; not graded. RunTask responses show containers with 'lastStatus': 'PENDING' and no exitCode.

## Steps to Reproduce
1. RegisterTaskDefinition with a container whose command writes a known string and exits with a known code (e.g. sh -c 'echo hi; exit 7').
2. RunTask it on a Fargate cluster.
3. Poll DescribeTasks: the task/container never reports a RUNNING->STOPPED transition with containers[0].exitCode=7, and nothing the container printed is observable.
4. Repeat with containerOverrides.environment, with a dependsOn SUCCESS pair, and with a non-essential sidecar beside an exiting essential container: none of those behaviours occur.
