Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr__ecs__elbv2.ecs-service-create-fails-with-target-group-type-mismatch
Issue-Key: ecr__ecs__elbv2.ecs-service-create-fails-with-target-group-type-mismatch|_error#6|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Commit: 8bb3e31d2bc3a99611966740f58be573e66087fa

# ECS CreateService accepts a target group whose TargetType does not match the task definition's network mode

> _Note: this issue was identified against the code base at commit `8bb3e31d2bc3a99611966740f58be573e66087fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateService

## Description
One cause (Cause 1 of 1).

Cause 1 of 1: CreateService binds an incompatible ELBv2 target group instead of refusing the request.

AWS: a service whose task definition uses networkMode=awsvpc (Fargate) may only be registered against a target group with TargetType=ip; a target group with TargetType=instance is refused synchronously with InvalidParameterException (400) and no service is created. Recorded behavior only for the exact message; the status (400) and the rejection are the requirement.

Emulator: the CreateService call SUCCEEDS and returns a service with status ACTIVE whose loadBalancers[] names the mismatched target group — e.g. {'serviceName': 'sim2real-svc-mismatch-...', 'status': 'ACTIVE', 'launchType': 'FARGATE', 'loadBalancers': [{'targetGroupArn': 'arn:aws:elasticloadbalancing:...:targetgroup/sim2real-tg-.../...', 'containerName': 'app', 'containerPort': 80}]}.

Why: the client gets a service AWS would never have created. Tasks can never register (an awsvpc task has no instance id to register), so the failure appears later as permanently-unhealthy targets instead of a synchronous, actionable 400 at create time.

Fix: target-group/network-mode compatibility validation in the ECS CreateService handler (resolve the referenced target group's TargetType before accepting loadBalancers).

Related open issue that records the same family but does not carry this replay key: 20261023T060000Z-ecs-elbv2-createservice-accepts-an-instance-type-target-group-for-an-awsvpc-task-definition.md.

Traces: ecr__ecs__elbv2.ecs-service-create-fails-with-target-group-type-mismatch.

## Evidence
Expected: CreateService(..., loadBalancers=[{targetGroupArn: <TargetType=instance group>, containerName, containerPort}]) with an awsvpc/FARGATE task definition -> HTTP 400 InvalidParameterException, no service created.
Actual: HTTP 200 — "expected the call to be rejected, but it SUCCEEDED and returned {'service': {... 'status': 'ACTIVE', 'launchType': 'FARGATE', 'loadBalancers': [{'targetGroupArn': 'arn:aws:elasticloadbalancing:us-west-2:...:targetgroup/sim2real-tg-.../...', 'containerName': 'app', 'containerPort': 80}] ...}}".

## Steps to Reproduce
1. Create an ELBv2 target group with TargetType='instance'.
2. Register a task definition with networkMode='awsvpc' and requiresCompatibilities=['FARGATE'].
3. ecs CreateService(cluster=..., taskDefinition=<that td>, launchType='FARGATE', loadBalancers=[{'targetGroupArn': <instance group>, 'containerName': 'app', 'containerPort': 80}]).
4. Observe HTTP 200 and an ACTIVE service instead of 400 InvalidParameterException.
