Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs__logs.svcreplace-deployment-whose-container-never-passes-its-healthcheck-is-rejected-and-rolled-back
Issue-Key: ecs__logs.svcreplace-deployment-whose-container-never-passes-its-healthcheck-is-rejected-and-rolled-back|_poll_until#4|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Commit: 31f7b518324c5238b816f5187d2718623b5667f6

# A task's container healthStatus never leaves UNKNOWN, so an unhealthy deployment is never rejected or rolled back

> _Note: this issue was identified against the code base at commit `31f7b518324c5238b816f5187d2718623b5667f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTasks

## Description
Cause 1 of 1: container health checks are never evaluated.
AWS: a container declaring a healthCheck reports healthStatus HEALTHY once the probe passes and UNHEALTHY when it keeps failing; a service deployment whose task never becomes healthy is failed and rolled back to the previous task set (with deployment circuit breaker) rather than hanging.
Emulator: the revision-1 task reaches RUNNING but its container healthStatus stays 'UNKNOWN' for the whole 480s budget, so the deployment's rejection and rollback can never be observed.
Why: a client's deployment safety net (circuit breaker on unhealthy tasks) never triggers; an unhealthy revision looks the same as a healthy one.
Fix: container health-check evaluation and the resulting deployment rollback in the ecs crate's task/service state.
Traces: ecs__logs.svcreplace-deployment-whose-container-never-passes-its-healthcheck-is-rejected-and-rolled-back.

## Evidence
Expected: the revision-1 task reaches RUNNING and reports healthStatus HEALTHY. Actual: 'AssertionError: the revision-1 task reaches RUNNING and its container health probe reports HEALTHY not met within 480s. Last result: {... 'containers': [{... 'lastStatus': 'RUNNING', 'healthStatus': 'UNKNOWN', ...}] ...}'.

## Steps to Reproduce
Replay ecs__logs.svcreplace-deployment-whose-container-never-passes-its-healthcheck-is-rejected-and-rolled-back: register a task definition with a container healthCheck, create the service and poll DescribeTasks for healthStatus.
