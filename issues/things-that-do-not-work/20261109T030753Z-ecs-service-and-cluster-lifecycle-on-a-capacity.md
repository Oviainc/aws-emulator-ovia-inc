Issue-Source: trace-sweep-divergence
Issue-Trace: ecs-ecsnrt-traj-zero-capacity-provider-strategy-repair
Issue-Step: setup#11
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ecs-ecsnrt-traj-zero-capacity-provider-strategy-repair ecs-ecsdep-traj-circuit-breaker-rollback-vs-stop
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs|CreateService|2xx->4xx|status
Issue-Key: ecs|DeleteService|2xx->4xx|status
Issue-Key: ecs|DeleteCluster|2xx->4xx|status
Issue-Key: ecs|DescribeServices|0->None|$
Issue-Key: ecs|DescribeServices|0->None|status
Issue-Commit: aba2adfa3005273c5f6b7f8438c028c27604934b

# ECS service and cluster lifecycle on a capacity-provider cluster is refused 400, and a circuit-breaker deployment wait never converges

> _Note: this issue was identified against the code base at commit `aba2adfa3005273c5f6b7f8438c028c27604934b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateService

## Description
FOUR net-new ECS root causes, bundled because they share one fault: a step of a recorded ECS trajectory that AWS completes normally never completes against the emulator — three of them are refused with HTTP 400 where AWS answers HTTP 200 (CreateService, DeleteService, DeleteCluster, all on a cluster whose capacity is supplied by an EC2_AUTOSCALING capacity provider), and the fourth is a DescribeServices wait on a circuit-breaker deployment that never reaches a terminal state. ALL FOUR causes must be closed before this issue is submitted.

Cause 1 of 4: CreateService on a cluster with a capacity-provider strategy is refused HTTP 400 where AWS answers 200 with the created service. Fixture ecs-ecsnrt-traj-zero-capacity-provider-strategy-repair, step setup#11.
AWS: Smithy (com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113) CreateService takes cluster (String), serviceName (String, required), taskDefinition, desiredCount (BoxedInteger), launchType (LaunchType), capacityProviderStrategy (CapacityProviderStrategy list), deploymentConfiguration, deploymentController, networkConfiguration, tags (length 0..50) and returns CreateServiceResponse.service -> Service. Its modelled errors are AccessDeniedException (403) and, at 400, ClientException, ClusterNotFoundException, InvalidParameterException, NamespaceNotFoundException, PlatformTaskDefinitionIncompatibilityException, PlatformUnknownException, UnsupportedFeatureException, plus ServerException (500). Creating a service in an existing cluster with a capacityProviderStrategy over an existing ACTIVE capacity provider — including one whose entry carries base 0 and weight 1 — is a valid request: AWS answers HTTP 200 with the full Service element (serviceArn, serviceName, clusterArn, capacityProviderStrategy, desiredCount, deploymentConfiguration, deploymentController {type 'ECS'}, deployments[] with a PRIMARY deployment, currentServiceRevisions). A 400 is reserved for a genuinely bad request, and each refusal class has its own __type: ClusterNotFoundException for an absent cluster, InvalidParameterException for a malformed/contradictory parameter.
Emulator: the step is refused with HTTP 400 and no `service` body at all — the replay reports only "status: expected 200, got 400" for divergence_key service=ecs|CreateService|status=200->400|keys=status — so no service is created and every later step of the trajectory that needs it (the UpdateService strategy repair, the DescribeServices wait, the teardown deletes) operates on a service that does not exist. Hypothesis: the refusal is a knock-on of the capacity-provider lookup failure already filed as open issue 20261106T135659Z-ecs-ec2-autoscaling-capacity-providers-are-invisible-after.md (its cause 2 is this fixture's CreateCluster at setup#8 being refused 400, so the cluster is absent) — but that issue carries no replay key for CreateService, so fixing it will not re-run or close this step, and CreateService must also answer 200 once the cluster and provider resolve.
Why: the standard EC2-capacity bootstrap (launch template -> Auto Scaling group -> capacity provider -> cluster -> service over a capacityProviderStrategy) cannot complete at all. A client gets a bare 400 with no error body to branch on, cannot tell a bad request from an emulator that cannot find its own cluster/provider, and never receives the serviceArn or the PRIMARY deployment it must monitor.
Fix: the ECS CreateService handler, its cluster/capacity-provider resolution and its Service serializer in the ecs service crate — resolve `cluster` and each capacityProviderStrategy entry by short name or full ARN against stored state, create the service, and answer 200 with the full Service element; reserve 400 for the modelled refusal classes and always emit a __type and message with it.
Traces: ecs-ecsnrt-traj-zero-capacity-provider-strategy-repair#setup#11 (CreateService).

Cause 2 of 4: DeleteService in the same trajectory's teardown is refused HTTP 400 where AWS answers 200 with the deleted service. Same fixture, step teardown#18.
AWS: Smithy DeleteService input is cluster (String), service (String, required), force (BoxedBoolean); output DeleteServiceResponse.service -> Service; the operation is @idempotent. Modelled errors: AccessDeniedException (403), and at 400 ClientException, ClusterNotFoundException, InvalidParameterException, ServiceNotFoundException, plus ServerException (500). Deleting an existing service (scaled to desiredCount 0, or with force) answers HTTP 200 carrying the Service element with status DRAINING/INACTIVE.
Emulator: HTTP 400 with no `service` body (divergence_key service=ecs|DeleteService|status=200->400|keys=status; detail "status: expected 200, got 400").
Why: teardown is not optional for a client — a CI/CD or IaC flow that cannot delete a service leaks it, and a bare 400 with no __type means the client cannot even treat it as "already gone" (which AWS expresses as ServiceNotFoundException with message "The specified service wasn't found.") and retry/ignore accordingly. Note that when the service truly does not exist, the required behavior is still not a bare 400: it is 400 with __type ServiceNotFoundException and AWS's fixed message text for that class.
Fix: the ECS DeleteService handler and its service lookup in the ecs service crate — resolve cluster/service by short name or ARN, mark the service INACTIVE and answer 200 with the Service element; when the identifier resolves to nothing answer ServiceNotFoundException with its modelled message, never an empty 400.
Traces: ecs-ecsnrt-traj-zero-capacity-provider-strategy-repair#teardown#18 (DeleteService).

Cause 3 of 4: DeleteCluster in the same teardown is refused HTTP 400 where AWS answers 200 with the deleted cluster. Same fixture, step teardown#19.
AWS: Smithy DeleteCluster input is cluster (String, required, resourceIdentifier ClusterName); output DeleteClusterResponse.cluster -> Cluster; @idempotent. Modelled errors: AccessDeniedException (403), and at 400 ClientException, ClusterContainsCapacityProviderException, ClusterContainsContainerInstancesException, ClusterContainsServicesException, ClusterContainsTasksException, ClusterNotFoundException, InvalidParameterException, UpdateInProgressException, plus ServerException (500). Deleting an emptied cluster answers HTTP 200 with the Cluster element and status INACTIVE. Each refusal condition has a distinct modelled __type — a cluster that still holds services is ClusterContainsServicesException, one that still has an associated capacity provider is ClusterContainsCapacityProviderException, an absent cluster is ClusterNotFoundException ("The specified cluster wasn't found.").
Emulator: HTTP 400 with no `cluster` body and no error discriminator reported (divergence_key service=ecs|DeleteCluster|status=200->400|keys=status; detail "status: expected 200, got 400").
Why: the cluster cannot be torn down, so repeated runs collide on the cluster name, and the client is given no modelled __type that would tell it which precondition to clear (drain services, disassociate the capacity provider) before retrying.
Fix: the ECS DeleteCluster handler and the cluster state in the ecs service crate — resolve the cluster by name or ARN, delete it and answer 200 with the Cluster element at status INACTIVE; where a precondition genuinely blocks deletion, answer the specific modelled exception (ClusterContainsServicesException / ClusterContainsTasksException / ClusterContainsContainerInstancesException / ClusterContainsCapacityProviderException) rather than a bodiless 400.
Traces: ecs-ecsnrt-traj-zero-capacity-provider-strategy-repair#teardown#19 (DeleteCluster).

Cause 4 of 4: a DescribeServices wait on a deployment with the deployment circuit breaker enabled never converges, so the fixture's whole time budget is consumed. Fixture ecs-ecsdep-traj-circuit-breaker-rollback-vs-stop, step task#15.
AWS: Smithy Deployment.rolloutState -> DeploymentRolloutState, an enum constrained to COMPLETED | FAILED | IN_PROGRESS, with Deployment.rolloutStateReason, Deployment.status (PRIMARY | ACTIVE | INACTIVE) and the running/pending/desired counts beside it; DeploymentCircuitBreaker.enable and .rollback are both required booleans. The model states the contract: a deployment begins IN_PROGRESS, transitions to COMPLETED when the service reaches a steady state, and transitions to FAILED when the service fails to reach a steady state and the circuit breaker is turned on; with rollback on, the service is rolled back to the last deployment that completed successfully. The requirement is that a deployment leaves IN_PROGRESS for a terminal rolloutState within a bounded time — FAILED with a rolloutStateReason attributing the failure to the circuit breaker — and that with rollback enabled the PRIMARY deployment returns to the last completed revision with runningCount converging on desiredCount; the parallel ServiceDeployment record reaches a terminal status/statusReason the same way, whether the rollout ended by circuit-breaker rollback or by StopServiceDeployment.
Emulator: the step is a wait with no status assertion of its own — the replay records "status: expected , got None" with actual_status null, i.e. no response was ever accepted as final — and the awaited deployment state never appears, so the step is retried until "$: fixture budget 960s spent" and every later step of the trajectory (including its StopServiceDeployment comparison arm) is abandoned unreplayed. The exposed deployment stays non-terminal: it never becomes FAILED with a circuit-breaker rolloutStateReason and no rollback to the last completed revision is ever visible.
Why: any client deploying behind the deployment circuit breaker relies on ECS marking an unlaunchable rollout FAILED and restoring the last good revision, and on DescribeServices reporting that within a bounded time. Against the emulator such waiters hang until their own timeout and then report a false verdict, and automation cannot tell "the circuit breaker rolled this back" from "an operator stopped the deployment".
Fix: the ECS service-deployment state machine in the ecs service crate — the progression driven by CreateService/UpdateService must evaluate the circuit breaker against failing task launches, drive the deployment to terminal rolloutState FAILED with a rolloutStateReason naming the circuit breaker, make the prior completed revision PRIMARY again when rollback is enabled, and converge runningCount on desiredCount; the DescribeServices deployments projection must surface those fields so a poll can terminate. NOTE for the fixer: open issues 20261107T170944Z-ecs-circuit-breaker-deployment-never-reaches-a-terminal.md and 20261107T184643Z-ecs-describeservices-waits-never-converge-a-capacity-provider.md (its cause 2) already describe this cause and 20261107T170944Z carries this exact fixture/step as its replay key, yet this replay still reported the cluster as unowned (owned_by empty), so it is carried here to keep the cluster covered — one fix closes all three, no separate work is needed. Behavioral issues 20261106T115645Z-ecs-3-net-new-causes-a-cluster-scoped.md (cause 2) and 20261104T062603Z-ecs-rolling-deployment-controls-are-not-enforced-the.md (cause 3) describe the same gap but carry no wire replay key.
Traces: ecs-ecsdep-traj-circuit-breaker-rollback-vs-stop#task#15 (DescribeServices).

## Evidence
Wire replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up true, /__admin/health answered 200, data listener accepting; 117 fixtures replayed, 1356 steps, 1151 passed, score 84.9%, 85 divergence clusters). These four clusters are exactly the ones whose owned_by AND stub_path are both empty. All four are service ecs.

Cause 1 — divergence_key service=ecs|CreateService|status=200->400|keys=status, verdict mismatch, affected_count 1:
  fixture_id: ecs-ecsnrt-traj-zero-capacity-provider-strategy-repair
  step_key:   setup#11
  operation:  CreateService
  expected_status: 200; actual_status: 400
  detail: "status: expected 200, got 400"
  Expected (AWS): 200 with `service` = the full Service element (serviceArn, serviceName, clusterArn, capacityProviderStrategy over the fixture's capacity provider with base 0 / weight 1, desiredCount, deploymentConfiguration {bakeTimeInMinutes, deploymentCircuitBreaker, maximumPercent, minimumHealthyPercent, strategy 'ROLLING'}, deploymentController {type 'ECS'}, deployments[] with status 'PRIMARY' / rolloutState 'IN_PROGRESS' / stabilityStatus 'STABILIZING' / taskDefinition, currentServiceRevisions[]).
  Actual (emulator): 400 and no `service` member; nothing in the response identifies a refusal class.
  Upstream sibling steps of the same fixture, already owned by other issues: setup#8 CreateCluster 200->400, setup#9 DescribeClusters reports the cluster with reason 'MISSING', task#12 DescribeCapacityProviders reports the just-created provider with reason 'MISSING', task#13 RunTask answers ClusterNotFoundException 'Cluster not found.'.

Cause 2 — divergence_key service=ecs|DeleteService|status=200->400|keys=status, verdict mismatch, affected_count 1:
  fixture_id: ecs-ecsnrt-traj-zero-capacity-provider-strategy-repair
  step_key:   teardown#18
  operation:  DeleteService
  expected_status: 200; actual_status: 400
  detail: "status: expected 200, got 400"
  Expected (AWS): 200 with `service` = the Service element at status DRAINING/INACTIVE. If and only if the identifier resolves to nothing: 400 with __type ServiceNotFoundException and its fixed message "The specified service wasn't found." (fixed text; no variable part).
  Actual (emulator): 400 with no `service` member and no error discriminator reported by the replay.

Cause 3 — divergence_key service=ecs|DeleteCluster|status=200->400|keys=status, verdict mismatch, affected_count 1:
  fixture_id: ecs-ecsnrt-traj-zero-capacity-provider-strategy-repair
  step_key:   teardown#19
  operation:  DeleteCluster
  expected_status: 200; actual_status: 400
  detail: "status: expected 200, got 400"
  Expected (AWS): 200 with `cluster` = the Cluster element (clusterArn, clusterName, status 'INACTIVE'). If and only if a precondition genuinely blocks it: 400 with the matching modelled __type — ClusterContainsServicesException, ClusterContainsTasksException, ClusterContainsContainerInstancesException, ClusterContainsCapacityProviderException, ClusterNotFoundException ("The specified cluster wasn't found.").
  Actual (emulator): 400 with no `cluster` member and no error discriminator reported by the replay.

Cause 4 — divergence_key service=ecs|DescribeServices|status=0->None|keys=$,status, verdict mismatch, affected_count 1:
  fixture_id: ecs-ecsdep-traj-circuit-breaker-rollback-vs-stop
  step_key:   task#15
  operation:  DescribeServices
  expected_status: 0 (wait step — no status asserted); actual_status: null
  detail: "status: expected , got None" and "$: fixture budget 960s spent (expected None, got None)"
  Expected (AWS): the poll terminates because the circuit-breaker deployment leaves IN_PROGRESS for a terminal rolloutState — FAILED with a rolloutStateReason attributing the failure to the deployment circuit breaker — and, with rollback enabled, the PRIMARY deployment returns to the last completed revision with runningCount == desiredCount. deployments[].rolloutState is constrained by the model to COMPLETED | FAILED | IN_PROGRESS.
  Actual (emulator): the awaited state never appears; no response is ever accepted as final and the fixture's whole budget is consumed, so this step and everything after it in the trajectory never complete.

Per-run values in these recordings are NOT requirements: the cluster/service/capacity-provider names and ARNs, the account id, the region, revision and deployment ids, createdAt timestamps, the replay's 180s poll cap and the 960s fixture budget. The requirements are the HTTP statuses (200 for all three lifecycle calls), the presence and shape of the `service` / `cluster` response members, the modelled __type and AWS's message text on any refusal, and a deployment that reaches a terminal rolloutState in bounded time.

## Steps to Reproduce
Causes 1-3 — fixture ecs-ecsnrt-traj-zero-capacity-provider-strategy-repair (steps setup#11, teardown#18, teardown#19):
1. Build and boot the emulator from this checkout and replay that fixture from trace set simparity-wire-aws-v1.
2. Equivalently by hand: EC2 CreateLaunchTemplate; Auto Scaling CreateAutoScalingGroup with zero capacity; ECS CreateCapacityProvider with an autoScalingGroupProvider over that group; ECS CreateCluster naming that provider in capacityProviders with a defaultCapacityProviderStrategy over it; RegisterTaskDefinition.
3. Cause 1: CreateService with that cluster, a serviceName, the task definition, desiredCount 0 and capacityProviderStrategy [{capacityProvider: <provider>, base: 0, weight: 1}].
   -> The emulator answers HTTP 400 with no body member; AWS answers 200 with the full Service element carrying that capacityProviderStrategy and a PRIMARY deployment.
4. Cause 2: UpdateService the service to desiredCount 0, then DeleteService with that cluster/service.
   -> The emulator answers HTTP 400 with no `service`; AWS answers 200 with the Service at status DRAINING/INACTIVE (or, for an identifier that resolves to nothing, 400 ServiceNotFoundException "The specified service wasn't found.").
5. Cause 3: DeleteCluster with that cluster (after the service is gone).
   -> The emulator answers HTTP 400 with no `cluster`; AWS answers 200 with the Cluster at status INACTIVE (or a specific ClusterContains* / ClusterNotFoundException when a precondition genuinely blocks it).

Cause 4 — fixture ecs-ecsdep-traj-circuit-breaker-rollback-vs-stop, step task#15:
1. Replay that fixture and observe step task#15.
2. Equivalently by hand: CreateCluster; RegisterTaskDefinition for a FARGATE/awsvpc task definition with a launchable image (revision 1); CreateService desiredCount=1, launchType FARGATE, deploymentController ECS, deploymentConfiguration.deploymentCircuitBreaker {enable: true, rollback: true}; wait for a steady state.
3. RegisterTaskDefinition a second revision whose tasks cannot start (unpullable image) and UpdateService taskDefinition=<revision 2>.
4. Poll DescribeServices for the new deployment to reach rolloutState FAILED with a rolloutStateReason naming the deployment circuit breaker, then for the PRIMARY deployment to point back at revision 1 with runningCount == desiredCount.
   -> Against the emulator the poll never satisfies either condition; it keeps answering with a non-terminal deployment until the caller's budget is exhausted (here the fixture's 960s), and the fixture's remaining steps — including its StopServiceDeployment comparison arm — never run.
5. Contrast arm: ListServiceDeployments / DescribeServiceDeployments for the same service -> the service deployment record never reaches a terminal status/statusReason either, so a circuit-breaker rollback cannot be told apart from a StopServiceDeployment'd rollout.
