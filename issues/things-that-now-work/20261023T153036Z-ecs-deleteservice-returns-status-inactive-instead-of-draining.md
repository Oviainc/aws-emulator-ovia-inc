Issue-Source: trace-sweep-divergence
Issue-Trace: ecs-service-lifecycle-on-cluster
Issue-Step: task#11
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ecs-service-lifecycle-on-cluster ecs-err-delete-cluster-contains-services ecs-err-service-not-active ecs-task-set-external-deployment-workflow ecs-taskset-basic-lifecycle ecs-taskset-lifecycle-ec2 ecs-taskset-tags-and-external-id ecs-taskset-errors
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs|DeleteService|2xx->2xx|$.service.createdBy
Issue-Key: ecs|DeleteService|2xx->2xx|$.service.deployments[*].failedLaunchTaskCount
Issue-Key: ecs|DeleteService|2xx->2xx|$.service.deployments[*].healthyTaskCount
Issue-Key: ecs|DeleteService|2xx->2xx|$.service.deployments[*].replacedTaskCount
Issue-Key: ecs|DeleteService|2xx->2xx|$.service.deployments[*].totalFailedTasks
Issue-Key: ecs|DeleteService|2xx->2xx|$.service.extendDeploymentTimePeriod
Issue-Key: ecs|DeleteService|2xx->2xx|$.service.runStatus
Issue-Key: ecs|DeleteService|2xx->2xx|$.service.status
Issue-Key: ecs|DeleteService|2xx->2xx|$.service.version
Issue-Key: ecs|DeleteService|2xx->2xx|$.service.taskSets[*]
Issue-Commit: 6d7833292dd9d4a45d8e52e2aa7ee7e5a822f7f5

# ECS DeleteService returns status INACTIVE instead of DRAINING and drops the service's taskSets

