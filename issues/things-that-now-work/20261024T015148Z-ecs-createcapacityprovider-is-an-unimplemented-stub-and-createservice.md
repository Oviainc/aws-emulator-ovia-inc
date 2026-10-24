Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2__ecs.ecsnrt-capacity-provider-name-and-group-reuse-rejected autoscaling__ec2__ecs.ecsnrt-empty-provider-run-task-rejected-unless-managed-scaling-parks-it ec2__ecs.ecsnrt-awsvpc-service-and-task-without-network-configuration-rejected ec2__ecs.ecsnt-exec-command-service-without-a-task-role-rejected ec2__ecs__elbv2.ecsnt-code-deploy-service-without-a-load-balancer-rejected ec2__ecs__elbv2.ecsnt-service-load-balancer-binding-outside-the-task-definition-rejected ec2__ecs__elbv2.ecsnt-service-on-a-target-group-without-a-load-balancer-rejected
Issue-Key: autoscaling__ec2__ecs|CreateCapacityProvider|NotImplemented
Issue-Key: ec2__ecs.ecsnrt-awsvpc-service-and-task-without-network-configuration-rejected|_error#8|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error")
Issue-Key: ec2__ecs.ecsnt-exec-command-service-without-a-task-role-rejected|_error#8|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: ec2__ecs__elbv2.ecsnt-code-deploy-service-without-a-load-balancer-rejected|_error#5|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: ec2__ecs__elbv2.ecsnt-service-load-balancer-binding-outside-the-task-definition-rejected|_error#8|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: ec2__ecs__elbv2.ecsnt-service-on-a-target-group-without-a-load-balancer-rejected|_error#10|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Commit: 1f316ce023d643c000ad0b8fb7b5799232d706fd

# ECS: CreateCapacityProvider is an unimplemented stub, and CreateService/RunTask accept five configurations AWS refuses with InvalidParameterException (6 causes)

