Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__ecs.ecsnrt-awsvpc-service-and-task-without-network-configuration-rejected ec2__ecs__elbv2.ecsnt-code-deploy-service-without-a-load-balancer-rejected ec2__ecs__elbv2.ecsnt-service-load-balancer-binding-outside-the-task-definition-rejected ec2__ecs__elbv2.ecsnt-service-on-a-target-group-without-a-load-balancer-rejected autoscaling__ec2__ecs.ecsnrt-capacity-provider-name-and-group-reuse-rejected autoscaling__ec2__ecs.ecsnrt-empty-provider-run-task-rejected-unless-managed-scaling-parks-it ec2__ecs.ecsnt-exec-command-service-without-a-task-role-rejected
Issue-Key: ec2__ecs|ListServices|NotImplemented
Issue-Key: ec2__ecs__elbv2|ListServices|NotImplemented
Issue-Key: autoscaling__ec2__ecs|CreateCapacityProvider|ClientException
Issue-Key: autoscaling__ec2__ecs.ecsnrt-empty-provider-run-task-rejected-unless-managed-scaling-parks-it|run#11|AssertionError|assert code_msg == want, f"RunTask on the unmanaged empty provider said {code_msg!r}, expected {want!r}"
Issue-Key: ec2__ecs.ecsnt-exec-command-service-without-a-task-role-rejected|run#10|AssertionError|assert (code, msg) == want, f"exec-command without a task role: {(code, msg)!r}, expected {want!r}"
Issue-Commit: 3a80585caf3e9f62e646ed534a139af0bb6604b7

# ECS: ListServices is an unimplemented scaffold stub, CreateCapacityProvider ignores its @idempotent contract, and two rejection messages diverge

