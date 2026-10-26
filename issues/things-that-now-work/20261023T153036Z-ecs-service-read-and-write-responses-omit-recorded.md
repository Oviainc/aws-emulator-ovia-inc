Issue-Source: trace-sweep-divergence
Issue-Trace: ecs-service-lifecycle-on-cluster
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ecs-service-lifecycle-on-cluster ecs-err-delete-cluster-contains-services ecs-task-set-external-deployment-workflow ecs-taskset-basic-lifecycle ecs-taskset-errors ecs-taskset-lifecycle-ec2
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs|DescribeServices|2xx->2xx|$.services[*]
Issue-Key: ecs|CreateService|2xx->2xx|$.service.createdBy
Issue-Key: ecs|CreateService|2xx->2xx|$.service.deployments[*].failedLaunchTaskCount
Issue-Key: ecs|CreateService|2xx->2xx|$.service.deployments[*].healthyTaskCount
Issue-Key: ecs|CreateService|2xx->2xx|$.service.deployments[*].replacedTaskCount
Issue-Key: ecs|CreateService|2xx->2xx|$.service.deployments[*].scale
Issue-Key: ecs|CreateService|2xx->2xx|$.service.deployments[*].stabilityStatus
Issue-Key: ecs|CreateService|2xx->2xx|$.service.deployments[*].totalFailedTasks
Issue-Key: ecs|CreateService|2xx->2xx|$.service.extendDeploymentTimePeriod
Issue-Key: ecs|CreateService|2xx->2xx|$.service.version
Issue-Key: ecs|UpdateService|2xx->2xx|$.service.createdBy
Issue-Key: ecs|UpdateService|2xx->2xx|$.service.deployments[*].failedLaunchTaskCount
Issue-Key: ecs|UpdateService|2xx->2xx|$.service.deployments[*].healthyTaskCount
Issue-Key: ecs|UpdateService|2xx->2xx|$.service.deployments[*].replacedTaskCount
Issue-Key: ecs|UpdateService|2xx->2xx|$.service.deployments[*].scale
Issue-Key: ecs|UpdateService|2xx->2xx|$.service.deployments[*].stabilityStatus
Issue-Key: ecs|UpdateService|2xx->2xx|$.service.deployments[*].totalFailedTasks
Issue-Key: ecs|UpdateService|2xx->2xx|$.service.events[*].id
Issue-Key: ecs|UpdateService|2xx->2xx|$.service.extendDeploymentTimePeriod
Issue-Key: ecs|UpdateService|2xx->2xx|$.service.runStatus
Issue-Key: ecs|UpdateService|2xx->2xx|$.service.version
Issue-Key: ecs|DescribeServices|2xx->2xx|$.services[*].createdBy
Issue-Key: ecs|DescribeServices|2xx->2xx|$.services[*].deployments[*].failedLaunchTaskCount
Issue-Key: ecs|DescribeServices|2xx->2xx|$.services[*].deployments[*].healthyTaskCount
Issue-Key: ecs|DescribeServices|2xx->2xx|$.services[*].deployments[*].replacedTaskCount
Issue-Key: ecs|DescribeServices|2xx->2xx|$.services[*].deployments[*].scale
Issue-Key: ecs|DescribeServices|2xx->2xx|$.services[*].deployments[*].stabilityStatus
Issue-Key: ecs|DescribeServices|2xx->2xx|$.services[*].deployments[*].totalFailedTasks
Issue-Key: ecs|DescribeServices|2xx->2xx|$.services[*].events[*]
Issue-Key: ecs|DescribeServices|2xx->2xx|$.services[*].extendDeploymentTimePeriod
Issue-Key: ecs|DescribeServices|2xx->2xx|$.services[*].runStatus
Issue-Key: ecs|DescribeServices|2xx->2xx|$.services[*].version
Issue-Commit: 6d7833292dd9d4a45d8e52e2aa7ee7e5a822f7f5

# ECS service read and write responses omit recorded Service and Deployment members

