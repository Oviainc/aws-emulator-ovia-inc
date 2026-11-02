Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__ecs__elbv2.e2e-ecsnt-diagnose-ecs-cluster-no-traffic-lifecycle ec2__ecs__elbv2.ecsnt-rolling-and-blue-green-services-represent-their-controllers-differently ec2__ecs__elbv2.ecsnt-code-deploy-service-task-definition-and-network-updates-rejected
Issue-Key: ec2__ecs__elbv2.e2e-ecsnt-diagnose-ecs-cluster-no-traffic-lifecycle|phase#8|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Key: ec2__ecs__elbv2.ecsnt-rolling-and-blue-green-services-represent-their-controllers-differently|run#19|AssertionError|assert [(t["status"], t["computedDesiredCount"], t["taskDefinition"]) for t in sets] == [
Issue-Key: ec2__ecs__elbv2.ecsnt-code-deploy-service-task-definition-and-network-updates-rejected|_error#6|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Commit: b1a4a993822b0574a1f24a3d622a9d42702ca481

# ec2__ecs__elbv2: 3 net-new ECS root causes — a cluster's capacity providers are dropped, a CODE_DEPLOY service carries no task set, and UpdateService accepts changes CODE_DEPLOY forbids

> _Note: this issue was identified against the code base at commit `b1a4a993822b0574a1f24a3d622a9d42702ca481`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeServices

## Description
Three net-new ECS root causes exposed by the ec2__ecs__elbv2 checks. The open issues 20261023T205437Z-cloudformation-reports-create-complete-for-resources-it-never and 20261029T175151Z-cloudformation-ecs-elbv2-a-cloudformation-created-aws-ecs concern CloudFormation not creating ECS resources at all; here the cluster and both services DO exist and answer — their content diverges.

Cause 1 of 3: a cluster created with capacity providers reports none.
AWS: DescribeClusters echoes the cluster's capacityProviders — for a cluster declared with FARGATE and FARGATE_SPOT, clusters[0].capacityProviders holds exactly those two names (order unspecified) while status is 'ACTIVE'. Recorded behavior only for the exact list; the requirement is that the submitted providers round-trip.
Emulator: status 'ACTIVE' but capacityProviders [] (and defaultCapacityProviderStrategy []), i.e. the providers submitted at create time are discarded.
Why: any client that picks a capacity provider for RunTask/CreateService off the cluster's own description concludes the cluster supports none, and a Fargate-Spot placement strategy cannot be derived.
Fix: ECS CreateCluster/PutClusterCapacityProviders state (persist capacityProviders) and the DescribeClusters serializer.
Traces: ec2__ecs__elbv2.e2e-ecsnt-diagnose-ecs-cluster-no-traffic-lifecycle (phase 1/4 deploy).

Cause 2 of 3: a CODE_DEPLOY-controlled service carries no task set.
AWS: a service whose deploymentController.type is 'CODE_DEPLOY' is represented by task SETS, not deployments: DescribeServices returns taskSets with one entry whose status is 'PRIMARY', whose computedDesiredCount matches the service's desired count and whose taskDefinition is the service's definition, and it lists no deployments. (An ECS-controlled sibling is the mirror image: one PRIMARY deployment, no taskSets.)
Emulator: the CODE_DEPLOY service's taskSets is empty.
Why: blue/green tooling (CodeDeploy, and any client that reads the live task set before shifting traffic) has nothing to address; the service looks like it has no running configuration at all.
Fix: ECS service state for the CODE_DEPLOY deployment controller — create the PRIMARY task set at CreateService and project it in DescribeServices.
Traces: ec2__ecs__elbv2.ecsnt-rolling-and-blue-green-services-represent-their-controllers-differently.

Cause 3 of 3: UpdateService accepts task-definition and network changes on a CODE_DEPLOY service.
AWS: HTTP 400, Code 'InvalidParameterException', with two fixed messages depending on which member was submitted — 'Unable to update task definition on services with a CODE_DEPLOY deployment controller. Use AWS CodeDeploy to trigger a new deployment.' for taskDefinition, and 'Unable to update network parameters on services with a CODE_DEPLOY deployment controller. Use AWS CodeDeploy to trigger a new deployment.' for networkConfiguration. Both are entirely fixed text with no values interpolated. The service's task set and network configuration are unchanged, while desiredCount and healthCheckGracePeriodSeconds updates ARE accepted on the same service, and a taskDefinition update on an ECS-controlled sibling is accepted.
Emulator: HTTP 200 — the taskDefinition update on the CODE_DEPLOY service succeeds (the check never reaches the networkConfiguration probe).
Why: a client can move a blue/green service's definition behind CodeDeploy's back, producing a state AWS makes impossible and that CodeDeploy's own rollback cannot reconcile.
Fix: UpdateService validation gated on the service's deploymentController.type.
Traces: ec2__ecs__elbv2.ecsnt-code-deploy-service-task-definition-and-network-updates-rejected.

## Evidence
Cause 1: expected (status, sorted(capacityProviders)) == ('ACTIVE', ['FARGATE', 'FARGATE_SPOT']); got the cluster described as {'status': 'ACTIVE', ..., 'capacityProviders': [], 'defaultCapacityProviderStrategy': []} ("[phase 1/4 deploy] deployed cluster: {...'capacityProviders': []...}"). The cluster name/ARN shown are one run's values.
Cause 2: expected taskSets [( 'PRIMARY', <computedDesiredCount>, <taskDefinitionArn> )] for the CODE_DEPLOY service; got "bg task sets: []".
Cause 3: expected ('InvalidParameterException', 'Unable to update task definition on services with a CODE_DEPLOY deployment controller. Use AWS CodeDeploy to trigger a new deployment.'); got a 200 success ("the call succeeded; real AWS rejects it, so there is no error code").

## Steps to Reproduce
Replay the behavioral suite and re-run the ec2__ecs__elbv2 checks in trace_ids. (1) Create an ECS cluster declaring capacityProviders FARGATE and FARGATE_SPOT (the check deploys it via its CloudFormation template) and read DescribeClusters. (2) CreateService with deploymentController {'type': 'CODE_DEPLOY'} and read DescribeServices.taskSets. (3) On that same service call UpdateService with a new taskDefinition (then with a new networkConfiguration) and expect 400 InvalidParameterException with the two sentences above, while desiredCount/healthCheckGracePeriodSeconds updates succeed.