> _Note: this issue was identified against the code base at commit `3a80585caf3e9f62e646ed534a139af0bb6604b7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListServices

## Description
Four net-new root causes in the ECS emulator, grounded in failing behavioral checks reached through EC2-led scenario groups.

Cause 1 of 4: ListServices answers NotImplemented.
AWS: Smithy (ecs, com-amazonaws-ecs-amazonec2containerservicev20141113): ListServices is a modelled, @readonly, @paginated operation - input {cluster, nextToken, maxResults, launchType, schedulingStrategy, resourceManagementType}, output {serviceArns: StringList, nextToken}, errors AccessDeniedException (403), ClientException (400), ClusterNotFoundException (400), InvalidParameterException (400), ServerException (500). A call against an existing cluster returns 200 with the cluster's service ARNs (an empty list when there are none).
Emulator: every call fails with error code NotImplemented and the message "operation ListServices is scaffolded but not implemented in this emulator".
Why: ListServices is the standard way to assert "the rejected CreateService created nothing" and to enumerate a cluster's services. Any client that enumerates services - and every negative CreateService test that verifies effect-absence - cannot run at all. This blocks four behavioral checks outright, so the CreateService validation rules they exist to grade are ungraded.
Fix: implement the operation handler in services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/list_services.rs over the existing service state, including cluster resolution (ClusterNotFoundException) and the nextToken/maxResults pagination the model declares.
Traces: ec2__ecs.ecsnrt-awsvpc-service-and-task-without-network-configuration-rejected, ec2__ecs__elbv2.ecsnt-code-deploy-service-without-a-load-balancer-rejected, ec2__ecs__elbv2.ecsnt-service-load-balancer-binding-outside-the-task-definition-rejected, ec2__ecs__elbv2.ecsnt-service-on-a-target-group-without-a-load-balancer-rejected.

Cause 2 of 4: CreateCapacityProvider is not idempotent for an identical repeat.
AWS: Smithy (ecs): CreateCapacityProvider carries the @idempotent trait. Repeating the create with the SAME name and the SAME autoScalingGroupProvider group ARN must therefore be a no-op that returns the same capacity provider; only a CONFLICTING request (a taken name over a different group, or a new name over a group another provider already owns, or a reserved "ecs" name prefix) is refused with ClientException (400).
Emulator: the identical repeat is refused with ClientException "A capacity provider with the specified name already exists. To change the configuration of an existing capacity provider, use UpdateCapacityProvider." - the emulator keys the conflict on the name alone and never recognises the idempotent repeat.
Why: idempotency is what makes retries and declarative/converging deployers (re-applying the same capacity-provider definition) safe. Against the emulator every retry after a network timeout, and every second apply, fails instead of succeeding, and the caller cannot tell a genuine name conflict from its own retry.
Fix: the CreateCapacityProvider handler in services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/create_capacity_provider.rs - compare the incoming (name, autoScalingGroupProvider) against the stored provider and return the existing one unchanged when they match, reserving ClientException for real conflicts.
Traces: autoscaling__ec2__ecs.ecsnrt-capacity-provider-name-and-group-reuse-rejected.

Cause 3 of 4: the empty-capacity-provider RunTask refusal does not name the capacity provider.
AWS: recorded behavior only. RunTask through a capacity provider whose Auto Scaling group is empty and has managed scaling DISABLED is refused with InvalidParameterException and a message built from the request, naming the provider: "The specified capacity provider '<name>' contains no container instances."
Emulator: refuses with the generic cluster-level text "No Container Instances were found in your cluster." - the request-derived provider name is absent.
Why: a cluster can carry several capacity providers; AWS's message identifies WHICH one is empty, which is the only way a caller (or an operator reading a deploy log) knows which Auto Scaling group to scale. The emulator's message is unactionable in a multi-provider cluster.
Fix: the RunTask capacity-provider resolution and its error construction in services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/run_task.rs.
Traces: autoscaling__ec2__ecs.ecsnrt-empty-provider-run-task-rejected-unless-managed-scaling-parks-it.

Cause 4 of 4: the CreateService missing-taskRoleArn refusal points the caller at the wrong thing to fix.
AWS: recorded behavior only. CreateService with enableExecuteCommand against a task definition that has no taskRoleArn is refused with InvalidParameterException and the message "The service couldn't be created because a valid taskRoleArn is not being used. Specify a valid task role in your task definition and try again."
Emulator: same code, but the message ends "Specify a valid task IAM role and try again."
Why: the message's remediation clause is the part that tells the caller WHERE the role belongs (in the task definition, not on the service request); the emulator's wording sends them to the wrong resource. The code matches, so this is a message-rule divergence only.
Fix: the CreateService / exec-command validation error text in services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/create_service.rs.
Note: several open issues cover OTHER CreateService permissiveness gaps (e.g. 20261024T014114Z-ec2-ecs-elbv2-ecs-createservice-is-overly-permissive-three-cross-field.md, 20261024T090000Z-ecs-createservice-accepts-a-load-balancer-entry-naming-a-container-port-the-task-definition-does-not-publish.md). Neither carries this check's replay key; they are named here so a fixer touching create_service.rs can land them together.
Traces: ec2__ecs.ecsnt-exec-command-service-without-a-task-role-rejected.

## Evidence
Cause 1 - botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListServices operation: operation ListServices is scaffolded but not implemented in this emulator. Raised identically in all four checks listed. Smithy models ListServices with output {serviceArns, nextToken} and errors AccessDeniedException/ClientException/ClusterNotFoundException/InvalidParameterException/ServerException - none of which is NotImplemented.
Cause 2 - botocore.errorfactory.ClientException: An error occurred (ClientException) when calling the CreateCapacityProvider operation: A capacity provider with the specified name already exists. To change the configuration of an existing capacity provider, use UpdateCapacityProvider. Raised on the IDENTICAL (name, group) repeat, which the Smithy @idempotent trait on CreateCapacityProvider requires to succeed.
Cause 3 - AssertionError: RunTask on the unmanaged empty provider said 'No Container Instances were found in your cluster.', expected "The specified capacity provider 'sim2real-ecsnrt-cp-5b6cdea33200' contains no container instances." (code InvalidParameterException matched).
Cause 4 - AssertionError: exec-command without a task role: ('InvalidParameterException', "The service couldn't be created because a valid taskRoleArn is not being used. Specify a valid task IAM role and try again."), expected ('InvalidParameterException', "The service couldn't be created because a valid taskRoleArn is not being used. Specify a valid task role in your task definition and try again.").
Generated cluster/provider/service names above are from one run and are examples; the requirements are the status, the error code, and the message RULE.

## Steps to Reproduce
1. git_clone(), then run_behavioral_suite().
2. Cause 1: CreateCluster, then ListServices(cluster=<name>) -> expect 200 with serviceArns (empty list), observe NotImplemented.
3. Cause 2: CreateCapacityProvider(name=N, autoScalingGroupProvider={autoScalingGroupArn: A}); repeat the IDENTICAL call -> expect the same capacityProvider returned, observe ClientException "already exists".
4. Cause 3: create a capacity provider over an EMPTY Auto Scaling group with managedScaling DISABLED, then RunTask through it -> compare the InvalidParameterException message, which must name the capacity provider.
5. Cause 4: register a task definition with no taskRoleArn, then CreateService(enableExecuteCommand=true) on it -> compare the InvalidParameterException remediation clause.
