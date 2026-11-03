Issue-Source: trace-sweep-divergence
Issue-Trace: ecs-alarm-firing-describe-services-mistakes
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ecs-alarm-firing-describe-services-mistakes ecs-escc-traj-describe-services-cluster-addressing-mistakes ecs-escc-traj-describe-services-name-list-mistakes ecs-ecracc-fargate-ecr-image-execution-role-vs-task-role ecs-escc-traj-mirror-custom-service-create-refusals ecs-ecscm-traj-attach-registry-via-update-service ecs-ecsdep-traj-update-circuit-breaker-partial-struct ecs-ecsnrt-traj-zero-capacity-provider-strategy-repair ecs-escc-traj-update-custom-settings-refusals ecs-ntraf-service-binding-accepted-once-a-listener-forwards ecs-ecscm-traj-cloudmap-service-deleted-under-ecs-service ecs-ecscm-traj-create-service-launch-mistakes ecs-ecscm-traj-registry-triplet-mistakes-bridge ecs-ecsdep-traj-create-deployment-configuration-refusals ecs-ntraf-service-load-balancer-block-mistakes ecs-ecsdep-traj-service-deployment-read-mistakes ecs-ecsnrt-traj-run-task-awsvpc-network-configuration-mistakes ecs-ecsnrt-traj-run-task-launch-selection-mistakes ecs-ecsnrt-traj-task-listing-and-describe-mistakes ecs-escc-traj-list-services-filter-and-paging-mistakes
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs|DescribeServices|4xx->2xx|$.__type
Issue-Key: ecs|DescribeServices|4xx->2xx|$.message
Issue-Key: ecs|DescribeServices|4xx->2xx|status
Issue-Key: ecs|RegisterTaskDefinition|4xx->2xx|$.__type
Issue-Key: ecs|RegisterTaskDefinition|4xx->2xx|$.message
Issue-Key: ecs|RegisterTaskDefinition|4xx->2xx|status
Issue-Key: ecs|UpdateService|4xx->2xx|$.__type
Issue-Key: ecs|UpdateService|4xx->2xx|$.message
Issue-Key: ecs|UpdateService|4xx->2xx|status
Issue-Key: ecs|CreateService|4xx->2xx|$.__type
Issue-Key: ecs|CreateService|4xx->2xx|$.message
Issue-Key: ecs|CreateService|4xx->2xx|status
Issue-Key: ecs|ListServiceDeployments|4xx->2xx|$.__type
Issue-Key: ecs|ListServiceDeployments|4xx->2xx|$.message
Issue-Key: ecs|ListServiceDeployments|4xx->2xx|status
Issue-Key: ecs|RunTask|4xx->2xx|$.__type
Issue-Key: ecs|RunTask|4xx->2xx|$.message
Issue-Key: ecs|RunTask|4xx->2xx|status
Issue-Key: ecs|DescribeTasks|4xx->2xx|$.__type
Issue-Key: ecs|DescribeTasks|4xx->2xx|$.message
Issue-Key: ecs|DescribeTasks|4xx->2xx|status
Issue-Key: ecs|ListServices|4xx->2xx|$.__type
Issue-Key: ecs|ListServices|4xx->2xx|$.message
Issue-Key: ecs|ListServices|4xx->2xx|status
Issue-Commit: 34977689197ee7e7ec3a117f2c4937da4e6f0436

# ECS accepts eight request classes AWS refuses, answering 200 with no error

> _Note: this issue was identified against the code base at commit `34977689197ee7e7ec3a117f2c4937da4e6f0436`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeServices

## Description
Eight net-new causes, one fault: a request AWS refuses with a 400 is accepted by the emulator and answered 200 with a success body. Open issues 20261024T081305Z-ecs-is-overly-permissive and 20261103T041326Z-ecs-createservice-runtask-createcluster-describeservices-accept-seven-request cover a different set of request classes and carry none of these replay keys.

Cause 1 of 8: DescribeServices with the same service named twice. AWS: 400 InvalidParameterException, fixed text 'Malformed request with duplicate service names.' (no variable part). Smithy: DescribeServicesRequest.services is a @required StringList. Emulator: 200 with the service described once, no __type and no message. Why: a client that accidentally duplicates a name gets a silently de-duplicated list, so index-based pairing of request entries to response entries is off by one and the bug is invisible. Fix: DescribeServices's validation. Traces: ecs-alarm-firing-describe-services-mistakes#task#2 (cluster of 5 steps across ecs-alarm-firing-describe-services-mistakes, ecs-escc-traj-describe-services-cluster-addressing-mistakes, ecs-escc-traj-describe-services-name-list-mistakes).

Cause 2 of 8: RegisterTaskDefinition for a Fargate task definition whose container image is in ECR but which declares no executionRoleArn. AWS: 400 ClientException, fixed text 'Fargate requires task definition to have execution role ARN to support ECR images.' Smithy: RegisterTaskDefinition models ClientException; executionRoleArn is optional in the model, so this is a service-side rule. Emulator: 200, the definition registers. Why: the client believes a task definition that can never pull its image is valid, and only discovers it when the task fails to start. Fix: RegisterTaskDefinition's validation. Note open issue 20261102T091623Z-ecs-registertaskdefinition-does-not-expand-bare-role-names names a similar rule but does not carry this step's replay key. Traces: ecs-ecracc-fargate-ecr-image-execution-role-vs-task-role#task#3 (cluster of 4 steps, also ecs-escc-traj-mirror-custom-service-create-refusals).