> _Note: this issue was identified against the code base at commit `6d7833292dd9d4a45d8e52e2aa7ee7e5a822f7f5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeServices

## Description
Six divergences in how the ecs emulator renders the Service structure on every operation that returns one. All share one fault: the Service/Deployment projection is incomplete and its createdBy principal is synthesized at random. A fixer must close ALL SIX causes below before submitting.

Cause 1 of 6: DescribeServices' service object does not match the recorded Service at all.
AWS: Smithy DescribeServices -> Services/Service. The recorded service object carries availabilityZoneRebalancing, currentServiceDeployment (a service-deployment ARN), currentServiceRevisions[] (arn + pendingTaskCount/requestedTaskCount/runningTaskCount), a fully populated deploymentConfiguration (bakeTimeInMinutes, deploymentCircuitBreaker with enable/resetOnHealthyTask/rollback/thresholdConfiguration, maximumPercent, minimumHealthyPercent, strategy), deploymentController, deployments[] with rolloutState/rolloutStateReason plus the undocumented-but-returned failedLaunchTaskCount, healthyTaskCount, replacedTaskCount, scale, stabilityStatus and totalFailedTasks, and the top-level extendDeploymentTimePeriod, runStatus and version. The members not in the Smithy model are recorded behavior only.
Emulator: the whole $.services[0] fails to match the recording.
Why: a client that reads deployment rollout/stability state or the service revision to decide whether a deployment is healthy gets absent values and cannot make the decision.
Fix: the DescribeServices serializer / Service projection in the ecs service crate.
Traces: ecs-service-lifecycle-on-cluster#task#4 (also #task#6).

Cause 2 of 6: CreateService's returned Service omits deployment and version members.
AWS: the recorded CreateService 200 body carries service.version 0, service.extendDeploymentTimePeriod 0 and deployments[0].{failedLaunchTaskCount, healthyTaskCount, replacedTaskCount, scale {unit PERCENT, value}, stabilityStatus, totalFailedTasks} (recorded behavior only — the Smithy Service/Deployment shapes do not declare these).
Emulator: all of those members are absent from the 200 body.
Why: a client that reads version for optimistic concurrency, or stabilityStatus to wait for STABILIZING -> STEADY_STATE, sees nothing and either spins forever or skips the wait.
Fix: the CreateService response serializer.
Traces: ecs-service-lifecycle-on-cluster#task#3.

Cause 3 of 6: UpdateService's returned Service omits the same members and emits non-UUID event ids.
AWS: the recorded UpdateService 200 body carries service.version, service.extendDeploymentTimePeriod, service.runStatus and deployments[0].{failedLaunchTaskCount, healthyTaskCount, replacedTaskCount, scale, stabilityStatus, totalFailedTasks}; service.events[].id is a UUID-shaped identifier (the fixture asserts its shape, not its value).
Emulator: those members are absent, and events[].id is a 32-character zero-padded counter ('00000000000000000000000000000001') that fails the recorded id shape.
Why: same unobservable deployment state as Cause 2, plus event ids that do not look like ECS event ids to clients that parse or de-duplicate them.
Fix: the UpdateService response serializer and the service-event id generator.
Traces: ecs-service-lifecycle-on-cluster#task#5.

Cause 4 of 6: a deleted service's DescribeServices body omits runStatus/version and still carries its event stream.
AWS: the recording returns runStatus TOMBSTONE, version, extendDeploymentTimePeriod and the deployment-level members, and returns NO events for the deleted service.
Emulator: runStatus/version/extendDeploymentTimePeriod and the deployment members are absent, and two steady-state events are still present.
Why: a client cannot distinguish a tombstoned service from a live one, and replays stale events for a service that no longer exists.
Fix: DescribeServices projection + service-event retention on delete.
Traces: ecs-service-lifecycle-on-cluster#task#12.

Cause 5 of 6: the same omission on DescribeServices for a service in a cluster that still contains services.
AWS: recorded body carries createdBy, runStatus TOMBSTONE, version, extendDeploymentTimePeriod and deployments[0].{failedLaunchTaskCount, healthyTaskCount, replacedTaskCount, scale, stabilityStatus, totalFailedTasks}.
Emulator: all absent.
Why: as Cause 1/4 — the read path is the one clients poll, so the gap is permanent rather than transient.
Fix: DescribeServices serializer.
Traces: ecs-err-delete-cluster-contains-services#task#7.

Cause 6 of 6: Service.createdBy is a random per-session principal instead of the caller's identity.
AWS: Smithy Service.createdBy — 'the principal that created the service'; the recording returns a single stable IAM user ARN for the calling principal across all services it creates.
Emulator: returns a different randomly-suffixed principal per session (arn:aws:iam::000000000000:user/EMULATOR<hex>), so the value is neither the caller's identity nor stable between calls. The recorded local-account ARN is cited as an example; the requirement is that createdBy be the ARN of the authenticated caller and identical for every resource that caller creates.
Why: a client that groups or filters resources by creating principal, or asserts createdBy equals its own GetCallerIdentity ARN, never matches.
Fix: caller-identity resolution feeding the Service serializer (shared by CreateService/UpdateService/DeleteService/DescribeServices).
Traces: ecs-task-set-external-deployment-workflow#setup#4 (also ecs-taskset-basic-lifecycle#setup#4, ecs-taskset-errors#setup#4, ecs-taskset-lifecycle-ec2#setup#4).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from mainline. All six clusters are status 200 -> 200 body mismatches.
Cause 1: divergence_key service=ecs|DescribeServices|status=200->200|keys=$.services[0]; detail '$.services[0]: missing (expected {availabilityZoneRebalancing: ENABLED, clusterArn, createdAt, createdBy, currentServiceDeployment, currentServiceRevisions: [{arn, pendingTaskCount: 0, requestedTaskCount: 0, runningTaskCount: 0}], deploymentConfiguration: {bakeTimeInMinutes: 0, deploymentCircuitBreaker: {enable: false, resetOnHealthyTask: true, rollback: false, thresholdConfiguration: {type: BOUNDED_PERCENT, value: 50}}, maximumPercent: 200, minimumHealthyPercent: 100, strategy: ROLLING}, deploymentController: {type: ECS}, deployments: [{... failedLaunchTaskCount: 0, healthyTaskCount: 0, replacedTaskCount: 0, rolloutState: COMPLETED, scale: {unit: PERCENT, value: 100.0}, stabilityStatus: STEADY_STATE, totalFailedTasks: 0}], ...})'.
Cause 2: detail '$.service.deployments[0].failedLaunchTaskCount: missing (expected 0, got None)', '.healthyTaskCount: missing', '.replacedTaskCount: missing', '.scale: missing (expected {unit: PERCENT, value: 0.0})', '.stabilityStatus: missing (expected STABILIZING)', '.totalFailedTasks: missing', '$.service.extendDeploymentTimePeriod: missing (expected 0)', '$.service.version: missing (expected 0)'.
Cause 3: same missing members plus '$.service.runStatus: missing (expected SteadyState)', '$.service.events[0].id: value_mismatch (expected <shaped-not-asserted>, got 00000000000000000000000000000002)', '$.service.events[1].id: value_mismatch (expected <shaped-not-asserted>, got 00000000000000000000000000000001)'.
Cause 4: detail '$.services[0].runStatus: missing (expected TOMBSTONE, got None)', '$.services[0].version: missing (expected 0)', '$.services[0].events[0]: extra (expected None, got {id: 000...0002, message: (service ...) has reached a steady state.})', '$.services[0].events[1]: extra (expected None, got ...deployment completed.)'.
Cause 5: detail '$.services[0].runStatus: missing (expected TOMBSTONE)', '$.services[0].version: missing (expected 0)', '$.services[0].deployments[0].stabilityStatus: missing (expected STEADY_STATE)'.
Cause 6: detail '$.service.createdBy: value_mismatch (expected arn:aws:iam::000000000000:user/emulator, got arn:aws:iam::000000000000:user/EMULATORAB17B2288A23)'; the suffix differs per session across clusters (EMULATOR382A8764C918, EMULATOR0075A257273D, EMULATOR7CD3078BDA28, EMULATOR6D752220A1E1).
state_reset_failures = 0 for this run.

## Steps to Reproduce
1. Build the emulator from mainline and boot it.
2. Replay fixture ecs-service-lifecycle-on-cluster and compare step task#4 (DescribeServices) — representative.
3. Replay the other bundled steps: ecs-service-lifecycle-on-cluster#task#3, ecs-service-lifecycle-on-cluster#task#5, ecs-service-lifecycle-on-cluster#task#12, ecs-err-delete-cluster-contains-services#task#7, ecs-task-set-external-deployment-workflow#setup#4.
4. All six must match the recorded bodies before this issue is closed.
