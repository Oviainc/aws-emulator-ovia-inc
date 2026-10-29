Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.put-attributes-invalid-target-rejected ecs.capacity-provider-delete-nonexistent-fails ecs.cluster-and-task-definition-are-region-scoped ecs.op-contract-createCluster ecs.op-contract-update-cluster ecs.stop-service-deployment-conflict-when-no-deployment ecs.stop-service-deployment-invalid-state-rejected ecs.ecsnrt-firelens-task-definition-round-trips-router-and-derived-attributes ecs.deploy-ecs-capacityprovider-cfn-association-enforced
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs.put-attributes-invalid-target-rejected|run#10|AssertionError|assert code == "TargetNotFoundException", f"expected TargetNotFoundException, got {code!r}"
Issue-Key: ecs.capacity-provider-delete-nonexistent-fails|run#4|AssertionError|assert code == "ClientException", f"unexpected error code: {code}"
Issue-Key: ecs.cluster-and-task-definition-are-region-scoped|run#18|AssertionError|assert away_desc["clusters"] == [], (
Issue-Key: ecs.op-contract-createCluster|_error#23|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: ecs.op-contract-update-cluster|run#13|AssertionError|assert persisted_arn and persisted_arn != sc_namespace_name and ":namespace/ns-" in persisted_arn, (
Issue-Key: ecs.stop-service-deployment-conflict-when-no-deployment|run#28|AssertionError|assert stopped.get("statusReason") == "No rollback candidate was found to run the rollback.", (
Issue-Key: ecs.stop-service-deployment-invalid-state-rejected|run#20|AssertionError|assert stopped_stuck.get("statusReason") == "No rollback candidate was found to run the rollback.", (
Issue-Key: ecs.ecsnrt-firelens-task-definition-round-trips-router-and-derived-attributes|run#18|AssertionError|assert want <= attrs, f"requiresAttributes is missing {sorted(want - attrs)!r}; got {sorted(attrs)!r}"
Issue-Key: ecs.deploy-ecs-capacityprovider-cfn-association-enforced|run#6|AssertionError|assert clusters[0]["capacityProviders"] == ["FARGATE_SPOT"], (
Issue-Commit: e8891ffb15bbac5be85198cac9e3766d8ad3529e

# ecs: 7 net-new root causes in state and derivation — PutAttributes/DeleteCapacityProvider answer NotImplemented, cluster state is not region-partitioned, Service Connect namespaces are never resolved, and three derived read models diverge

