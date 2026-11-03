Issue-Source: trace-sweep-divergence
Issue-Trace: ecs-ecscm-traj-create-service-launch-mistakes
Issue-Step: task#11
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ecs-ecscm-traj-create-service-launch-mistakes ecs-ecsdep-traj-create-deployment-configuration-refusals ecs-escc-traj-mirror-custom-service-create-refusals ecs-ecscm-traj-cloudmap-service-deleted-under-ecs-service ecs-ecscm-traj-registry-triplet-mistakes-bridge ecs-ecsnrt-traj-run-task-awsvpc-network-configuration-mistakes ecs-ecsnrt-traj-run-task-launch-selection-mistakes ecs-ecscm-traj-private-dns-srv-reference-path ecs-ecscm-traj-reference-http-namespace-registry ecs-ecsdep-traj-update-circuit-breaker-partial-struct ecs-ecsnrt-traj-fargate-unpullable-tag-stopped-record ecs-ecsnrt-traj-task-listing-and-describe-mistakes
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs|CreateService|2xx->4xx|$.service
Issue-Key: ecs|CreateService|2xx->4xx|status
Issue-Key: ecs|DeleteCluster|2xx->4xx|status
Issue-Key: ecs|DeleteService|2xx->4xx|status
Issue-Key: ecs|ListServicesByNamespace|2xx->4xx|$.serviceArns
Issue-Key: ecs|ListServicesByNamespace|2xx->4xx|status
Issue-Key: ecs|UpdateService|2xx->4xx|$.service
Issue-Key: ecs|UpdateService|2xx->4xx|status
Issue-Key: ecs|ListTasks|2xx->4xx|$.taskArns
Issue-Key: ecs|ListTasks|2xx->4xx|status
Issue-Commit: 34977689197ee7e7ec3a117f2c4937da4e6f0436

# ECS refuses six request classes AWS accepts, answering 400 instead of 200

> _Note: this issue was identified against the code base at commit `34977689197ee7e7ec3a117f2c4937da4e6f0436`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateService

## Description
Six net-new causes, one fault: the emulator over-validates and returns 400 for requests AWS serves with 200, so a correct client cannot complete the workflow (including its own teardown). No open issue carries these replay keys.

Cause 1 of 6: CreateService for a valid EC2-launch-type service after a sequence of rejected attempts. AWS: 200 with the full Service body (Smithy: CreateServiceResponse.service -> Service). Emulator: 400 and no service member. Why: the recording's happy path is unreachable — the client cannot create the service it just corrected, and every later step in the trajectory is denied its subject. Fix: CreateService's validation — identify which predicate rejects the corrected request and stop applying it to accepted inputs. Traces: ecs-ecscm-traj-create-service-launch-mistakes#task#11 (cluster of 3 steps, also ecs-ecsdep-traj-create-deployment-configuration-refusals, ecs-escc-traj-mirror-custom-service-create-refusals).

Cause 2 of 6: DeleteCluster at teardown. AWS: 200 with the Cluster body. Smithy: DeleteCluster models ClusterContainsServicesException / ClusterContainsTasksException / ClusterContainsContainerInstancesException, so a 400 here means the emulator still believes the cluster holds services or tasks after they were deleted. Emulator: 400. Why: teardown cannot complete, so resources leak across a client's cleanup and a retry loop on DeleteCluster never terminates. Fix: DeleteCluster's handler and the cluster-contents bookkeeping it reads (see also the stale-listing issue filed from this same replay). Traces: ecs-ecscm-traj-cloudmap-service-deleted-under-ecs-service#teardown#20 (cluster of 4 steps across ecs-ecscm-traj-cloudmap-service-deleted-under-ecs-service, ecs-ecscm-traj-registry-triplet-mistakes-bridge, ecs-ecsnrt-traj-run-task-awsvpc-network-configuration-mistakes, ecs-ecsnrt-traj-run-task-launch-selection-mistakes).

Cause 3 of 6: DeleteService at teardown. AWS: 200 with the Service body, status moving toward INACTIVE (Smithy: DeleteServiceResponse.service). Emulator: 400. Why: the service cannot be torn down, which also blocks the DeleteCluster in cause 2. Fix: DeleteService's handler. Traces: ecs-ecscm-traj-private-dns-srv-reference-path#teardown#14.

Cause 4 of 6: ListServicesByNamespace for a Cloud Map HTTP namespace with no ECS services in it. AWS: 200 with serviceArns = [] (Smithy: ListServicesByNamespaceRequest.namespace @required; NamespaceNotFoundException is modeled, but the namespace here exists). Emulator: 400 with no serviceArns. Why: 'no services in this namespace' is a normal empty result; returning 400 makes a client treat an empty namespace as an error. Fix: ListServicesByNamespace's handler — resolve the namespace and return an empty list. Traces: ecs-ecscm-traj-reference-http-namespace-registry#task#10.

Cause 5 of 6: UpdateService with a deploymentConfiguration that sets only part of the structure (deploymentCircuitBreaker without every sibling). AWS: 200, merging the partial structure over the service's current configuration. Smithy: DeploymentConfiguration's members are all optional. Emulator: 400 with no service member. Why: the SDK-idiomatic partial update is rejected, so a client cannot change one deployment knob without restating the whole structure. Fix: UpdateService's validation/merge of deploymentConfiguration. Traces: ecs-ecsdep-traj-update-circuit-breaker-partial-struct#task#6.

Cause 6 of 6: ListTasks filtered to stopped tasks. AWS: 200 with the matching taskArns list (Smithy: ListTasksRequest.desiredStatus -> DesiredStatus enum). Emulator: 400 with no taskArns. Why: the standard way to enumerate stopped tasks fails outright, so a client cannot find the stopped task it must describe. Fix: ListTasks's handler/validation for the desiredStatus filter. Traces: ecs-ecsnrt-traj-fargate-unpullable-tag-stopped-record#task#13 (cluster of 2 steps, also ecs-ecsnrt-traj-task-listing-and-describe-mistakes).

## Evidence
Replay of trace set simparity-wire-aws-v1; all six clusters have empty owned_by and empty stub_path.
Cause 1 — status expected 200, got 400; $.service missing (recorded body carries serviceArn, clusterArn, deployments[], deploymentConfiguration, launchType 'EC2', ...).
Cause 2 — DeleteCluster: status expected 200, got 400.
Cause 3 — DeleteService: status expected 200, got 400.
Cause 4 — ListServicesByNamespace: status expected 200, got 400; $.serviceArns expected [], got absent.
Cause 5 — UpdateService: status expected 200, got 400; $.service missing (recorded body echoes deploymentConfiguration.deploymentCircuitBreaker {enable true, rollback false} with maximumPercent 150 / minimumHealthyPercent 50 preserved).
Cause 6 — ListTasks: status expected 200, got 400; $.taskArns expected a one-element list, got absent.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus).
2. Representative: fixture ecs-ecscm-traj-create-service-launch-mistakes, step task#11 — a valid CreateService answered 400 instead of 200.
3. Replay each fixture#step in trace_ids and compare the HTTP status and body against the recorded response.
