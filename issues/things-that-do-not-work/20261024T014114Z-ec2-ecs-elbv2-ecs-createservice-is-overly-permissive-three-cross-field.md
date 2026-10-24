Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__ecs__elbv2.ecsnt-code-deploy-service-without-a-load-balancer-rejected ec2__ecs__elbv2.ecsnt-service-load-balancer-binding-outside-the-task-definition-rejected ec2__ecs__elbv2.ecsnt-service-on-a-target-group-without-a-load-balancer-rejected
Issue-Key: ec2__ecs__elbv2.ecsnt-code-deploy-service-without-a-load-balancer-rejected|_error#5|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: ec2__ecs__elbv2.ecsnt-service-load-balancer-binding-outside-the-task-definition-rejected|_error#8|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: ec2__ecs__elbv2.ecsnt-service-on-a-target-group-without-a-load-balancer-rejected|_error#10|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Commit: 1f316ce023d643c000ad0b8fb7b5799232d706fd

# ECS CreateService is overly permissive: three cross-field load-balancer validations AWS enforces are missing, so invalid services are created with 200

> _Note: this issue was identified against the code base at commit `1f316ce023d643c000ad0b8fb7b5799232d706fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateService

## Description
THREE distinct root causes are bundled here; all three must be closed. In every one the emulator's ECS CreateService returns HTTP 200 and creates a service, where real AWS refuses the request with 400 InvalidParameterException and creates nothing. These are cross-field rules keyed on state stored by an EARLIER call (the registered task definition, the target group's live load-balancer association) or on another input member (the deployment-controller type), which is why a wire-shape check cannot see them.

Related open issues that do NOT cover these causes and do not carry these replay keys: 20261023T205437Z-ecs-createservice-accepts-a-target-group-whose-targettype.md and 20261023T060000Z-ecs-elbv2-createservice-accepts-an-instance-type-target-group-for-an-awsvpc-task-definition.md both record the TargetType-vs-network-mode rule; 20261022T152302Z-ecs-accepts-invalid-requests-with-200-and-misreports.md records unrelated describe/accept defects. The three causes below are separate validations.

Cause 1 of 3 - a CODE_DEPLOY-controlled service with no loadBalancers is accepted.
AWS: Smithy CreateService models InvalidParameterException (errorKind client, httpError 400 defaulted) and takes deploymentController (DeploymentController) and loadBalancers (LoadBalancers list) as independent, non-required members. The cross-field rule is server-side: when deploymentController.type is CODE_DEPLOY the service MUST carry an Application or Network Load Balancer binding, so the omission is refused 400 InvalidParameterException. Recorded message, as an example of the request-derived rule naming the controller type: "When specifying the CODE_DEPLOY deployment controller type, the service requires an Application Load Balancer or Network Load Balancer." The identical request with deploymentController.type=ECS is legal and must be accepted.
Emulator: the CODE_DEPLOY CreateService SUCCEEDS (the check's _error helper reports "the call succeeded; real AWS rejects it, so there is no error code"), so no error code is produced at all and ListServices is left non-empty.
Why: a client wiring a blue/green CodeDeploy service gets a green control-plane answer for a service CodeDeploy can never deploy to; the synchronous InvalidParameterException it would have corrected at create time never arrives, and the failure surfaces much later inside a CodeDeploy deployment.
Fix: cross-field validation in the CreateService handler - gate on deploymentController.type before the service is persisted.
Traces: ec2__ecs__elbv2.ecsnt-code-deploy-service-without-a-load-balancer-rejected.

Cause 2 of 3 - a loadBalancers binding naming a container or port the REGISTERED task definition does not have is accepted.
AWS: the binding's containerName/containerPort are validated against the task definition named by the taskDefinition member - the container must exist in its containerDefinitions and the port must appear in that container's portMappings. Either violation is refused 400 InvalidParameterException. Recorded messages, as examples of the request-derived rule naming the offending container and port: "The container web does not exist in the task definition." and "The container app did not have a container port 8080 defined." The correct binding (container 'app', port 80) must be accepted and round-trip in service.loadBalancers.
Emulator: both malformed bindings SUCCEED ("the call succeeded; real AWS rejects it, so there is no error code"), and the refused creates leave services behind.
Why: the service is created with a binding that can never attach a task to the target group, so targets never register and the ALB has no healthy backends - with no diagnostic anywhere, because the control plane said 200. AWS would have rejected the request and named the exact field.
Fix: validation in the CreateService handler that resolves the stored task definition (ECS state) and checks each loadBalancers entry's containerName/containerPort against its containerDefinitions/portMappings.
Traces: ec2__ecs__elbv2.ecsnt-service-load-balancer-binding-outside-the-task-definition-rejected.

Cause 3 of 3 - a service bound to a target group that no load balancer forwards to is accepted.
AWS: the target group named in loadBalancers[].targetGroupArn must be associated with a load balancer, i.e. elbv2 DescribeTargetGroups must report a non-empty LoadBalancerArns for it. A target group with no listener forwarding to it is refused 400 InvalidParameterException. Recorded message, as an example of the request-derived rule naming the ARN: "The target group with targetGroupArn <arn> does not have an associated load balancer." This resolution is LIVE: after a listener on the ALB is created forwarding to that same group, the identical CreateService must be accepted - so the same ARN is refused and then accepted.
Emulator: the create against the orphan target group SUCCEEDS ("the call succeeded; real AWS rejects it, so there is no error code") and leaves a service behind.
Why: the ordering guarantee clients rely on (create the listener, then the service) is erased; an emulated deployment "works" with a dangling target group where the real one is rejected, so a template or script with its dependencies in the wrong order passes here and fails in AWS.
Fix: validation in the CreateService handler that reads the target group's load-balancer association from elbv2 state at request time, rather than trusting the ARN.
Traces: ec2__ecs__elbv2.ecsnt-service-on-a-target-group-without-a-load-balancer-rejected.

## Evidence
Cause 1: expected 400 InvalidParameterException (message example "When specifying the CODE_DEPLOY deployment controller type, the service requires an Application Load Balancer or Network Load Balancer."); actual - CreateService returned success, check failed with AssertionError "the call succeeded; real AWS rejects it, so there is no error code" at check_ec2__ecs__elbv2_ecsnt_code_deploy_service_without_a_load_balancer_rejected.py:86 (_error).
Cause 2: expected 400 InvalidParameterException twice (message examples "The container web does not exist in the task definition." and "The container app did not have a container port 8080 defined."); actual - CreateService returned success, AssertionError "the call succeeded; real AWS rejects it, so there is no error code" at check_ec2__ecs__elbv2_ecsnt_service_load_balancer_binding_outside_the_task_definition_rejected.py:93 (_error).
Cause 3: expected 400 InvalidParameterException (message example "The target group with targetGroupArn <arn> does not have an associated load balancer."); actual - CreateService returned success, AssertionError "the call succeeded; real AWS rejects it, so there is no error code" at check_ec2__ecs__elbv2_ecsnt_service_on_a_target_group_without_a_load_balancer_rejected.py:95 (_error).
Smithy (services/service-com-amazonaws-ecs-amazonec2containerservicev20141113): CreateService input members serviceName (required), taskDefinition, loadBalancers -> LoadBalancers, deploymentController -> DeploymentController; modeled errors include InvalidParameterException (client, httpError 400 defaulted) and ClientException.

## Steps to Reproduce
1. Create a VPC (10.0.0.0/16) with two subnets, a security group, and an ECS cluster.
2. RegisterTaskDefinition with networkMode=awsvpc and one container named 'app' whose portMappings contain containerPort 80.
3. Cause 1: CreateService(cluster, serviceName, taskDefinition=<the above>, networkConfiguration=<awsvpc config>, deploymentController={'type':'CODE_DEPLOY'}, platformVersion='1.4.0') with NO loadBalancers member -> emulator returns 200 and ListServices(cluster) is non-empty; AWS returns 400 InvalidParameterException and ListServices stays empty. Repeat with deploymentController={'type':'ECS'} -> must be accepted (this twin is the control).
4. Cause 2: create an internal ALB, an ip target group in the VPC, and a listener on port 80 forwarding to it. Then CreateService(..., loadBalancers=[{'targetGroupArn':tg,'containerName':'web','containerPort':80}]) and again with {'containerName':'app','containerPort':8080} -> each returns 200 instead of 400 InvalidParameterException. The correct binding {'containerName':'app','containerPort':80} must still be accepted and round-trip.
5. Cause 3: create an internal ALB and an ip target group but NO listener; confirm DescribeTargetGroups(TargetGroupArns=[tg])['TargetGroups'][0]['LoadBalancerArns'] == []. CreateService(..., loadBalancers=[{'targetGroupArn':tg,'containerName':'app','containerPort':80}]) -> emulator returns 200 instead of 400 InvalidParameterException. Then create the listener forwarding to tg, wait for LoadBalancerArns to report the ALB, and re-issue the identical CreateService -> it must now be accepted.
