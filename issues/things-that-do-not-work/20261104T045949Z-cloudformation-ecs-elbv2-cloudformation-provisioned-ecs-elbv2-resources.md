Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ecs__elbv2.cfn-alb-listener-rule-routes-to-ecs-service-positive cloudformation__ecs__elbv2.cfn-alb-target-group-port-mismatch-negative cloudformation__ecs__elbv2.cfn-ecs-service-alb-target-group-registration-positive cloudformation__ecs__elbv2.cfn-ecs-service-awsvpc-ip-target-type-positive cloudformation__ecs__elbv2.cfn-ecs-task-role-missing-trust-policy-negative cloudformation__ecs__elbv2.cfn-ecs-task-role-trusts-ecs-tasks-service-positive cloudformation__ecs__elbv2.cli-cloudformation-create-stack-then-ecs-service-list-tasks cloudformation__ecs__elbv2.cli-ecs-create-service-with-alb-attach-and-verify-targets cloudformation__ecs__elbv2.cli-ecs-service-missing-iam-pass-role-permission-negative cloudformation__ecs__elbv2.cli-ecs-update-service-target-group-swap-positive cloudformation__ecs__elbv2.cli-elbv2-create-target-group-wrong-vpc-negative cloudformation__ecs__elbv2.cli-elbv2-register-unregister-targets-manual-workflow cloudformation__ecs__elbv2.ecs-service-alb-health-check-grace-period-prevents-premature-deregistration-positive cloudformation__ecs__elbv2.ecs-service-registers-tasks-as-alb-targets-positive cloudformation__ecs__elbv2.ecs-service-scale-down-deregisters-alb-targets-positive cloudformation__ecs__elbv2.ecs-service-scale-up-increases-alb-targets-positive cloudformation__ecs__elbv2.ecs-service-target-group-arn-attribute-not-set-negative cloudformation__ecs__elbv2.ecs-task-network-mode-awsvpc-ip-target-registration-positive cloudformation__ecs__elbv2.ecs-task-stopped-event-triggers-target-deregistration-event cloudformation__ecs__elbv2.elbv2-deregisters-targets-on-ecs-task-stop-positive cloudformation__ecs__elbv2.elbv2-listener-rule-path-based-routing-to-multiple-ecs-services-positive cloudformation__ecs__elbv2.elbv2-security-group-mismatch-blocks-health-checks-negative cloudformation__ecs__elbv2.elbv2-target-health-check-path-affects-ecs-target-status-positive
Issue-Key: cloudformation__ecs__elbv2.cfn-alb-listener-rule-routes-to-ecs-service-positive|probe#10|IndexError|return ecs.describe_services(cluster=cluster, services=[service])["services"][0]
Issue-Key: cloudformation__ecs__elbv2|UpdateService|ServiceNotFoundException
Issue-Key: cloudformation__ecs__elbv2.cfn-ecs-service-alb-target-group-registration-positive|probe#9|IndexError|return ecs.describe_services(cluster=cluster, services=[service])["services"][0]
Issue-Key: elastic-load-balancing-v2|DescribeTargetGroups|TargetGroupNotFound
Issue-Key: cloudformation__ecs__elbv2.cfn-ecs-task-role-missing-trust-policy-negative|run#11|AssertionError|assert "unable to assume" in msg.lower() or "assumerole" in msg.lower() or "access" in msg.lower(), (
Issue-Key: ecs|RunTask|ClientException
Issue-Key: cloudformation__ecs__elbv2.cli-cloudformation-create-stack-then-ecs-service-list-tasks|_run_shell#20|AssertionError|raise AssertionError(
Issue-Key: cloudformation__ecs__elbv2.cli-ecs-create-service-with-alb-attach-and-verify-targets|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Key: cloudformation__ecs__elbv2.cli-ecs-service-missing-iam-pass-role-permission-negative|_error_code#24|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded and returned {result!r}")
Issue-Key: cloudformation__ecs__elbv2.cli-ecs-update-service-target-group-swap-positive|_poll_until#17|Timeout|time.sleep(interval_s)
Issue-Key: cloudformation__ecs__elbv2.cli-elbv2-create-target-group-wrong-vpc-negative|run#30|Timeout|states = [s for _, s in _target_states(ctx, tg_wrong_arn)]
Issue-Key: cloudformation__ecs__elbv2.cli-elbv2-register-unregister-targets-manual-workflow|_poll_until#14|Timeout|time.sleep(interval_s)
Issue-Key: cloudformation__ecs__elbv2.ecs-service-alb-health-check-grace-period-prevents-premature-deregistration-positive|run#16|AssertionError|assert observed_unhealthy, "target reported healthy before the container could plausibly have started responding"
Issue-Key: cloudformation__ecs__elbv2.ecs-service-registers-tasks-as-alb-targets-positive|_task_private_ip#12|AssertionError|raise AssertionError(f"no ENI private IP found for task {task_arn}")
Issue-Key: cloudformation__ecs__elbv2.ecs-service-scale-down-deregisters-alb-targets-positive|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: cloudformation__ecs__elbv2.ecs-service-scale-up-increases-alb-targets-positive|_poll_until#10|Timeout|last = probe()
Issue-Key: cloudformation__ecs__elbv2.ecs-service-target-group-arn-attribute-not-set-negative|_poll_until#18|Timeout|last = probe()
Issue-Key: cloudformation__ecs__elbv2.ecs-task-network-mode-awsvpc-ip-target-registration-positive|_task_private_ip#12|AssertionError|raise AssertionError(f"no ENI private IP found for task {task_arn}")
Issue-Key: cloudformation__ecs__elbv2.ecs-task-stopped-event-triggers-target-deregistration-event|_task_private_ip#13|AssertionError|raise AssertionError(f"no ENI private IP found for task {task_arn}")
Issue-Key: cloudformation__ecs__elbv2.elbv2-deregisters-targets-on-ecs-task-stop-positive|_task_private_ip#13|AssertionError|raise AssertionError(f"no ENI private IP found for task {task_arn}")
Issue-Key: cloudformation__ecs__elbv2.elbv2-listener-rule-path-based-routing-to-multiple-ecs-services-positive|_poll_until#18|Timeout|last = probe()
Issue-Key: cloudformation__ecs__elbv2.elbv2-security-group-mismatch-blocks-health-checks-negative|_poll_until#11|Timeout|last = probe()
Issue-Key: cloudformation__ecs__elbv2.elbv2-target-health-check-path-affects-ecs-target-status-positive|_single_target#10|AssertionError|assert len(descs) == 1, f"expected exactly one registered target, got {len(descs)}: {descs!r}"
Issue-Commit: f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7

