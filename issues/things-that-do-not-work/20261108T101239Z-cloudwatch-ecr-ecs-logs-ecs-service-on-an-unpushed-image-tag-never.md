Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ecr__ecs__logs.deploy-ecs-service-rollback-on-missing-ecr-image
Issue-Key: cloudwatch__ecr__ecs__logs.deploy-ecs-service-rollback-on-missing-ecr-image|_find_pull_error#15|Timeout|tasks = ecs.list_tasks(
Issue-Commit: 5326d37abbcb9c44bc1d6bac564fe7e72c9c6fa2

# ECS service on an unpushed image tag never produces a STOPPED task: ListTasks(desiredStatus=STOPPED)/DescribeTasks never report CannotPullContainerError

> _Note: this issue was identified against the code base at commit `5326d37abbcb9c44bc1d6bac564fe7e72c9c6fa2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTasks

## Description
One net-new root cause, grounded in the failing behavioral check under the suite heading `cloudwatch__ecr__ecs__logs`.

Cause 1 of 1: a Fargate service whose container image reference cannot be resolved in ECR never yields a stopped task carrying a pull failure, so the image-pull failure class is unobservable.

AWS: a service created with an image reference `<repositoryUri>:<tag>` that was never pushed launches a task that fails at the pull stage. The task walks PROVISIONING -> PENDING -> STOPPED (Smithy: ecs#Task.lastStatus, with stoppedAt/stoppingAt/executionStoppedAt set, ecs#Task.stopCode of the modeled enum ecs#TaskStopCode — TaskFailedToStart for a task that never started a container — and ecs#Task.stoppedReason "The reason that the task was stopped"). The failure is reported on the container: ecs#Container.reason is "A short (1024 max characters) human-readable string to provide additional details about a running or stopped container" and carries the pull failure, whose fixed text is the error class token `CannotPullContainerError` followed by the resolver's detail; the variable parts are the image reference from the task definition and the registry/repository names taken from it (e.g. `CannotPullContainerError: ... <imageRef>: not found`). That task is reachable through ListTasks with desiredStatus=STOPPED and then DescribeTasks. Meanwhile the service keeps runningCount 0 (ecs#Service.runningCount). Exact detail wording after the `CannotPullContainerError:` token is recorded behavior only; the token itself and the STOPPED-task-with-a-reason shape are the requirement.

Emulator: no stopped task is ever produced for the unpullable service. The check polls ListTasks(cluster, serviceName, desiredStatus="STOPPED") every 10 s for 180 s and gets an empty `taskArns` (or tasks whose containers carry no `reason`), so no DescribeTasks response ever contains `CannotPullContainerError` in either `containers[].reason` or `stoppedReason`, and the check dies with "Check timed out after 180.0s". The pull failure is simply never modelled: the service neither places a task that fails nor records why.

Why: a client cannot distinguish "image tag does not exist" from "service is still converging". Deployment-health logic, rollback-on-failed-deployment, and any diagnosis that reads a stopped task's container reason to classify CannotPullContainerError never fires; a caller waiting for a terminal task waits forever.

Fix: the ECS task lifecycle behind CreateService/the service scheduler and DescribeTasks — the image-resolution step must resolve the container image against the ECR registry state, and on failure stop the task (lastStatus STOPPED, stopCode TaskFailedToStart, stoppedReason set) with the container's `reason` carrying the CannotPullContainerError text, and expose it via ListTasks(desiredStatus=STOPPED)/DescribeTasks while runningCount stays 0 (services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src).

Note: the open issue 20261104T060239Z-cloudwatch-ecr-ecs-logs-tasks-never-settle-and.md lists this check id among its traces, but the replay no longer matches its key (the suite did not mark this check as owned — the failure now trips the 180 s CannotPullContainerError poll rather than that issue's recorded signature), so this unpullable-image path is filed separately; closing that issue does not close this one.

Traces: cloudwatch__ecr__ecs__logs.deploy-ecs-service-rollback-on-missing-ecr-image.

## Evidence
Expected: within 180 s of creating the service, ListTasks(cluster=<cluster>, serviceName=<service>, desiredStatus="STOPPED") returns at least one task ARN, and DescribeTasks on it returns a task whose containers[].reason (or whose stoppedReason) contains "CannotPullContainerError", with lastStatus "STOPPED" and the service's runningCount 0.

Actual: the poll `_poll_until(_find_pull_error, lambda r: "CannotPullContainerError" in r, timeout_s=180.0, interval_s=10.0, description="a stopped task recording CannotPullContainerError")` never satisfies its predicate and the check ends with:

  Error: Check timed out after 180.0s

No STOPPED task is ever listed for the service created against `<repositoryUri>:does-not-exist` on a repository that was deliberately left empty, so the assertion "expected a stopped task on the missing-image service to record CannotPullContainerError" can never be evaluated.

## Steps to Reproduce
1. ecs CreateCluster.
2. ecr CreateRepository and push nothing to it; build the image reference `<repository.repositoryUri>:does-not-exist`.
3. iam CreateRole with an ecs-tasks.amazonaws.com trust policy for the task execution role.
4. ecs RegisterTaskDefinition (FARGATE, awsvpc) with one container using that unpushed image reference, then ecs CreateService with desiredCount >= 1 and an awsvpcConfiguration on a shared-network subnet.
5. Poll ecs ListTasks(cluster=<cluster>, serviceName=<service>, desiredStatus="STOPPED") and ecs DescribeTasks on the results for up to 180 s, looking for "CannotPullContainerError" in containers[].reason or stoppedReason — nothing is ever returned, and DescribeServices keeps runningCount 0 with no stopped-task history to explain it.
