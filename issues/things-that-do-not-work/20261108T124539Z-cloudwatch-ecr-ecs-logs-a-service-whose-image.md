Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ecr__ecs__logs.deploy-ecs-service-rollback-on-missing-ecr-image
Issue-Key: cloudwatch__ecr__ecs__logs.deploy-ecs-service-rollback-on-missing-ecr-image|_find_pull_error#15|Timeout|return ""
Issue-Commit: 6596661b7bed7b2c68eacfc83c0e7a703969540b

# cloudwatch__ecr__ecs__logs: a service whose image tag was never pushed never produces a STOPPED task with CannotPullContainerError

> _Note: this issue was identified against the code base at commit `6596661b7bed7b2c68eacfc83c0e7a703969540b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTasks

## Description
One net-new root cause under the suite heading `cloudwatch__ecr__ecs__logs`, grounded in a failing check that no open issue carries the current replay key for.

Cause 1 of 1: an ECS service pointed at an ECR image reference that can never resolve produces no failed task record — the emulator never stops a task with a pull failure, so the negative class (image missing) is indistinguishable from a platform that simply does nothing. Failing check id: cloudwatch__ecr__ecs__logs.deploy-ecs-service-rollback-on-missing-ecr-image.
  AWS: recorded behavior only for the reason text; the model gives the shapes — `ecs#Task.lastStatus`/`desiredStatus` reach STOPPED, `ecs#Task.stoppedReason` and `ecs#Container.reason` carry the failure text, and `ecs#Service.runningCount` stays at 0. In the recording, a service whose container image is `<repositoryUri>:<tag-never-pushed>` has its task attempts stop and `ListTasks(desiredStatus="STOPPED")` return them; the stopped task's container `reason` (or the task's `stoppedReason`) contains `CannotPullContainerError` followed by the registry's own diagnostic for the unresolvable reference, and the service's `runningCount` remains 0. The required, fixed part is the `CannotPullContainerError` classification on a STOPPED task of that service; the repository URI, tag and image digest in the rest of the sentence are values from the request and the registry state.
  Emulator: no stopped task with that reason ever appears — the check ends with `Error: Check timed out after 180.0s` while polling `ListTasks(desiredStatus="STOPPED")` + `DescribeTasks` for a container/task reason containing `CannotPullContainerError`. The emulator reports neither a pull failure nor any terminal task for the service.
  Why: image-pull failure is the single most common ECS deployment fault, and clients (deployment circuit breakers, rollback automation, operators triaging a stuck service) diagnose it exactly by reading `CannotPullContainerError` off a stopped task. Against this emulator such a deploy hangs with `runningCount` 0 and no explanation, so rollback-on-unpullable-image logic never fires and negative tests cannot be told apart from an inert control plane.
  Fix: the ECS task lifecycle behind RunTask/service reconciliation and DescribeTasks — it must attempt the pull of the container image, and on an unresolvable ECR reference stop the task (stopCode `TaskFailedToStart`) with a `CannotPullContainerError` reason recorded on the container (and the task's `stoppedReason`), leaving `runningCount` at 0 (ECS crate's task state machine and its ECR image resolution).
  Related open issue that does NOT carry this check's current replay key: 20261104T060239Z-cloudwatch-ecr-ecs-logs-tasks-never-settle-and.md (its cause 1 concerns tasks never reaching STOPPED and awslogs never delivering, under a different replay key; the suite did not mark this failure as owned by it).
  Traces: cloudwatch__ecr__ecs__logs.deploy-ecs-service-rollback-on-missing-ecr-image.

## Evidence
Cause 1 — expected: for a service created with container image `<ecr-repo-uri>:does-not-exist` (nothing ever pushed to that repository), `ecs list_tasks(cluster=<c>, serviceName=<s>, desiredStatus="STOPPED")` returns task ARNs and `describe_tasks` shows a container `reason` (or task `stoppedReason`) containing `CannotPullContainerError`, while `describe_services(...)['services'][0]['runningCount']` is 0. Actual: `Error: Check timed out after 180.0s` — the poll for a stopped task recording `CannotPullContainerError` never succeeds (the check allows 180s), so the positive control that follows is never reached.

## Steps to Reproduce
1. `ecs create_cluster`; `ecr create_repository(repositoryName=<r>)` and push NOTHING to it.
2. Create an execution role trusting ecs-tasks.amazonaws.com with AmazonECSTaskExecutionRolePolicy.
3. Register a FARGATE/awsvpc task definition whose container image is `<repositoryUri>:does-not-exist` and `create_service(..., desiredCount=1, networkConfiguration={awsvpcConfiguration:{subnets:[...], securityGroups:[...], assignPublicIp:"ENABLED"}})`.
4. Poll `ecs list_tasks(cluster=<c>, serviceName=<s>, desiredStatus="STOPPED")` and `ecs describe_tasks` for a container/task reason containing `CannotPullContainerError`, and check `runningCount == 0`.
Replay check id: cloudwatch__ecr__ecs__logs.deploy-ecs-service-rollback-on-missing-ecr-image.