# cloudformation__ecs__elbv2: CloudFormation-provisioned ECS/ELBv2 resources are absent from their owning services and the ECS/ALB data plane never registers a target (8 causes)

> _Note: this issue was identified against the code base at commit `f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeServices

## Description
EIGHT net-new root causes, all in the cloudformation__ecs__elbv2 surface. Each must be closed before submitting.

Related but NOT covering (none of the 23 check ids below is marked owned by them): open issues 20261029T175151Z-cloudformation-ecs-elbv2-a-cloudformation-created-aws-ecs.md (carries only cfn-security-group-blocks-alb-to-ecs-traffic-negative), 20261023T205437Z-cloudformation-reports-create-complete-for-resources-it-never.md and 20261022T234922Z-cloudwatch-ecr-ecs-a-cloudformation-stack-declaring-aws-ecr-repository-aws.md.

Cause 1 of 8 — a CloudFormation-created AWS::ECS::Service does not exist in ECS.
AWS: recorded behavior only for the CloudFormation binding (the ECS Smithy model carries no CFN coupling). In AWS, a stack containing AWS::ECS::Cluster + AWS::ECS::TaskDefinition + AWS::ECS::Service that reports CREATE_COMPLETE has registered that service in ECS: DescribeServices(cluster=<stack cluster>, services=[<stack service>]) returns one entry in `services` (Smithy: DescribeServicesResponse has both `services` and `failures`; an existing service is reported in `services`), and ListTasks/UpdateService against it succeed with 200.
Emulator: for a CREATE_COMPLETE stack, DescribeServices returns an EMPTY `services` list (the check's `["services"][0]` raises IndexError), ListTasks is refused 400 ServiceNotFoundException "Service not found." and UpdateService is refused 400 ServiceNotFoundException "Service not found.". The wording and class of those refusals is AWS's for a genuinely missing service; the defect is that the stack-created service is missing. Fixed text "Service not found." carries no variable part.
Why: every CloudFormation-first ECS workflow breaks past the stack — an operator who deploys a service with CFN cannot scale, list tasks for, or update it, and cannot distinguish a never-provisioned service from a typo.
Fix: the CloudFormation resource provider for AWS::ECS::Service (it must create the service in the ECS crate's state and set the resource's physical id / Ref to it), and/or the ECS service lookup those operations use.
Traces: cloudformation__ecs__elbv2.cfn-alb-listener-rule-routes-to-ecs-service-positive, cloudformation__ecs__elbv2.cfn-ecs-service-alb-target-group-registration-positive, cloudformation__ecs__elbv2.cfn-alb-target-group-port-mismatch-negative, cloudformation__ecs__elbv2.cli-cloudformation-create-stack-then-ecs-service-list-tasks.

Cause 2 of 8 — a CloudFormation-created AWS::ECS::TaskDefinition is unknown to ECS, so RunTask against the stack's own output is refused.
AWS: recorded behavior only for the CFN binding. In AWS, {"Ref": <AWS::ECS::TaskDefinition logical id>} resolves to arn:aws:ecs:<region>:<account>:task-definition/<family>:<revision>, that revision is ACTIVE in ECS, and RunTask(taskDefinition=<that ARN>) is accepted. Smithy: RunTask classifies an unresolvable task definition under ClientException (400); a resolvable one must not be refused.
Emulator: RunTask with the stack's task-definition ARN is refused 400 ClientException "TaskDefinition not found." — fixed text, no variable part. The negative sibling check is corrupted by the same cause: it expects RunTask to fail because ECS cannot assume a badly-trusted taskRoleArn (an "unable to assume"/AccessDenied-class refusal naming the role) and instead receives ClientException "TaskDefinition not found.", i.e. the right refusal for the wrong reason.
Why: a client cannot tell a missing revision from a stack-registered one, and every authorization negative layered on a CFN task definition silently passes for the wrong reason.
Fix: the CloudFormation resource provider for AWS::ECS::TaskDefinition (register the revision in ECS task-definition state, set the physical id to the registered ARN) and/or the task-definition resolution used by RunTask.
Traces: cloudformation__ecs__elbv2.cfn-ecs-task-role-trusts-ecs-tasks-service-positive, cloudformation__ecs__elbv2.cfn-ecs-task-role-missing-trust-policy-negative.

Cause 3 of 8 — a CloudFormation-created AWS::ElasticLoadBalancingV2::TargetGroup is unknown to ELBv2.
AWS: recorded behavior only for the CFN binding. A CREATE_COMPLETE stack's target group is describable: DescribeTargetGroups(TargetGroupArns=[<stack output>]) returns it with 200.
Emulator: DescribeTargetGroups with the stack's target-group ARN is refused 400 TargetGroupNotFound, message "One or more target groups not found" — fixed text, no variable part. That is AWS's refusal for an ARN that does not exist; the defect is that the stack's own target group does not exist.
Why: ALB wiring declared in a template cannot be inspected or used by the SDK/CLI after a successful deploy.
Fix: the CloudFormation resource provider for AWS::ElasticLoadBalancingV2::TargetGroup — it must create the group in the ELBv2 crate's state and return that ARN as the physical id.
Traces: cloudformation__ecs__elbv2.cfn-ecs-service-awsvpc-ip-target-type-positive.

Cause 4 of 8 — an awsvpc-mode ECS task carries no elastic-network-interface attachment, so it has no private IPv4 address.
AWS: Smithy: Task.attachments is documented as "The Elastic Network Adapter that's associated with the task if the task uses the awsvpc network mode"; Attachment.type is e.g. "ElasticNetworkInterface" and Attachment.details "for elastic network interfaces ... includes the network interface ID, the MAC address, the subnet ID, and the private IPv4 address" (detail name privateIPv4Address). A RUNNING awsvpc task therefore always exposes one such attachment, and that IP is what an ip-target-type target group registers.
Emulator: DescribeTasks for a RUNNING awsvpc task returns no ElasticNetworkInterface attachment and no privateIPv4Address detail (the checks fail with "no ENI private IP found for task <task ARN>"); the task ARN is a per-run value, the absence of the attachment is the requirement.
Why: ip-target-type registration, ENI-based security-group reasoning and task-level networking are all unreachable — a client cannot find the address its task serves on.
Fix: ECS RunTask/CreateService task materialization — it must allocate an ENI for awsvpc tasks and populate Task.attachments with type ElasticNetworkInterface and details including networkInterfaceId, subnetId, macAddress and privateIPv4Address.
Traces: cloudformation__ecs__elbv2.ecs-service-registers-tasks-as-alb-targets-positive, cloudformation__ecs__elbv2.ecs-task-network-mode-awsvpc-ip-target-registration-positive, cloudformation__ecs__elbv2.ecs-task-stopped-event-triggers-target-deregistration-event, cloudformation__ecs__elbv2.elbv2-deregisters-targets-on-ecs-task-stop-positive.

Cause 5 of 8 — an ECS service created with loadBalancers registers no target in its target group.
AWS: recorded behavior only (no cross-service coupling is modelled). An ECS service with loadBalancers=[{targetGroupArn, containerName, containerPort}] registers each running task as a target, so DescribeTargetHealth(TargetGroupArn=<that group>) returns exactly one TargetHealthDescription per running task.
Emulator: DescribeTargetHealth returns an empty TargetHealthDescriptions list for a service that was created with that target group and whose tasks are reported running ("expected exactly one registered target, got 0: []").
Why: no ALB-fronted ECS deployment can be verified or exercised; health, draining and routing behaviour are all unobservable.
Fix: the ECS service controller's load-balancer registration path, which must register/deregister task targets in the named ELBv2 target group's state.
Traces: cloudformation__ecs__elbv2.elbv2-target-health-check-path-affects-ecs-target-status-positive.

Cause 6 of 8 — registered target health is reported healthy immediately, with no initial unhealthy/initial state.
AWS: recorded behavior only. A newly registered target starts in TargetHealth.State "initial" and only becomes "healthy" after the configured health-check interval/threshold; the service's healthCheckGracePeriodSeconds exists precisely because a just-started container cannot answer yet.
Emulator: the target is reported healthy from the first DescribeTargetHealth poll — the check never observes a non-healthy state ("target reported healthy before the container could plausibly have started responding").
Why: grace-period and health-check-threshold behaviour cannot be tested; a client that relies on an initial unhealthy window (blue/green cutovers, premature-deregistration guards) sees the opposite of AWS.
Fix: the ELBv2 target-health derivation — model the initial state and the interval/threshold transition rather than synthesizing healthy at registration.
Traces: cloudformation__ecs__elbv2.ecs-service-alb-health-check-grace-period-prevents-premature-deregistration-positive.

Cause 7 of 8 — RegisterTaskDefinition succeeds for a caller with no iam:PassRole on the executionRoleArn.
AWS: Smithy: RegisterTaskDefinition models AccessDeniedException (HTTP 403). Passing executionRoleArn requires iam:PassRole on that role, so a principal whose only policy is {Allow ecs:* on *} is refused 403 AccessDeniedException with the standard IAM authorization message: "User: <caller ARN> is not authorized to perform: iam:PassRole on resource: <execution role ARN> because no identity-based policy allows the iam:PassRole action". Fixed text: "User: ", " is not authorized to perform: iam:PassRole on resource: ", " because no identity-based policy allows the iam:PassRole action"; variable parts: the caller's assumed-role ARN (arn:aws:sts::<account>:assumed-role/<role name>/<session name>) and the execution-role ARN from the request. No task-definition revision is created.
Emulator: the call succeeds (returns 200 with a registered revision) — "expected the call to be refused, but it succeeded and returned None" — so the restricted principal registers a revision AWS would have refused.
Why: PassRole is the main privilege boundary around ECS execution roles; an emulator that ignores it lets a least-privilege test pass against code that would be denied in production.
Fix: ECS RegisterTaskDefinition's authorization step — it must evaluate iam:PassRole for executionRoleArn/taskRoleArn against the caller's identity policies and refuse with AccessDeniedException in AWS's wording.
Traces: cloudformation__ecs__elbv2.cli-ecs-service-missing-iam-pass-role-permission-negative.

Cause 8 of 8 — nine ECS/ELBv2 data-plane scenarios never settle, so they produce no verdict at all.
AWS: recorded behavior only. Each of these checks creates a cluster, a task definition, a target group/listener and a service (via SDK or the CLI) and then polls for a settled outcome — a service reaching steady state, a target appearing/draining in a target group, or a refusal of an invalid create. Against AWS each poll converges well inside its own budget.
Emulator: every one of the nine is killed by the harness with "Check timed out after 60.0s" having never converged; given causes 1, 4, 5 and 6 above, the polled condition (a settled service, a registered or drained target) is one the emulator never produces.
Why: the entire ECS+ALB scale/route/drain surface is unverifiable — a user cannot observe scale-up, scale-down, target-group swap, path-based routing or a security-group-blocked health check at all.
Fix: the ECS service/task stabilization path and the ELBv2 target registration/health derivation named in causes 1, 4, 5 and 6 — the scenario must reach an observable terminal state (settled service, registered/drained targets) instead of hanging.
Traces: cloudformation__ecs__elbv2.cli-ecs-create-service-with-alb-attach-and-verify-targets, cloudformation__ecs__elbv2.cli-ecs-update-service-target-group-swap-positive, cloudformation__ecs__elbv2.cli-elbv2-create-target-group-wrong-vpc-negative, cloudformation__ecs__elbv2.cli-elbv2-register-unregister-targets-manual-workflow, cloudformation__ecs__elbv2.ecs-service-scale-down-deregisters-alb-targets-positive, cloudformation__ecs__elbv2.ecs-service-scale-up-increases-alb-targets-positive, cloudformation__ecs__elbv2.ecs-service-target-group-arn-attribute-not-set-negative, cloudformation__ecs__elbv2.elbv2-listener-rule-path-based-routing-to-multiple-ecs-services-positive, cloudformation__ecs__elbv2.elbv2-security-group-mismatch-blocks-health-checks-negative.

## Evidence
Cause 1: ecs.describe_services(cluster=<stack cluster>, services=[<stack service>]) -> expected one entry in `services`; actual `services` == [] (IndexError on ["services"][0]). aws ecs list-tasks --cluster <c> --service-name <s> -> expected 200 with taskArns; actual rc=254, "An error occurred (ServiceNotFoundException) when calling the ListTasks operation: Service not found.". ecs.update_service(...) -> actual ServiceNotFoundException "Service not found.".
Cause 2: ecs.run_task(taskDefinition=<stack output task-definition ARN>) -> expected 200; actual 400 ClientException "TaskDefinition not found.". Negative sibling: expected a refusal naming the un-assumable task role ("unable to assume"/assumerole/access), actual ClientException "TaskDefinition not found.".
Cause 3: elbv2.describe_target_groups(TargetGroupArns=[<stack output>]) -> expected 200 with the group; actual 400 TargetGroupNotFound "One or more target groups not found".
Cause 4: ecs.describe_tasks(cluster=..., tasks=[<running awsvpc task>]) -> expected an attachment of type ElasticNetworkInterface with a privateIPv4Address detail; actual none ("no ENI private IP found for task arn:aws:ecs:<region>:<account>:task/<cluster>/<task id>").
Cause 5: elbv2.describe_target_health(TargetGroupArn=<group named in the service's loadBalancers>) -> expected exactly 1 TargetHealthDescription; actual 0 ([]).
Cause 6: first poll of describe_target_health after service creation -> expected State "initial"/unhealthy until the health check passes; actual "healthy" immediately.
Cause 7: restricted principal (identity policy {Allow ecs:* on *}, no iam:PassRole) calls ecs.register_task_definition(executionRoleArn=<role>) -> expected 403 AccessDeniedException "User: arn:aws:sts::<account>:assumed-role/<caller role>/<session> is not authorized to perform: iam:PassRole on resource: <execution role ARN> because no identity-based policy allows the iam:PassRole action" and no revision created; actual 200 with a registered revision.
Cause 8: nine checks terminated with "Check timed out after 60.0s" without reaching any assertion.

## Steps to Reproduce
1. cloudformation CreateStack with a template containing AWS::ECS::Cluster, AWS::ECS::TaskDefinition (FARGATE/awsvpc, ExecutionRoleArn from an AWS::IAM::Role), AWS::ElasticLoadBalancingV2::{LoadBalancer,TargetGroup,Listener,ListenerRule} and AWS::ECS::Service with loadBalancers wired to the target group; Outputs exporting the cluster name, service name, task-definition ARN and target-group ARN. Wait for CREATE_COMPLETE.
2. ecs DescribeServices(cluster=<ClusterName>, services=[<ServiceName>]) -> observe an empty `services` list (cause 1); aws ecs list-tasks --cluster <ClusterName> --service-name <ServiceName> and ecs UpdateService -> observe ServiceNotFoundException "Service not found." (cause 1).
3. ecs RunTask(cluster=<ClusterName>, taskDefinition=<TaskDefinitionArn output>) -> observe 400 ClientException "TaskDefinition not found." (cause 2).
4. elbv2 DescribeTargetGroups(TargetGroupArns=[<TargetGroupArn output>]) -> observe 400 TargetGroupNotFound "One or more target groups not found" (cause 3).
5. Without CloudFormation: ecs CreateCluster, RegisterTaskDefinition (FARGATE, awsvpc), elbv2 CreateTargetGroup(TargetType=ip)+CreateLoadBalancer+CreateListener, then ecs CreateService(desiredCount=1, launchType=FARGATE, loadBalancers=[{targetGroupArn, containerName, containerPort}], networkConfiguration.awsvpcConfiguration). ListTasks+DescribeTasks the running task -> no ElasticNetworkInterface attachment / privateIPv4Address (cause 4). elbv2 DescribeTargetHealth(TargetGroupArn) -> 0 descriptions (cause 5), and where a target does appear it is already "healthy" on the first poll (cause 6).
6. iam CreateRole with a trust for the account root and an inline policy {Effect Allow, Action ecs:*, Resource *}; sts AssumeRole into it; with those credentials call ecs RegisterTaskDefinition(executionRoleArn=<some execution role>) -> observe 200 instead of 403 AccessDeniedException for iam:PassRole (cause 7).
7. Run the nine scale/swap/route/drain checks listed under cause 8 and observe that each is killed after 60s without converging.