> _Note: this issue was identified against the code base at commit `6d7833292dd9d4a45d8e52e2aa7ee7e5a822f7f5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteService

## Description
Five divergences on the DeleteService response, all in the DeleteService handler and the Service projection it returns. A fixer must close ALL FIVE causes below before submitting.

Cause 1 of 5: DeleteService reports the service as already INACTIVE and omits its lifecycle members.
AWS: Smithy DeleteService -> DeleteServiceResponse{service: Service}; Service.status is modeled as ACTIVE | DRAINING | INACTIVE, and the recorded DeleteService response returns status DRAINING (the service is draining its remaining tasks, not yet deleted). The recording also returns runStatus, version, extendDeploymentTimePeriod and deployments[0].{failedLaunchTaskCount, healthyTaskCount, replacedTaskCount, totalFailedTasks} (recorded behavior only for the members the model does not declare).
Emulator: returns status 'INACTIVE' and omits runStatus, version, extendDeploymentTimePeriod and those deployment members.
Why: a client that deletes a service and then waits for DRAINING -> INACTIVE to know when cleanup finished observes the terminal state immediately and tears down dependencies (load balancers, task definitions, the cluster) while the emulator still considers tasks attached.
Fix: DeleteService handler state transition + its Service serializer.
Traces: ecs-service-lifecycle-on-cluster#task#11.

Cause 2 of 5: DeleteService omits version and extendDeploymentTimePeriod.
AWS: the recorded 200 body carries service.version 0 and service.extendDeploymentTimePeriod 0.
Emulator: both absent.
Why: a client using version for optimistic concurrency on a subsequent UpdateService has no value to send.
Fix: DeleteService response serializer.
Traces: ecs-err-delete-cluster-contains-services#task#6 (also ecs-err-service-not-active#task#6).

Cause 3 of 5: DeleteService drops the service's taskSets entirely (EXTERNAL deployment controller, two task sets).
AWS: Smithy Service.taskSets — 'information about a set of Amazon ECS tasks in either a CodeDeploy or an EXTERNAL deployment'; the recorded DeleteService body returns both task sets with status DRAINING, computedDesiredCount, launchType, scale, stabilityStatus, serviceArn, taskSetArn, taskDefinition and tags.
Emulator: $.service.taskSets is absent, so neither task set is reported; it also returns status INACTIVE instead of DRAINING and omits runStatus/version/extendDeploymentTimePeriod.
Why: a client running an EXTERNAL/blue-green deployment reconciles its task sets from this response; with taskSets absent it believes there is nothing left to drain and leaks or double-deletes them.
Fix: DeleteService handler/state (task-set cascade) + Service.taskSets serialization.
Traces: ecs-task-set-external-deployment-workflow#teardown#16.

Cause 4 of 5: the same taskSets drop for a single-task-set service.
AWS: the recorded body returns taskSets[0] with status DRAINING and its scale/stabilityStatus/ARNs, plus status DRAINING, runStatus, version and extendDeploymentTimePeriod on the service.
Emulator: taskSets absent, status INACTIVE, those members absent.
Why: as Cause 3.
Fix: DeleteService handler/state + serializer.
Traces: ecs-taskset-basic-lifecycle#teardown#12 (also ecs-taskset-lifecycle-ec2#teardown#12, ecs-taskset-tags-and-external-id#teardown#8).

Cause 5 of 5: DeleteService omits runStatus (recorded MoreWork) while reporting INACTIVE.
AWS: the recorded body returns status DRAINING with runStatus 'MoreWork' — i.e. the delete is explicitly reported as incomplete — plus version and extendDeploymentTimePeriod.
Emulator: status INACTIVE, runStatus/version/extendDeploymentTimePeriod absent.
Why: the single field that tells a client the deletion still has work to do is missing, and the status contradicts it.
Fix: DeleteService handler state transition + serializer.
Traces: ecs-taskset-errors#teardown#13.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from mainline. All five clusters are status 200 -> 200 body mismatches on DeleteService.
Cause 1: detail '$.service.status: value_mismatch (expected DRAINING, got INACTIVE)', '$.service.runStatus: missing (expected SteadyState, got None)', '$.service.version: missing (expected 0)', '$.service.extendDeploymentTimePeriod: missing (expected 0)', '$.service.deployments[0].failedLaunchTaskCount/.healthyTaskCount/.replacedTaskCount/.totalFailedTasks: missing (expected 0)'.
Cause 2: divergence_key service=ecs|DeleteService|status=200->200|keys=$.service.extendDeploymentTimePeriod,$.service.version; detail '$.service.extendDeploymentTimePeriod: missing (expected 0, got None)', '$.service.version: missing (expected 0, got None)'; 2 affected steps.
Cause 3: detail '$.service.taskSets[0]: missing (expected {computedDesiredCount: 0, launchType: EC2, scale: {unit: PERCENT, value: 0.0}, stabilityStatus: STABILIZING, status: DRAINING, taskSetArn, taskDefinition ...}, got None)', '$.service.taskSets[1]: missing (...)', '$.service.status: value_mismatch (expected DRAINING, got INACTIVE)'.
Cause 4: detail '$.service.taskSets[0]: missing (expected {... status: DRAINING, stabilityStatus: STABILIZING ...}, got None)', '$.service.status: value_mismatch (expected DRAINING, got INACTIVE)'; 3 affected steps.
Cause 5: detail '$.service.runStatus: missing (expected MoreWork, got None)', '$.service.status: value_mismatch (expected DRAINING, got INACTIVE)'.
state_reset_failures = 0 for this run.

## Steps to Reproduce
1. Build the emulator from mainline and boot it.
2. Replay fixture ecs-service-lifecycle-on-cluster and compare step task#11 (DeleteService) — representative.
3. Replay the other bundled steps: ecs-err-delete-cluster-contains-services#task#6, ecs-task-set-external-deployment-workflow#teardown#16, ecs-taskset-basic-lifecycle#teardown#12, ecs-taskset-errors#teardown#13.
4. All five must return the recorded DRAINING status, the lifecycle members and the task sets before this issue is closed.
