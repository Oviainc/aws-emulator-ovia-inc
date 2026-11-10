Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.capacity-provider-ops-on-missing-cluster-not-found ecs.container-health-check-transitions-healthy-and-unhealthy ecs.run-task-fargate-count-three-runs-every-task-to-its-exit-code-or-refuses-honestly ecs.run-task-with-an-unpullable-image-reports-failed-to-start-not-running
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs.capacity-provider-ops-on-missing-cluster-not-found|run#7|AssertionError|assert code == "ClusterNotFoundException", f"CreateCapacityProvider: got {code}"
Issue-Key: ecs.container-health-check-transitions-healthy-and-unhealthy|_poll_until#16|Timeout|time.sleep(interval_s)
Issue-Key: ecs.run-task-fargate-count-three-runs-every-task-to-its-exit-code-or-refuses-honestly|run#21|Timeout|assert statuses == ["STOPPED"] * _COUNT, (
Issue-Key: ecs.run-task-with-an-unpullable-image-reports-failed-to-start-not-running|_run_to_stopped#11|Timeout|raise AssertionError(
Issue-Commit: b11785bf9bf8af57a00ad7e4d814b1c5f1d66117

# ecs: 3 net-new causes — CreateCapacityProvider answers ClientException for a missing cluster instead of ClusterNotFoundException, container health checks still never run, and Fargate tasks never reach STOPPED with a stopCode/exitCode

> _Note: this issue was identified against the code base at commit `b11785bf9bf8af57a00ad7e4d814b1c5f1d66117`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTasks

## Description
Three root causes in the ecs service, each to be closed.