Cause 3 of 8: UpdateService attaching a serviceRegistries entry when the task definition's networkMode is host or bridge, without containerName/containerPort. AWS: 400 InvalidParameterException, message "When specifying 'host' or 'bridge' for networkMode, values for 'containerName' and 'containerPort' must be specified from the task definition. Registry: " + the registry ARN from the request — the sentence is fixed, the registry ARN is the request value. Emulator: 200, the registry is attached. Why: the service is wired to a Cloud Map registry that can never be populated, and the client gets no signal. Fix: UpdateService's validation of serviceRegistries against the resolved task definition's networkMode. Traces: ecs-ecscm-traj-attach-registry-via-update-service#task#8 (cluster of 7 steps across ecs-ecscm-traj-attach-registry-via-update-service, ecs-ecsdep-traj-update-circuit-breaker-partial-struct, ecs-ecsnrt-traj-zero-capacity-provider-strategy-repair, ecs-escc-traj-update-custom-settings-refusals, ecs-ntraf-service-binding-accepted-once-a-listener-forwards).

Cause 4 of 8: CreateService naming a serviceRegistries registry ARN that no longer exists in Cloud Map. AWS: 400 ClientException, fixed text 'The service registry could not be validated. Route 53 error: "ServiceNotFound."' Emulator: 200, the service is created. Why: ECS never cross-checks the registry, so a dangling registry reference is accepted and the deleted-namespace repair path the recording exercises is never reached. Fix: CreateService's validation — resolve each serviceRegistries entry against servicediscovery state. Traces: ecs-ecscm-traj-cloudmap-service-deleted-under-ecs-service#task#11 (cluster of 8 steps across ecs-ecscm-traj-*, ecs-ecsdep-traj-create-deployment-configuration-refusals, ecs-escc-traj-mirror-custom-service-create-refusals, ecs-ntraf-service-load-balancer-block-mistakes).

Cause 5 of 8: ListServiceDeployments with a createdAt filter older than the retention window. AWS: 400 InvalidParameterException, fixed text 'Before and After time filter must be within the last 3 months.' Smithy: ListServiceDeploymentsRequest.createdAt is a CreatedAt structure with before/after timestamps. Emulator: 200 with a result list. Why: the client is handed results for a window AWS will not serve, so a paging loop written against AWS's bound behaves differently here. Fix: ListServiceDeployments's validation of the createdAt filter. Traces: ecs-ecsdep-traj-service-deployment-read-mistakes#task#9.

Cause 6 of 8: RunTask with an awsvpcConfiguration whose subnets list is empty. AWS: 400 InvalidParameterException, fixed text 'subnets can not be empty.' Emulator: 200 and a task is returned. Why: a task is created with no network placement at all; the client proceeds as if its network configuration took effect. Fix: RunTask's validation of networkConfiguration.awsvpcConfiguration.subnets. Traces: ecs-ecsnrt-traj-run-task-awsvpc-network-configuration-mistakes#task#8 (cluster of 5 steps, also ecs-ecsnrt-traj-run-task-launch-selection-mistakes).

Cause 7 of 8: DescribeTasks with an empty tasks list. AWS: 400 InvalidParameterException, fixed text 'Tasks cannot be empty.' Smithy: DescribeTasksRequest.tasks is @required. Emulator: 200 with empty tasks/failures. Why: an empty-list bug in the caller reads as 'no such tasks' instead of a rejected request. Fix: DescribeTasks's validation. Traces: ecs-ecsnrt-traj-task-listing-and-describe-mistakes#task#7.

Cause 8 of 8: ListServices with a cluster ARN whose region segment is not the endpoint's region. AWS: 400 InvalidParameterException, fixed text 'Invalid Region in ARN'. Emulator: 200 with a service list. Why: a cross-region ARN is silently served from the local region's state, so a client mis-targeting a region gets plausible-looking but wrong data instead of a refusal. Fix: ListServices's validation — parse the cluster ARN and compare its region partition before resolving it. Traces: ecs-escc-traj-describe-services-cluster-addressing-mistakes#task#11 (cluster of 2 steps, also ecs-escc-traj-list-services-filter-and-paging-mistakes).

## Evidence
Replay of trace set simparity-wire-aws-v1; every cluster below has empty owned_by and empty stub_path, and every one is 'status: expected 400, got 200' with $.__type and $.message missing from the actual response.
Cause 1 — expected 400 InvalidParameterException 'Malformed request with duplicate service names.'; got 200.
Cause 2 — expected 400 ClientException 'Fargate requires task definition to have execution role ARN to support ECR images.'; got 200.
Cause 3 — expected 400 InvalidParameterException "When specifying 'host' or 'bridge' for networkMode, values for 'containerName' and 'containerPort' must be specified from the task definition. Registry: <registry-arn>"; got 200.
Cause 4 — expected 400 ClientException 'The service registry could not be validated. Route 53 error: "ServiceNotFound."'; got 200.
Cause 5 — expected 400 InvalidParameterException 'Before and After time filter must be within the last 3 months.'; got 200.
Cause 6 — expected 400 InvalidParameterException 'subnets can not be empty.'; got 200.
Cause 7 — expected 400 InvalidParameterException 'Tasks cannot be empty.'; got 200.
Cause 8 — expected 400 InvalidParameterException 'Invalid Region in ARN'; got 200.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus).
2. Representative: fixture ecs-alarm-firing-describe-services-mistakes, step task#2 — DescribeServices with a duplicated service name; the emulator answers 200 where AWS answers 400.
3. Replay each fixture#step in trace_ids and compare the HTTP status plus $.__type/$.message against the recorded response.