> _Note: this issue was identified against the code base at commit `1f316ce023d643c000ad0b8fb7b5799232d706fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateService

## Description
Six independent ECS control-plane defects. All six must be closed before submitting. Causes 2-6 are all CreateService/RunTask admitting a service definition real ECS refuses, which means the emulator creates services that cannot exist in AWS; cause 1 is an operation that does not answer at all.

Cause 1 of 6: CreateCapacityProvider is a scaffolded stub.
AWS: creates a capacity provider bound to an Auto Scaling group and returns capacityProvider. Smithy: CreateCapacityProvider input {name (required), cluster, autoScalingGroupProvider, managedInstancesProvider, tags}, output capacityProvider, errors include ClientException/InvalidParameterException/LimitExceededException/UpdateInProgressException (all HTTP 400).
Emulator: NotImplemented — 'operation CreateCapacityProvider is scaffolded but not implemented in this emulator'.
Why: no ECS capacity-provider workflow can start, so the name-reuse rejection and the managed-scaling parking behaviour the two checks assert are unreachable.
Fix: implement the CreateCapacityProvider handler (ECS ops) and the capacity-provider state it writes.
Traces: autoscaling__ec2__ecs.ecsnrt-capacity-provider-name-and-group-reuse-rejected, autoscaling__ec2__ecs.ecsnrt-empty-provider-run-task-rejected-unless-managed-scaling-parks-it.

Cause 2 of 6: CreateService and RunTask accept an awsvpc task definition with no networkConfiguration.
AWS: both are refused with InvalidParameterException (HTTP 400), message 'Network Configuration must be provided when networkMode \'awsvpc\' is specified.', and nothing is created. Smithy: InvalidParameterException is modelled on both CreateService and RunTask; networkConfiguration is optional in the model, so this is a business rule the model cannot express.
Emulator: the call succeeds and the service/task exists.
Why: a client gets an awsvpc service with no ENI configuration, which in AWS can never be launched; the misconfiguration is discovered nowhere.
Fix: networkMode-vs-networkConfiguration validation in the CreateService and RunTask handlers.
Traces: ec2__ecs.ecsnrt-awsvpc-service-and-task-without-network-configuration-rejected.

Cause 3 of 6: CreateService accepts enableExecuteCommand=true on a task definition with no taskRoleArn.
AWS: refused with InvalidParameterException (HTTP 400) naming the missing valid taskRoleArn, and nothing is created.
Emulator: the call succeeds.
Why: ECS Exec cannot work without a task role; the emulator reports a service as exec-enabled that could never accept a session.
Fix: taskRoleArn validation on the enableExecuteCommand path of the CreateService handler.
Traces: ec2__ecs.ecsnt-exec-command-service-without-a-task-role-rejected.

Cause 4 of 6: CreateService accepts deploymentController CODE_DEPLOY with no load balancer.
AWS: refused with InvalidParameterException (HTTP 400) — 'When specifying the CODE_DEPLOY deployment controller type, the service requires ...' — and nothing is created; the identical request under the ECS controller is accepted.
Emulator: the call succeeds.
Why: a blue/green service is created that CodeDeploy can never drive.
Fix: deploymentController/loadBalancers cross-validation in the CreateService handler.
Traces: ec2__ecs__elbv2.ecsnt-code-deploy-service-without-a-load-balancer-rejected.

Cause 5 of 6: CreateService accepts a loadBalancers binding naming a container or port the task definition does not declare.
AWS: refused with InvalidParameterException (HTTP 400) naming the unknown container/port, and nothing is created.
Emulator: the call succeeds.
Why: the service is registered against a target that can never receive traffic, and the typo is never reported.
Fix: validate loadBalancers[].containerName/containerPort against the referenced task definition in the CreateService handler.
Traces: ec2__ecs__elbv2.ecsnt-service-load-balancer-binding-outside-the-task-definition-rejected.

Cause 6 of 6: CreateService accepts a target group that no load balancer forwards to.
AWS: refused with InvalidParameterException (HTTP 400) — 'The target group with targetGroupArn <arn> does not have an associated load balancer.' — and nothing is created; once a listener forwards to that group the identical request is accepted.
Emulator: the call succeeds.
Why: the service appears healthy-by-configuration while no listener can ever reach its targets.
Fix: resolve the target group's load-balancer association (elbv2) during CreateService validation.
Traces: ec2__ecs__elbv2.ecsnt-service-on-a-target-group-without-a-load-balancer-rejected.

## Evidence
Cause 1 — botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateCapacityProvider operation: operation CreateCapacityProvider is scaffolded but not implemented in this emulator (both checks).
Cause 2 — AssertionError: the call succeeded; real AWS rejects it, so there is no error (expected ('InvalidParameterException', "Network Configuration must be provided when networkMode 'awsvpc' is specified.") from both CreateService and RunTask).
Cause 3 — AssertionError: the call succeeded; real AWS rejects it, so there is no error code (expected InvalidParameterException, 'The service couldn't be created because a valid taskRoleArn is not being used. ...').
Cause 4 — AssertionError: the call succeeded; real AWS rejects it, so there is no error code (assert code == "InvalidParameterException" for controller=CODE_DEPLOY with no LB).
Cause 5 — AssertionError: the call succeeded; real AWS rejects it, so there is no error code (assert code == "InvalidParameterException" for an unknown container/port binding).
Cause 6 — AssertionError: the call succeeded; real AWS rejects it, so there is no error code (assert code == "InvalidParameterException" for an orphan target group).

## Steps to Reproduce
Replay the check ids in trace_ids. Minimal repros against a cluster and a registered task definition: (1) ecs:CreateCapacityProvider name=<n> autoScalingGroupProvider={'autoScalingGroupArn':<asg arn>}. (2) register a task definition with networkMode='awsvpc', then ecs:CreateService ... launchType='EC2' desiredCount=0 with no networkConfiguration, and ecs:RunTask on the same definition with no networkConfiguration. (3) register a task definition with no taskRoleArn, then ecs:CreateService enableExecuteCommand=True. (4) ecs:CreateService deploymentController={'type':'CODE_DEPLOY'} with loadBalancers=[]. (5) ecs:CreateService loadBalancers=[{'targetGroupArn':<tg>,'containerName':'nope','containerPort':9999}]. (6) create an elbv2 target group with no listener forwarding to it and ecs:CreateService bound to it.
