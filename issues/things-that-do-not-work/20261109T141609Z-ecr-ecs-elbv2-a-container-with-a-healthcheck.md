Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr__ecs__elbv2.elbv2-health-check-reflects-ecs-task-status
Issue-Key: ecr__ecs__elbv2.elbv2-health-check-reflects-ecs-task-status|_poll_until#7|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s (last={last!r})")
Issue-Commit: 5252be52e0cc9f5d73bee5aa197523afb271707f

# ecr__ecs__elbv2: a container with a HEALTHCHECK never reports healthStatus HEALTHY — DescribeTasks keeps the container at UNKNOWN, so ELBv2 health can never track task health

> _Note: this issue was identified against the code base at commit `5252be52e0cc9f5d73bee5aa197523afb271707f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTasks

## Description
Cause 1 of 1: container health status never advances past UNKNOWN.
AWS: when a task definition's container declares a healthCheck, the ECS agent runs it and DescribeTasks reports the container's healthStatus transitioning UNKNOWN → HEALTHY (or UNHEALTHY), and the task's healthStatus follows its essential containers. Smithy (ECS): Container$healthStatus / Task$healthStatus target HealthStatus with values HEALTHY | UNHEALTHY | UNKNOWN.
Emulator: the container's healthStatus stays 'UNKNOWN' for the whole 300 s the check polls — the declared health check is never executed or never recorded, so HEALTHY is unreachable.
Why: a client that gates a rollout, an ALB target's health or an alarm on container health waits forever and then reports the workload unhealthy, even though the container is running; ELBv2 target health can never reflect ECS task health.
Fix: the ECS task runtime's container health-check evaluation and the Container/Task healthStatus projection in DescribeTasks.
Traces: ecr__ecs__elbv2.elbv2-health-check-reflects-ecs-task-status.

## Evidence
AssertionError: container health==HEALTHY not met within 300s (last=['UNKNOWN'])

Expected: the container's healthStatus reaches HEALTHY once its declared healthCheck passes, and the ALB target's health follows it.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Re-replay ecr__ecs__elbv2.elbv2-health-check-reflects-ecs-task-status.
3. Or: register a task definition whose container declares a healthCheck, run it, and poll DescribeTasks for containers[].healthStatus.
