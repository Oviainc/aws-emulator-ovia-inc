Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.escc-assign-public-ip-rejected-for-the-ec2-launch-type ecs.escc-negative-grace-period-rejected ecs.escc-windows-service-rejects-a-linux-platform-version ecs.fargate-service-on-a-deleted-subnet-rejected ecs.cluster-capacity-provider-replacement-drops-fargate-spot-from-strategies ecs.ecsdeploy-cluster-recreate-with-conflicting-settings-rejected ecs.describe-services-resolves-a-name-only-within-its-cluster
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs.escc-assign-public-ip-rejected-for-the-ec2-launch-type|_error#6|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it")
Issue-Key: ecs.escc-negative-grace-period-rejected|_error#6|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it")
Issue-Key: ecs.escc-windows-service-rejects-a-linux-platform-version|_error#6|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it")
Issue-Key: ecs.fargate-service-on-a-deleted-subnet-rejected|_error#12|AssertionError|raise AssertionError("the call succeeded; real AWS refuses it")
Issue-Key: ecs.cluster-capacity-provider-replacement-drops-fargate-spot-from-strategies|_error#11|AssertionError|raise AssertionError("the call succeeded; real AWS refuses it")
Issue-Key: ecs.ecsdeploy-cluster-recreate-with-conflicting-settings-rejected|_error#7|AssertionError|raise AssertionError(f"the call SUCCEEDED with {result!r}; real AWS rejects it")
Issue-Key: ecs.describe-services-resolves-a-name-only-within-its-cluster|_error#14|AssertionError|raise AssertionError("the call succeeded; real AWS refuses it")
Issue-Commit: a2c0c19d1874b6d8f510c899cc1a51333f7d4b07

# ECS CreateService/RunTask/CreateCluster/DescribeServices accept seven request classes AWS refuses (assignPublicIp on EC2, negative grace period, Windows platform version, deleted subnet, unassociated capacity provider, conflicting cluster re-create, cross-cluster service ARN)

> _Note: this issue was identified against the code base at commit `a2c0c19d1874b6d8f510c899cc1a51333f7d4b07`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateService

## Description
Seven independent validation gates that real ECS applies at the service, cluster and task-run surfaces are absent in the emulator, so each request succeeds with HTTP 200 where AWS answers HTTP 400. Each is a separate missing check in a different handler, bundled here because they share the `ecs` service. Seven causes, numbered below — all seven must be closed before this issue is done.

Cause 1 of 7: CreateService accepts networkConfiguration.awsvpcConfiguration.assignPublicIp=ENABLED for launchType EC2.
AWS: Smithy: CreateService.launchType -> LaunchType enum, networkConfiguration -> NetworkConfiguration -> AwsVpcConfiguration.assignPublicIp -> AssignPublicIp enum; the combination is refused at the service with the modeled InvalidParameterException (httpError 400). Recorded behavior: HTTP 400 InvalidParameterException with the fixed message `Assign public IP is not supported for this launch type.` (no request values interpolated), no service created, while the identical request with assignPublicIp=DISABLED is accepted and reads back ACTIVE.
Emulator: returns 200 and creates the service.
Why: public IP assignment is only meaningful for Fargate ENIs; a client that depends on the refusal believes its EC2-launch tasks are internet-reachable and ships a networking design AWS will not honour.
Fix: CreateService validation (the launchType/awsvpcConfiguration cross-check), services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/create_service.rs.
Traces: ecs.escc-assign-public-ip-rejected-for-the-ec2-launch-type.

Cause 2 of 7: CreateService accepts healthCheckGracePeriodSeconds = -1.
AWS: Smithy: CreateService.healthCheckGracePeriodSeconds -> BoxedInteger with no @range, so the lower bound is enforced by the service; the modeled client error is InvalidParameterException 400. Recorded behavior: HTTP 400 InvalidParameterException with the fixed message `healthCheckGracePeriod must be at least 0`, no service created; the boundary value 0 is accepted and reads back as 0.
Emulator: returns 200 and creates the service with the negative grace period.
Why: a negative grace period is stored and later used in deployment/health arithmetic, so the emulated service's health evaluation cannot match AWS, and the negative test passes wrongly.
Fix: CreateService validation (numeric bounds on healthCheckGracePeriodSeconds), same handler.
Traces: ecs.escc-negative-grace-period-rejected.