> _Note: this issue was identified against the code base at commit `e8891ffb15bbac5be85198cac9e3766d8ad3529e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutAttributes

## Description
Seven independent ECS state/derivation defects. Each cause below must be closed before this issue is submitted.

Cause 1 of 7: PutAttributes answers NotImplemented.
AWS: PutAttributes with an attribute whose `targetId` names a container instance that is not in the cluster is refused TargetNotFoundException (Smithy: PutAttributes models TargetNotFoundException, ClusterNotFoundException, AttributeLimitExceededException, InvalidParameterException and ClientException, all httpError 400) and the attribute is not persisted — ListAttributes afterwards does not report it. This holds whether or not any real container instance is registered in the cluster.
Emulator: the call answers error code NotImplemented, so neither the TargetNotFoundException contract nor the effect-absent guarantee is reachable. The replay report did NOT attribute this to a scaffold-stub issue, so no filed issue covers it.
Why: attribute-based placement constraints cannot be exercised at all, and the client receives an error code that is not in this operation's modelled error set.
Fix: implement src/ops/put_attributes.rs, including the target resolution that raises TargetNotFoundException.
Traces: ecs.put-attributes-invalid-target-rejected.

Cause 2 of 7: DeleteCapacityProvider answers NotImplemented.
AWS: DeleteCapacityProvider naming a capacity provider that was never created is refused ClientException with the message "Failed to get info for provided CapacityProvider" — fixed text, no interpolation. (Smithy: DeleteCapacityProvider models ClientException, ClusterNotFoundException, InvalidParameterException, UpdateInProgressException, and is @idempotent.)
Emulator: the call answers error code NotImplemented. The replay report did NOT attribute this to a scaffold-stub issue, so no filed issue covers it.
Why: capacity-provider teardown is unreachable and the not-found contract on the delete path cannot be observed; a client cleaning up receives an error code outside the operation's modelled set.
Fix: implement src/ops/delete_capacity_provider.rs, including the unknown-provider refusal.
Traces: ecs.capacity-provider-delete-nonexistent-fails.

Cause 3 of 7: ECS state is global rather than region-partitioned.
AWS: a cluster (and a task-definition family) created in one region does not exist in another — there the name is MISSING from DescribeClusters, the home region's ARN is refused as naming another region, the family is undescribable and unlisted, and DeleteCluster is refused — while the home region keeps serving both as ACTIVE.
Emulator: a DescribeClusters issued against a different region returns the home region's cluster in full, ARN and all, so one global store backs every region.
Why: region isolation is a correctness boundary clients depend on: a multi-region deployment cannot be modelled, a cross-region name collision cannot be reproduced, and a client that mistakenly targets the wrong region silently succeeds where AWS fails it.
Fix: key cluster, service and task-definition state by region in state.rs, and have the cluster-scoped read/write handlers resolve only within the request's region.
Traces: ecs.cluster-and-task-definition-are-region-scoped.

Cause 4 of 7: serviceConnectDefaults.namespace is neither validated nor resolved.
AWS: ECS resolves `serviceConnectDefaults.namespace` against Cloud Map at call time. CreateCluster given a well-formed namespace ARN in the caller's own account that names no existing namespace is refused NamespaceNotFoundException (Smithy: CreateCluster and UpdateCluster both model NamespaceNotFoundException, httpError 400) and the cluster does not appear in ListClusters afterwards; given an ARN that does resolve, that ARN is stored and read back. UpdateCluster given a BARE NAME auto-provisions a Cloud Map HTTP namespace with that name and persists the new namespace's ARN (a ":namespace/ns-..." ARN whose Cloud Map Name equals the supplied name), not the raw name.
Emulator: CreateCluster with a nonexistent namespace ARN SUCCEEDS with HTTP 200 and the cluster is created carrying a serviceConnectDefaults.namespace value the emulator fabricated rather than resolved. UpdateCluster with a bare name persists that bare name verbatim and provisions nothing in Cloud Map.
Why: Service Connect wiring cannot be validated at all — a client that mistypes a namespace ARN is told its cluster is ready, and a client that relies on the auto-provisioned namespace existing in Cloud Map (to add services to it, or to delete it) finds nothing there.
Fix: resolve `serviceConnectDefaults.namespace` against servicediscovery state in src/ops/create_cluster.rs and src/ops/update_cluster.rs — refuse an unresolvable ARN with NamespaceNotFoundException, and auto-provision then store the ARN for a bare name.
Traces: ecs.op-contract-createCluster, ecs.op-contract-update-cluster.

Cause 5 of 7: a stopped first deployment's rollback outcome carries the wrong statusReason and no rollback reason.
AWS: StopServiceDeployment against a service's FIRST deployment ends in status ROLLBACK_FAILED (there is no earlier revision to roll back to) with statusReason exactly "No rollback candidate was found to run the rollback." and rollback.reason "Service deployment rolled back by user." — both fixed text, naming no ARN.
Emulator: the status does reach ROLLBACK_FAILED, but statusReason is "This service deployment couldn't be rolled back because service <serviceArn> has no previous service revision." — invented prose interpolating the service ARN — and the recorded rollback.reason is not reported.
Why: statusReason and rollback.reason are how a client or a deployment dashboard explains a failed rollback; the emulator's text is not AWS's, and the missing rollback.reason removes the "stopped by user" attribution entirely.
Fix: the ROLLBACK_FAILED outcome construction in src/ops/stop_service_deployment.rs and the service-deployment projection it writes.
Traces: ecs.stop-service-deployment-conflict-when-no-deployment, ecs.stop-service-deployment-invalid-state-rejected.

Cause 6 of 7: requiresAttributes derivation still omits two capabilities.
AWS: RegisterTaskDefinition derives `requiresAttributes` from the containers' content. A FireLens task definition with a fluentbit router carrying config-file options and a container health check must include, among others, "ecs.capability.firelens.options.config.file" (the router declares config-file-type/config-file-value) and "ecs.capability.container-health-check" (a container declares healthCheck).
Emulator: the derived set contains the docker-remote-api, awsfirelens logging-driver, firelens.fluentbit and task-eni capabilities but BOTH 'ecs.capability.container-health-check' and 'ecs.capability.firelens.options.config.file' are missing.
Why: requiresAttributes is what ECS uses to decide whether a container instance can run a task; an under-derived set makes a definition look placeable on instances that cannot run it, and a client comparing the stored definition with AWS's sees two attributes absent.
Fix: the requiresAttributes derivation in src/ops/register_task_definition.rs — add the healthCheck-driven and firelens-config-file-driven capabilities.
Note: already-fixed issue 20261026T234427Z-ecs-requiresattributes-derivation-omits-the-firelens-and-execution.md addressed part of this derivation (ecs.capability.firelens.fluentbit is now present). This is not a refusal — the call succeeds with 200 — it is the remaining two capabilities still absent from the same derivation.
Traces: ecs.ecsnrt-firelens-task-definition-round-trips-router-and-derived-attributes.

Cause 7 of 7: a CloudFormation ClusterCapacityProviderAssociations resource never reaches the ECS cluster.
AWS: a stack declaring AWS::ECS::ClusterCapacityProviderAssociations with CapacityProviders [FARGATE_SPOT] and a matching DefaultCapacityProviderStrategy makes DescribeClusters report capacityProviders ['FARGATE_SPOT'] and a defaultCapacityProviderStrategy naming FARGATE_SPOT; a RunTask that supplies neither launchType nor capacityProviderStrategy then falls back to that default strategy and the resulting task reports capacityProviderName FARGATE_SPOT.
Emulator: after the stack is provisioned, DescribeClusters on the stack's own cluster ARN reports capacityProviders [] — the association was never applied, so nothing downstream of it (the default strategy, the strategy-driven RunTask placement) can be exercised.
Why: a client that configures capacity providers through CloudFormation gets a cluster with no association while the stack reports success, so placement silently falls back to something other than what was declared.
Fix: the CloudFormation resource provisioner for AWS::ECS::ClusterCapacityProviderAssociations must call through to the ECS PutClusterCapacityProviders path (src/ops/put_cluster_capacity_providers.rs) rather than reporting the resource created.
Traces: ecs.deploy-ecs-capacityprovider-cfn-association-enforced.

## Evidence
Cause 1 — PutAttributes with an attribute targeting a container instance absent from the cluster: expected error code TargetNotFoundException, got 'NotImplemented'.
Cause 2 — DeleteCapacityProvider(capacityProvider=<never created>): expected ClientException with message "Failed to get info for provided CapacityProvider", got code NotImplemented.
Cause 3 — DescribeClusters issued to a second region for a cluster created in the home region: expected clusters == [] (and a failure entry for the name); actual the home cluster was returned in full, e.g. [{'clusterArn': 'arn:aws:ecs:us-west-2:...:cluster/<name>', 'clusterName': '<name>', 'status': 'ACTIVE', ...}] answered to a us-east-1 client.
Cause 4 — CreateCluster with serviceConnectDefaults={'namespace': 'arn:aws:servicediscovery:<region>:<caller account>:namespace/ns-0000000000abcdef'} (well-formed, nonexistent): expected NamespaceNotFoundException and the cluster absent from ListClusters; actual HTTP 200 with the cluster ACTIVE and serviceConnectDefaults.namespace set to a fabricated namespace ARN. UpdateCluster with serviceConnectDefaults={'namespace': '<bare name>'}: expected a persisted ':namespace/ns-...' ARN resolvable via servicediscovery GetNamespace with Name == the bare name; actual {'namespace': '<bare name>'} persisted verbatim.
Cause 5 — after StopServiceDeployment on a first deployment: expected status ROLLBACK_FAILED with statusReason "No rollback candidate was found to run the rollback." and rollback.reason "Service deployment rolled back by user."; actual status ROLLBACK_FAILED with statusReason "This service deployment couldn't be rolled back because service <serviceArn> has no previous service revision." and no matching rollback.reason.
Cause 6 — expected requiresAttributes to include 'ecs.capability.container-health-check' and 'ecs.capability.firelens.options.config.file'; actual ['com.amazonaws.ecs.capability.docker-remote-api.1.18', 'com.amazonaws.ecs.capability.logging-driver.awsfirelens', 'ecs.capability.firelens.fluentbit', 'ecs.capability.task-eni'].
Cause 7 — expected DescribeClusters(<stack cluster>).capacityProviders == ['FARGATE_SPOT'] and a defaultCapacityProviderStrategy naming FARGATE_SPOT; actual capacityProviders == [].
Generated names/ARNs and the 000000000000 account in the recordings are per-run values, not requirements.

## Steps to Reproduce
Cause 1: CreateCluster; ListAttributes(cluster=<arn>, targetType='container-instance'); PutAttributes(cluster=<arn>, attributes=[{name:'k', value:'v', targetType:'container-instance', targetId:'<a container-instance id not in the cluster>'}]).
Cause 2: DeleteCapacityProvider(capacityProvider='<a name never created>').
Cause 3: CreateCluster and RegisterTaskDefinition in region A; with an ECS client in region B call DescribeClusters([<that name>]), DescribeTaskDefinition(<that family>), ListTaskDefinitions(familyPrefix=<family>) and DeleteCluster(<name>).
Cause 4: CreateCluster with serviceConnectDefaults.namespace set to a well-formed-but-nonexistent 'ns-...' ARN in your own account, then ListClusters; separately CreateCluster plain and UpdateCluster(cluster=<name>, serviceConnectDefaults={'namespace': '<bare name>'}), then servicediscovery GetNamespace on the id in the persisted value.
Cause 5: CreateCluster + CreateService(desiredCount=1) on a cluster with no capacity so the first deployment stays IN_PROGRESS; ListServiceDeployments to get its ARN; StopServiceDeployment(serviceDeploymentArn=<arn>); poll DescribeServiceDeployments to a terminal status and read status, statusReason and rollback.reason.
Cause 6: RegisterTaskDefinition(networkMode='awsvpc', requiresCompatibilities=['EC2']) with a 'router' container carrying firelensConfiguration {type:'fluentbit', options:{config-file-type:'file', config-file-value:'/config/fluent-bit.conf', enable-ecs-log-metadata:'false'}} and a healthCheck, plus an 'app' container with logConfiguration {logDriver:'awsfirelens'}; DescribeTaskDefinition and read requiresAttributes.
Cause 7: CreateStack with AWS::ECS::Cluster + AWS::ECS::ClusterCapacityProviderAssociations (CapacityProviders [FARGATE_SPOT], DefaultCapacityProviderStrategy FARGATE_SPOT weight 1), then DescribeClusters on the stack's cluster ARN.
Replay the check ids in trace_ids to confirm.
