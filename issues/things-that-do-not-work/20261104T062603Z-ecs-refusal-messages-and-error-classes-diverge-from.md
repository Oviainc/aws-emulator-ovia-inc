Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.create-service-load-balancer-missing-target-group-rejected ecs.update-service-network-configuration-wrong-network-mode-rejected ecs.deploy-ecs-runtask-fargate-incompatible-taskdef-rejected ecs.list-services-by-namespace-filters-and-validates ecs.capacity-provider-removal-while-in-use-rejected ecs.run-task-capacity-provider-strategy-conflict-rejected ecs.run-task-launch-type-and-capacity-provider-conflict-rejected ecs.run-task-platform-incompatibility-rejected ecs.op-contract-createCapacityProvider ecs.service-deployment-managed-orchestration ecs.service-deployment-blue-green-sequential-progress
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs.create-service-load-balancer-missing-target-group-rejected|run#19|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: ecs.update-service-network-configuration-wrong-network-mode-rejected|run#16|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: ecs.deploy-ecs-runtask-fargate-incompatible-taskdef-rejected|run#16|AssertionError|assert message == 'Task definition does not support launch_type FARGATE.', f"expected the exact network-mode message, got {message!r}"
Issue-Key: ecs.list-services-by-namespace-filters-and-validates|run#29|AssertionError|assert not_found == expected, f"expected {expected!r} for a nonexistent namespace, got {not_found!r}"
Issue-Key: ecs.capacity-provider-removal-while-in-use-rejected|run#17|AssertionError|assert message == "The specified capacity provider is in use and cannot be removed.", (
Issue-Key: ecs.run-task-capacity-provider-strategy-conflict-rejected|run#17|AssertionError|assert both_message == "You may choose a capacity provider or a launch type but not both.", f"refusal message {both_message!r}"
Issue-Key: ecs.run-task-launch-type-and-capacity-provider-conflict-rejected|run#20|AssertionError|assert message == "You may choose a capacity provider or a launch type but not both.", f"refusal message {message!r}"
Issue-Key: ecs.run-task-platform-incompatibility-rejected|run#15|AssertionError|assert error.get("Code") == "AccessDeniedException", (
Issue-Key: ecs.op-contract-createCapacityProvider|run#26|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: ecs|ContinueServiceDeployment|InvalidParameterException
Issue-Key: ecs.service-deployment-blue-green-sequential-progress|run#23|AssertionError|assert bad_message == "Invalid hookId: hookId must follow the format: ecs-{hookType}-{id}", f"unexpected message for a bogus hookId: {bad_message!r}"
Issue-Commit: b64c3d45927e2833db57dde0eaeae2a33a5b497f

# ECS refusal messages and error classes diverge from AWS on seven refusals (7 causes)