Cause 3 of 7: CreateService accepts a Linux-only platformVersion for a Windows Fargate task definition.
AWS: Smithy: CreateService.platformVersion is a String and the operation models PlatformUnknownException (httpError 400) and PlatformTaskDefinitionIncompatibilityException (400); the platform catalogue is per operating system (Windows Fargate offers only 1.0.0). Recorded behavior: pinning a Windows Fargate service to the Linux platform version yields HTTP 400 PlatformUnknownException with the fixed message `The specified platform does not exist.`, no service created; the Windows platform version is accepted and read back literally.
Emulator: returns 200 and creates the service.
Why: the emulator answers 200 for a platform that does not exist, so a client cannot discover the per-OS platform catalogue at all and a mispinned service looks healthy; the modeled PlatformUnknownException is never exercised.
Fix: CreateService validation (resolve platformVersion against the task definition's runtimePlatform.operatingSystemFamily), same handler.
Traces: ecs.escc-windows-service-rejects-a-linux-platform-version.

Cause 4 of 7: CreateService accepts an awsvpcConfiguration naming a subnet that has been deleted.
AWS: Smithy: CreateService errors include InvalidParameterException 400; ECS resolves every subnet in awsvpcConfiguration.subnets against EC2 before creating the service. Recorded behavior: HTTP 400 InvalidParameterException whose message wraps the EC2 lookup failure — `Error retrieving subnet information for [<subnetId>]: The subnet ID '<subnetId>' does not exist (ErrorCode: InvalidSubnetID.NotFound)`, where `<subnetId>` is the subnet id from the request and the rest is fixed text — and ListServices on the cluster stays empty. The identical request on a live subnet of the same VPC creates the service with that subnet.
Emulator: returns 200 and creates the service on the deleted subnet.
Why: ECS never validates its network inputs, so a service references a subnet that cannot exist; the caller only learns at task placement, and infrastructure teardown ordering bugs (delete subnet, then create service) go undetected.
Fix: CreateService validation (subnet/security-group resolution against the EC2 state before the service is persisted), same handler.
Traces: ecs.fargate-service-on-a-deleted-subnet-rejected.

Cause 5 of 7: RunTask accepts a capacityProviderStrategy naming a capacity provider that is not associated with the cluster.
AWS: Smithy: RunTask.capacityProviderStrategy -> CapacityProviderStrategy; errors include InvalidParameterException 400. PutClusterCapacityProviders replaces (not merges) the cluster's provider list, so narrowing it drops the other provider, and a RunTask strategy naming the dropped provider is refused with HTTP 400 InvalidParameterException and the fixed message `The specified capacity provider strategy cannot contain a capacity provider that is not associated with the cluster. Associate the capacity provider with the cluster or specify a valid capacity provider and try again.` — no request values are interpolated. Re-associating the provider lifts the refusal, and a strategy naming a still-associated provider passes this check.
Emulator: RunTask returns success for the strategy naming the unassociated provider.
Why: the cluster's capacity-provider association becomes unobservable — a client cannot tell whether PutClusterCapacityProviders replaced or merged the list, and a run that AWS would refuse appears to launch.
Fix: RunTask validation (check each strategy entry against the cluster's associated capacity providers in state), services/.../src/ops/run_task.rs. Related open issues 20261022T093447Z and 20261102T104402Z cover other capacity-provider gaps; this cross-check is not among them.
Traces: ecs.cluster-capacity-provider-replacement-drops-fargate-spot-from-strategies.

Cause 6 of 7: CreateCluster on an existing cluster name accepts arguments that conflict with the original request.
AWS: Smithy: CreateCluster.clusterName/.settings/.tags, errors ClientException 400 and InvalidParameterException 400. CreateCluster is idempotent: an identical re-create returns the existing clusterArn, but a re-create whose arguments differ is refused with HTTP 400 InvalidParameterException and the fixed message `Arguments on this idempotent request are inconsistent with arguments used in previous request(s).`, leaving the original setting untouched.
Emulator: returns 200 with the cluster body and the NEW settings applied (containerInsights flipped), so the refused call silently mutates the cluster.
Why: a repeated deploy with drifted parameters silently rewrites cluster configuration instead of failing, which is the opposite of the idempotency contract clients build re-runnable deploys on.
Fix: CreateCluster's idempotency handling and validation (compare the request against the stored cluster and refuse on conflict instead of overwriting), services/.../src/ops/create_cluster.rs.
Traces: ecs.ecsdeploy-cluster-recreate-with-conflicting-settings-rejected.

Cause 7 of 7: DescribeServices accepts a service ARN belonging to another cluster.
AWS: Smithy: DescribeServices.cluster (String) and .services (StringList, required); errors include InvalidParameterException 400 and ClusterNotFoundException 400. A service *name* that does not exist in the named cluster comes back as a per-item failure with reason MISSING carrying that cluster's ARN; a fully-qualified service ARN that belongs to a DIFFERENT cluster is refused outright with HTTP 400 InvalidParameterException and the fixed message `Service is not within the provided Cluster.` The home cluster resolves the same service by name and by ARN alike as ACTIVE.
Emulator: the call succeeds, describing the other cluster's service through the wrong cluster.
Why: cluster scoping is the ECS tenancy boundary — resolving an ARN across clusters lets a caller read and (via the same resolution path) act on services outside the cluster it addressed, and masks misconfigured cluster parameters in client code.
Fix: DescribeServices' reference resolution (reject a service ARN whose cluster segment does not match the request's cluster), services/.../src/ops/describe_services.rs.
Traces: ecs.describe-services-resolves-a-name-only-within-its-cluster.

## Evidence
Replay output (verdict mismatch for all seven):
- ecs.escc-assign-public-ip-rejected-for-the-ec2-launch-type — `create_service(cluster=..., serviceName=..., taskDefinition=..., desiredCount=0, launchType="EC2", networkConfiguration={"awsvpcConfiguration": {"subnets": [<subnet>], "assignPublicIp": "ENABLED"}})` -> AssertionError "the call succeeded; real AWS rejects it". Expected ('InvalidParameterException', 'Assign public IP is not supported for this launch type.').
- ecs.escc-negative-grace-period-rejected — `create_service(..., launchType="FARGATE", healthCheckGracePeriodSeconds=-1)` -> "the call succeeded; real AWS rejects it". Expected ('InvalidParameterException', 'healthCheckGracePeriod must be at least 0').
- ecs.escc-windows-service-rejects-a-linux-platform-version — `create_service(..., taskDefinition=<WINDOWS_SERVER_2019_FULL fargate td>, launchType="FARGATE", platformVersion="1.4.0")` -> "the call succeeded; real AWS rejects it". Expected ('PlatformUnknownException', 'The specified platform does not exist.').
- ecs.fargate-service-on-a-deleted-subnet-rejected — subnet created then `ec2.delete_subnet`, then `create_service(..., networkConfiguration={"awsvpcConfiguration": {"subnets": [<deleted subnet>], "assignPublicIp": "DISABLED"}})` -> "the call succeeded; real AWS refuses it". Expected ('InvalidParameterException', "Error retrieving subnet information for [<subnetId>]: The subnet ID '<subnetId>' does not exist (ErrorCode: InvalidSubnetID.NotFound)") and no service listed.
- ecs.cluster-capacity-provider-replacement-drops-fargate-spot-from-strategies — after PutClusterCapacityProviders narrowed the cluster's list, `run_task(cluster=..., taskDefinition=..., capacityProviderStrategy=<names the dropped provider>)` -> "the call succeeded; real AWS refuses it". Expected ('InvalidParameterException', 'The specified capacity provider strategy cannot contain a capacity provider that is not associated with the cluster. Associate the capacity provider with the cluster or specify a valid capacity provider and try again.').
- ecs.ecsdeploy-cluster-recreate-with-conflicting-settings-rejected — `create_cluster(clusterName=<existing>, tags=<same>, settings=[{"name": "containerInsights", "value": "disabled"}])` after the cluster was created with containerInsights enabled -> "the call SUCCEEDED with {'cluster': {'clusterArn': 'arn:aws:ecs:<region>:<account>:cluster/<name>', 'status': 'ACTIVE', ..., 'settings': [{'name': 'containerInsights', 'value': 'enabled'}]}, 'ResponseMetadata': {'HTTPStatusCode': 200}}; real AWS rejects it". Expected ('InvalidParameterException', 'Arguments on this idempotent request are inconsistent with arguments used in previous request(s).').
- ecs.describe-services-resolves-a-name-only-within-its-cluster — `describe_services(cluster=<other cluster>, services=[<home cluster service ARN>])` -> "the call succeeded; real AWS refuses it". Expected ('InvalidParameterException', 'Service is not within the provided Cluster.').
Expected in every case: HTTP 400 with the modeled ECS client error named above. Actual: HTTP 200 with the mutation applied or the read answered.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Re-run the seven checks listed in trace_ids (all under the `ecs` service heading).
3. Per cause, issue the call shown in Evidence against the emulator and observe HTTP 200 instead of HTTP 400 with the quoted error code and message; for Cause 6 additionally DescribeClusters with include=["SETTINGS"] and observe the containerInsights setting was flipped by the call AWS would have refused.
