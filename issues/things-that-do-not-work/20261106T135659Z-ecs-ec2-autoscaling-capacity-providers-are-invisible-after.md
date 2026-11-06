Issue-Source: trace-sweep-divergence
Issue-Trace: ecs-ecsnrt-traj-zero-capacity-provider-strategy-repair
Issue-Step: task#12
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ecs-ecsnrt-traj-zero-capacity-provider-strategy-repair
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs|DescribeCapacityProviders|2xx->2xx|$.capacityProviders[*]
Issue-Key: ecs|DescribeCapacityProviders|2xx->2xx|$.failures[*]
Issue-Key: ecs|CreateCluster|2xx->4xx|status
Issue-Key: ecs|RunTask|4xx->4xx|$.__type
Issue-Key: ecs|UpdateService|2xx->4xx|$.service
Issue-Key: ecs|UpdateService|2xx->4xx|status
Issue-Related: 20261103T150504Z-ecs-error-paths-answer-with-the-wrong-exception.md
Issue-Commit: dabd605678ea57cce68275edff8c08147ffd283a

# ECS EC2_AUTOSCALING capacity providers are invisible after creation, so cluster creation, RunTask and UpdateService on them all diverge

> _Note: this issue was identified against the code base at commit `dabd605678ea57cce68275edff8c08147ffd283a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeCapacityProviders

## Description
Four net-new ECS root causes sharing one fault: an EC2_AUTOSCALING capacity provider that was accepted at creation is not resolvable afterwards, so every operation that has to look it up — DescribeCapacityProviders, CreateCluster, RunTask and UpdateService — diverges. All four must be closed before this issue is submitted.

Cause 1 of 4: DescribeCapacityProviders reports a just-created capacity provider as MISSING. Fixture ecs-ecsnrt-traj-zero-capacity-provider-strategy-repair, step task#12.
AWS: Smithy (com.amazonaws.ecs) DescribeCapacityProviders, input member capacityProviders documented as "The short name or full Amazon Resource Name (ARN) of one or more capacity providers. Up to 100 capacity providers can be described in an action." — so a provider is resolvable by either form. Output capacityProviders is CapacityProviders -> CapacityProvider, whose members include capacityProviderArn, name, status (CapacityProviderStatus; "Only capacity providers in an ACTIVE state can be used in a cluster"), type (CapacityProviderType), tags and autoScalingGroupProvider ("The Auto Scaling group settings for the capacity provider"). After a successful CreateCapacityProvider, AWS answers 200 with that provider described — status ACTIVE, type EC2_AUTOSCALING, tags as an empty list when none were applied, and autoScalingGroupProvider echoing autoScalingGroupArn, managedScaling, managedDraining and managedTerminationProtection — and failures empty.
Emulator: capacityProviders comes back empty and the provider is reported in failures with reason 'MISSING', addressed by the same identifier the request used. Hypothesis: the capacity-provider lookup only matches the short name (so the full-ARN form never resolves), or CreateCapacityProvider's record is not persisted into the state DescribeCapacityProviders reads.
Why: a client cannot confirm that the capacity provider it just created exists, cannot read back its managedScaling/managedTerminationProtection settings, and is told a resource it owns is MISSING — the response AWS reserves for a provider that genuinely does not exist. Any provisioning flow that polls for status ACTIVE before associating the provider with a cluster never proceeds.
Fix: the ECS DescribeCapacityProviders handler and the capacity-provider lookup in the ecs service crate's state — resolve each requested identifier as either a short name or a full capacity-provider ARN against the providers CreateCapacityProvider stored, and only emit a MISSING failure for identifiers that resolve to nothing. Open issue 20261102T101202Z-cloudformation-ecs-a-cloudformation-created-aws-ecs-capacityprovider.md covers the CloudFormation-created path and does not carry this step's replay key.
Traces: ecs-ecsnrt-traj-zero-capacity-provider-strategy-repair#task#12.

Cause 2 of 4: CreateCluster naming that capacity provider is refused 400 where AWS answers 200. Same fixture, step setup#8.
AWS: Smithy CreateCluster takes capacityProviders (StringList) and defaultCapacityProviderStrategy (CapacityProviderStrategy) and returns CreateClusterResponse.cluster; its modelled errors are AccessDeniedException, ClientException, InvalidParameterException, NamespaceNotFoundException and ServerException. A CreateCluster that associates an existing ACTIVE capacity provider with a default strategy over it is valid: AWS answers HTTP 200 with the Cluster element (clusterArn, clusterName, status ACTIVE, capacityProviders, defaultCapacityProviderStrategy). There is no refusal for this input.
Emulator: the call is refused with HTTP 400 and no cluster is created, so the cluster does not exist for the rest of the fixture (the following DescribeClusters poll reports the cluster ARN with reason MISSING). Hypothesis: the association validation fails the same lookup as cause 1 and rejects the request as naming an unknown capacity provider.
Why: the standard EC2-capacity bootstrap (launch template -> Auto Scaling group -> capacity provider -> cluster associating it) cannot complete at all, and a 400 on CreateCluster gives the client no way to tell a bad request from an emulator that cannot find its own provider.
Fix: the ECS CreateCluster handler's capacity-provider association validation in the ecs service crate — resolve names/ARNs against stored providers, associate them, and answer 200 with the Cluster carrying capacityProviders and defaultCapacityProviderStrategy.
Traces: ecs-ecsnrt-traj-zero-capacity-provider-strategy-repair#setup#8.

Cause 3 of 4: RunTask against a capacity provider with no container instances answers the wrong exception class and a message about a different fault. Same fixture, step task#13.
AWS: Smithy RunTask models InvalidParameterException and ClusterNotFoundException separately. When the cluster exists and the requested capacityProviderStrategy names a provider whose Auto Scaling group has zero capacity, AWS refuses with HTTP 400, __type InvalidParameterException and the message "The specified capacity provider '<name>' contains no container instances." — fixed text "The specified capacity provider '" ... "' contains no container instances.", with the quoted value taken verbatim from the capacityProvider named in the request's capacityProviderStrategy.
Emulator: HTTP 400 with __type 'ClusterNotFoundException' and message 'Cluster not found.' — a refusal about a different resource. Two defects compose here: the cluster is absent only because cause 2 refused its creation, and independently the emulator has no zero-capacity capacity-provider refusal to answer with once the cluster does exist.
Why: a client cannot act on the refusal. 'Cluster not found.' tells it to create a cluster it already created, while AWS tells it exactly which capacity provider has no instances — the one actionable signal in a zero-capacity scenario — and the wrong __type defeats error-class-based retry/branching.
Fix: the ECS RunTask handler's capacity-provider resolution and refusal path in the ecs service crate — once cause 2 is fixed, validate the requested capacityProviderStrategy against the provider's available container instances and raise InvalidParameterException with AWS's message, reserving ClusterNotFoundException for an actually absent cluster.
Traces: ecs-ecsnrt-traj-zero-capacity-provider-strategy-repair#task#13.

Cause 4 of 4: UpdateService repairing a capacityProviderStrategy is refused 400 where AWS answers 200 with the updated service. Same fixture, steps task#15 and task#17.
AWS: Smithy UpdateService returns UpdateServiceResponse.service — the full Service element, including capacityProviderStrategy, clusterArn, deploymentConfiguration, deploymentController, deployments (each with capacityProviderStrategy, rolloutState, stabilityStatus, status PRIMARY, taskDefinition) and currentServiceRevisions. Changing a service's capacityProviderStrategy is a supported update and AWS answers HTTP 200 with the service reflecting the new strategy and a new PRIMARY deployment.
Emulator: HTTP 400 with no service body, both on the repair call and on its re-issue. Downstream of causes 1 and 2 (the cluster and therefore the service never came into being), but the contract this step asserts — a capacityProviderStrategy repair returning the updated service — is also unimplemented behavior in its own right.
Why: a client that repairs a mis-configured capacity-provider strategy gets a refusal instead of the updated service, cannot read back the strategy that is now in force, and has no deployment to monitor.
Fix: the ECS UpdateService handler and Service serializer in the ecs service crate — accept a capacityProviderStrategy change, record it on the service and its new PRIMARY deployment, and answer 200 with the full Service element.
Traces: ecs-ecsnrt-traj-zero-capacity-provider-strategy-repair#task#15 (and the identical re-issue at task#17).

## Evidence
Wire replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout. All four causes come from fixture ecs-ecsnrt-traj-zero-capacity-provider-strategy-repair, service ecs.

Cause 1 — step task#12, DescribeCapacityProviders, expected 200 / actual 200:
  $.capacityProviders[0]: missing (expected a provider with name/capacityProviderArn for the just-created provider, status 'ACTIVE', type 'EC2_AUTOSCALING', tags [], autoScalingGroupProvider {autoScalingGroupArn, managedDraining 'ENABLED', managedScaling {instanceWarmupPeriod 300, maximumScalingStepSize 10000, minimumScalingStepSize 1, scalingApproach 'SCALING_POLICY', status 'DISABLED', targetCapacity 100}, managedTerminationProtection 'DISABLED'}; got None)
  $.failures[0]: extra (got {'arn': '<the capacity-provider ARN requested>', 'reason': 'MISSING'})
(The provider name/ARN and the ASG ARN are per-run values; the requirement is that the created provider is described and failures is empty.)

Cause 2 — step setup#8, CreateCluster: status expected 200, got 400 (no cluster element). The following DescribeClusters poll (setup#9) then returns clusters: [] with failures [{arn: '<cluster ARN>', reason: 'MISSING'}], confirming no cluster was created.

Cause 3 — step task#13, RunTask, expected 400 / actual 400 but the wrong error:
  $.__type: value_mismatch (expected 'InvalidParameterException', got 'ClusterNotFoundException')
  $.message: value_mismatch (expected "The specified capacity provider '<provider name from the request>' contains no container instances.", got 'Cluster not found.')
Message anatomy: fixed text "The specified capacity provider '" + "' contains no container instances."; the quoted provider name is the value from the request's capacityProviderStrategy.

Cause 4 — steps task#15 and task#17, UpdateService: status expected 200, got 400.
  $.service: missing (expected the full Service element — capacityProviderStrategy over the provider, clusterArn, createdAt/createdBy, currentServiceRevisions, deploymentConfiguration {bakeTimeInMinutes, deploymentCircuitBreaker, maximumPercent, minimumHealthyPercent, strategy 'ROLLING'}, deploymentController {type 'ECS'}, deployments[] with rolloutState/stabilityStatus/status 'PRIMARY'/taskDefinition; got None)
(The recorded ARNs, revision ids and createdAt timestamps are per-run values; the requirement is 200 with the service reflecting the new strategy.)

## Steps to Reproduce
1. Build and boot the emulator from this checkout; replay trace set simparity-wire-aws-v1, fixture ecs-ecsnrt-traj-zero-capacity-provider-strategy-repair.
2. Create a launch template and a zero-capacity Auto Scaling group, then call ECS CreateCapacityProvider with an autoScalingGroupProvider over that group (the emulator accepts this with 200).
3. Cause 1: call DescribeCapacityProviders with that provider's full ARN (and/or its short name). The emulator answers 200 with capacityProviders: [] and failures: [{arn: <requested ARN>, reason: 'MISSING'}]; AWS describes the provider with status ACTIVE, type EC2_AUTOSCALING, tags [] and its autoScalingGroupProvider.
4. Cause 2: call CreateCluster with capacityProviders naming that provider and a defaultCapacityProviderStrategy over it. The emulator answers 400 and creates no cluster (a following DescribeClusters reports the cluster MISSING); AWS answers 200 with the Cluster.
5. Cause 3: call RunTask on that cluster with a capacityProviderStrategy naming the zero-capacity provider. The emulator answers 400 ClusterNotFoundException 'Cluster not found.'; AWS answers 400 InvalidParameterException "The specified capacity provider '<name>' contains no container instances.".
6. Cause 4: call UpdateService on the cluster's service to repair its capacityProviderStrategy. The emulator answers 400 with no body; AWS answers 200 with the full Service element carrying the new strategy and a new PRIMARY deployment.