> _Note: this issue was identified against the code base at commit `b64c3d45927e2833db57dde0eaeae2a33a5b497f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateService

## Description
Seven refusals carry the right rejection but the wrong wire text or the wrong error class. In each case the fixed text is AWS's sentence and the variable parts (ARNs, names, ids, member names) come from the request or the emulator's own state; the recorded per-run values below only fill the template.

Cause 1 of 7: CreateService with a loadBalancers entry naming a nonexistent target group. AWS: InvalidParameterException 'The target group <targetGroupArn> does not exist.' Emulator: 'The target group with targetGroupArn <targetGroupArn> does not exist' — extra 'with targetGroupArn', missing terminal period. Fix: the CreateService load-balancer validation message. Trace: ecs.create-service-load-balancer-missing-target-group-rejected.

Cause 2 of 7: a networkConfiguration supplied for a task definition whose networkMode is not awsvpc. AWS: InvalidParameterException 'Network Configuration is not valid for the given networkMode of this task definition.' Emulator: "Network Configuration must not be provided when networkMode 'bridge' is specified." — emulator-authored prose that also leaks the networkMode. Fix: the shared network-configuration validator used by UpdateService and RunTask. Traces: ecs.update-service-network-configuration-wrong-network-mode-rejected, ecs.deploy-ecs-runtask-fargate-incompatible-taskdef-rejected.

Cause 3 of 7: ListServicesByNamespace for a namespace that does not exist. AWS: NamespaceNotFoundException 'Failed to retrieve namespace for <namespace>' (the namespace from the request). Emulator: NamespaceNotFoundException "The specified namespace wasn't found." — right code, wrong sentence, and the namespace is dropped. Fix: the ListServicesByNamespace not-found message. Trace: ecs.list-services-by-namespace-filters-and-validates.

Cause 4 of 7: disassociating a capacity provider still in use. Emulator: 'capacity provider "FARGATE" is still used by a task of cluster "<cluster>" and cannot be disassociated' — lower-case emulator prose, not AWS's sentence for the in-use condition. Fix: the PutClusterCapacityProviders/UpdateCluster in-use refusal message. Trace: ecs.capacity-provider-removal-while-in-use-rejected.

Cause 5 of 7: RunTask with both launchType and capacityProviderStrategy. AWS (Smithy RunTask.launchType/capacityProviderStrategy docs: "If a launchType is specified, the capacityProviderStrategy parameter must be omitted"): InvalidParameterException 'Specifying both a launch type and capacity provider strategy is not supported. Remove one and try again.' Emulator: a different sentence. Fix: the RunTask conflict refusal message. Traces: ecs.run-task-capacity-provider-strategy-conflict-rejected, ecs.run-task-launch-type-and-capacity-provider-conflict-rejected.

Cause 6 of 7: RunTask naming a withdrawn Fargate platformVersion (1.3.0). AWS refuses with AccessDeniedException. Emulator refuses with PlatformUnknownException — a different modelled error class, so a client's error handling branches differently. Fix: the RunTask platform-version resolution. Trace: ecs.run-task-platform-incompatibility-rejected.

Cause 7 of 7: CreateCapacityProvider with an invalid managed-scaling step size picks the wrong refusal. AWS: InvalidParameterException 'The minimum or maximum scaling step size value is invalid. Specify a minimum scaling step size value that is less than or equal to the maximum scaling step size value and try again.' Emulator answers the unrelated 'The specified Auto Scaling group ARN is already being used by another capacity provider. Specify a unique Auto Scaling group ARN and try again.' — i.e. validation order/selection is wrong. Fix: CreateCapacityProvider validation ordering. Trace: ecs.op-contract-createCapacityProvider.

Bonus of cause 7's family: ContinueServiceDeployment with an invalid hookId answers InvalidParameterException "The specified hookId <hookId> isn't valid for service deployment <arn>. Review the available parameters for the API request." where the check expects AWS's wording for that condition; both blue/green orchestration checks are blocked on it (ecs.service-deployment-managed-orchestration, ecs.service-deployment-blue-green-sequential-progress) and are listed in trace_ids under this cause.

Why: a client that matches on AWS's message (log scrapers, retry classifiers, CDK/Terraform error handling) does not recognise the emulator's text, and cause 6 changes the error class itself.

## Evidence
Replayed details:
1. `AssertionError: expected message 'The target group arn:aws:elasticloadbalancing:us-west-2:000000000000:targetgroup/<tg>/<id> does not exist.', got 'The target group with targetGroupArn arn:aws:elasticloadbalancing:us-west-2:000000000000:targetgroup/<tg>/<id> does not exist'`
2. `AssertionError: expected message 'Network Configuration is not valid for the given networkMode of this task definition.', got "Network Configuration must not be provided when networkMode 'bridge' is specified."` (twice)
3. `AssertionError: expected ('NamespaceNotFoundException', 'Failed to retrieve namespace for <namespace>') for a nonexistent namespace, got ('NamespaceNotFoundException', "The specified namespace wasn't found.")`
4. `AssertionError: unexpected message: 'capacity provider "FARGATE" is still used by a task of cluster "<cluster>" and cannot be disassociated'`
5. `AssertionError: refusal message 'Specifying both a launch type and capacity provider strategy is not supported. Remove one and try again.'` (expected text not produced)
6. `AssertionError: expected the withdrawn platformVersion 1.3.0 RunTask to be rejected with AccessDeniedException, got 'PlatformUnknownException'`
7. `AssertionError: expected message 'The minimum or maximum scaling step size value is invalid. ...', got 'The specified Auto Scaling group ARN is already being used by another capacity provider. ...'`; `AssertionError: unexpected message for a bogus hookId: "The specified hookId ecs-pause-not-a-real-hook-id isn't valid for service deployment arn:aws:ecs:...:service-deployment/<cluster>/<svc>/<id>. Review the available parameters for the API request."`
ARNs/names quoted are per-run values filling the template.

## Steps to Reproduce
1. CreateService with loadBalancers naming a target-group ARN that does not exist.
2. UpdateService (and RunTask) supplying networkConfiguration for a bridge/host networkMode task definition.
3. ListServicesByNamespace with a namespace name that does not exist.
4. Associate a capacity provider, run a task on it, then try to disassociate it.
5. RunTask with both launchType=FARGATE and a capacityProviderStrategy.
6. RunTask with platformVersion='1.3.0'.
7. CreateCapacityProvider with managedScaling minimumScalingStepSize > maximumScalingStepSize; and ContinueServiceDeployment with a hookId that does not exist.