Cause 1 of 3: CreateCapacityProvider with a `cluster` that does not exist is refused with the wrong error class and message.
AWS: Smithy (ecs) CreateCapacityProvider takes `name` (required) plus optional `cluster`, `autoScalingGroupProvider`/`managedInstancesProvider`, and models ClusterNotFoundException (HTTP 400, alongside ClientException, InvalidParameterException, LimitExceededException, UnsupportedFeatureException, UpdateInProgressException). When `cluster` names no existing cluster, the cluster is resolved first and the request is refused with ClusterNotFoundException whose message is the fixed text `Cluster not found.` (no request values interpolated — ClusterNotFoundException is documented as 'The specified cluster wasn't found').
Emulator: the call is refused with error code `ClientException` instead. The check asserts the code first, so only CreateCapacityProvider is proven here; the same request family (DescribeCapacityProviders/UpdateCapacityProvider/PutClusterCapacityProviders with a nonexistent `cluster`) is asserted later in the same check and was never reached.
Why: ClientException and ClusterNotFoundException are different contracts — a client that creates the cluster on demand, or that distinguishes a retriable client error from 'this cluster does not exist', takes the wrong branch and reports a generic failure instead of a missing cluster.
Fix: cluster resolution and error construction in the create_capacity_provider handler of the ECS crate — resolve the request's `cluster` before validating the provider body and raise ClusterNotFoundException with AWS's wording.
Traces: ecs.capacity-provider-ops-on-missing-cluster-not-found.

Cause 2 of 3: a declared container healthCheck is never executed, so healthStatus never leaves UNKNOWN — and the scenario now burns its full 600 s budget rather than failing its wait.
AWS: Smithy (ecs) ContainerDefinition.healthCheck (HealthCheck: command, interval, timeout, retries, startPeriod) is run by the agent inside the container, and DescribeTasks reports Container.healthStatus and Task.healthStatus from the HealthStatus enum {HEALTHY, UNHEALTHY, UNKNOWN}: a probe exiting 0 drives the container (and a task whose only essential container is healthy) to HEALTHY, a probe that keeps failing drives it to UNHEALTHY after `retries`, a container with no healthCheck stays UNKNOWN, and a standalone (non-service) task is NOT stopped for being unhealthy.
Emulator: healthStatus stays UNKNOWN forever; the wait for HEALTHY never completes and the runner kills the check at 600 s.
Why: health status is what ECS uses to gate dependsOn, replace service tasks, judge deployments and feed load-balancer target health. Pinned at UNKNOWN, a failing container is indistinguishable from an unprobed one and no health-driven behaviour can be relied on.
Fix: the ECS container health-check executor and the task/container state it writes (task state machine plus the DescribeTasks serializer), honouring startPeriod and retries.
Traces: ecs.container-health-check-transitions-healthy-and-unhealthy. (Open issue 20261109T144605Z-ecs-10-net-new-causes-container-health-checks.md describes this behaviour, but its replay key for this check was the _poll_until AssertionError; the check now times out at the runner budget and is no longer keyed to it.)

Cause 3 of 3: an accepted Fargate RunTask never reaches a terminal state — no task ever reports STOPPED with a stopCode and a container exitCode, for either a normal exit or a failed image pull.
AWS: Smithy (ecs) Task carries lastStatus, stopCode (TaskStopCode enum: EssentialContainerExited, TaskFailedToStart, UserInitiated, ...), stoppedReason and containers[].exitCode. A task whose only essential container runs `exit 7` stops promptly with lastStatus STOPPED, stopCode EssentialContainerExited and containers[].exitCode 7, and ListTasks(desiredStatus='STOPPED') then names exactly those tasks; a task whose image tag does not exist in the referenced repository either is refused synchronously with a structured reason and starts nothing, or stops with stopCode TaskFailedToStart, a stoppedReason naming the unresolvable image, no startedAt and no exitCode.
Emulator: neither terminal state is ever produced. RunTask(count=3) is accepted, DescribeTasks keeps reporting the tasks non-STOPPED for the whole window, and the runner kills both checks at 60 s; a task reported RUNNING that runs nothing is indistinguishable from real success.
Why: the exit code and stop reason are the only machine-readable result of a task. A client that runs a batch job, waits for it to stop and reads its exit code (or that must distinguish 'could not pull the image' from 'ran and failed') waits forever and then cannot tell what happened.
Fix: the ECS task lifecycle — the task runtime that executes the container and its exit, and the task state machine/serializer that must set lastStatus=STOPPED, stopCode, stoppedReason and containers[].exitCode (including the TaskFailedToStart path for an unresolvable image).
Traces: ecs.run-task-fargate-count-three-runs-every-task-to-its-exit-code-or-refuses-honestly, ecs.run-task-with-an-unpullable-image-reports-failed-to-start-not-running. (Open issue 20261104T062603Z-ecs-containers-never-execute-no-in-container-behaviour.md describes the same family but carries neither of these checks.)

## Evidence
Cause 1 — ecs.capacity-provider-ops-on-missing-cluster-not-found: `AssertionError: CreateCapacityProvider: got ClientException` at `assert code == "ClusterNotFoundException"`. Expected (code, message) = ('ClusterNotFoundException', 'Cluster not found.') for CreateCapacityProvider(name=<unused>, cluster=<nonexistent cluster name>, autoScalingGroupProvider={...}); actual code 'ClientException'. The fixed text is `Cluster not found.`; nothing in it is taken from the request.

Cause 2 — ecs.container-health-check-transitions-healthy-and-unhealthy: `Error: Check timed out after 600.0s` while waiting for `the passing container health check drives the container to HEALTHY`. Expected: DescribeTasks containers[name='app'].healthStatus == 'HEALTHY' and Task.healthStatus == 'HEALTHY' with lastStatus 'RUNNING' for a CMD-SHELL probe that exits 0; the failing-probe task reaching 'UNHEALTHY' while its sibling container with no healthCheck stays 'UNKNOWN' and the standalone task stays RUNNING. Actual: healthStatus stays 'UNKNOWN', no verdict inside the budget.

Cause 3 — ecs.run-task-fargate-count-three-runs-every-task-to-its-exit-code-or-refuses-honestly: `Error: Check timed out after 60.0s`. Expected after RunTask(count=3, launchType=FARGATE) of a definition whose only container runs `sh -c "exit 7"`: all three tasks reach lastStatus 'STOPPED' with stopCode 'EssentialContainerExited' and containers[].exitCode 7, and ListTasks(desiredStatus='STOPPED') names exactly those three ARNs. Actual: the tasks never report STOPPED and the run is killed. ecs.run-task-with-an-unpullable-image-reports-failed-to-start-not-running: `Error: Check timed out after 60.0s`; expected either an honest synchronous refusal (error code + 4xx/5xx + message, with ListTasks empty in every desiredStatus) or lastStatus 'STOPPED' with stopCode 'TaskFailedToStart', a stoppedReason naming the missing tag, no startedAt and no exitCode. Actual: neither; the task never settles.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Re-replay the four checks in trace_ids (service ecs).
3. Cause 1: ecs CreateCapacityProvider(name='cp-x', cluster='no-such-cluster-x', autoScalingGroupProvider={autoScalingGroupArn: <any ASG arn>, managedScaling: {status: 'DISABLED'}}) and read Error.Code/Error.Message.
4. Cause 2: RegisterTaskDefinition with a container healthCheck whose command exits 0 (and a second whose command exits 1, plus a sibling container with no healthCheck), RunTask it on Fargate and poll DescribeTasks containers[].healthStatus / task healthStatus.
5. Cause 3: RegisterTaskDefinition (FARGATE/awsvpc, cpu 256, memory 512) whose container runs `sh -c "exit 7"`, RunTask(count=3), then poll DescribeTasks for lastStatus/stopCode/containers[].exitCode and ListTasks(desiredStatus='STOPPED'); repeat with an image tag that does not exist in a private ECR repository and look for stopCode 'TaskFailedToStart'.
