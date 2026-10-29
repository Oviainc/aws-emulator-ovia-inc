Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ecs__elbv2.cfn-security-group-blocks-alb-to-ecs-traffic-negative
Issue-Key: cloudformation__ecs__elbv2|CreateService|ClientException
Issue-Commit: e8891ffb15bbac5be85198cac9e3766d8ad3529e

# cloudformation__ecs__elbv2: a CloudFormation-created AWS::ECS::TaskDefinition is unknown to ECS — CreateService against the stack's task-definition ARN answers ClientException "TaskDefinition not found."

> _Note: this issue was identified against the code base at commit `e8891ffb15bbac5be85198cac9e3766d8ad3529e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateService

## Description
ONE root cause.

Cause 1 of 1 — the physical id CloudFormation hands back for an AWS::ECS::TaskDefinition resource does not name a task definition that exists in ECS, so an ECS call that uses it is refused.
AWS: recorded behavior only for the CloudFormation binding (the ECS Smithy model has no CFN coupling). In AWS, creating a stack containing an AWS::ECS::TaskDefinition registers that revision in ECS; {"Ref": "<TaskDef logical id"} resolves to the task definition's ARN (arn:aws:ecs:<region>:<account>:task-definition/<family>:<revision>), that ARN is returned as the stack Output value, ecs DescribeTaskDefinition(taskDefinition=<that ARN>) reports status ACTIVE, and ecs CreateService(taskDefinition=<that ARN>, launchType="FARGATE", networkConfiguration.awsvpcConfiguration, loadBalancers=[...]) succeeds with 200 and the service's own task definition. Smithy: CreateService models taskDefinition as a plain String and classifies a task definition that cannot be resolved under ClientException (400); a resolvable one must NOT be refused.
Emulator: ecs CreateService with the stack's BlockedTaskDefinitionArn output is refused — HTTP 400 ClientException, message "TaskDefinition not found." — although the CloudFormation stack reported the resource created and handed that value back as an Output. The check dies on that call, so the whole SG-blocks-ALB-to-ECS scenario (its blocked service, its steady-state waits and its positive control) is never exercised.
Why: every CloudFormation-first ECS workflow is broken past the stack: an operator who deploys a task definition with CFN and then references it from the SDK/CLI (CreateService, RunTask, DescribeTaskDefinition) gets a not-found refusal for a resource the stack says exists, with no way to tell a genuinely missing revision from a stack-registered one.
Fix: the CloudFormation resource provider for AWS::ECS::TaskDefinition — it must register the revision in the ECS service's task-definition state and set the resource's physical id / Ref value to that registered revision's ARN — and/or the ECS task-definition lookup used by CreateService, so a full task-definition ARN resolves to the stored revision.
Related but not covering: open issues 20261023T205437Z-cloudformation-reports-create-complete-for-resources-it-never.md and 20261022T234922Z-cloudwatch-ecr-ecs-a-cloudformation-declaring-aws-ecr-repository-aws... record CREATE_COMPLETE-without-provisioning for other resource types/services; this check is not marked owned by either and fails on the ECS task-definition binding specifically.

## Evidence
Expected: ecs CreateService(cluster=<stack cluster>, taskDefinition=<stack output BlockedTaskDefinitionArn>, desiredCount=1, launchType="FARGATE", networkConfiguration={awsvpcConfiguration:{subnets, securityGroups}}, loadBalancers=[{targetGroupArn, containerName, containerPort}]) -> 200 with the created service.
Actual: botocore.errorfactory.ClientException: An error occurred (ClientException) when calling the CreateService operation: TaskDefinition not found.
The ARN passed is the stack's own Output, declared as {"Value": {"Ref": "BlockedTaskDef"}} over a resource of Type "AWS::ECS::TaskDefinition" that the stack reported created.

## Steps to Reproduce
1. cloudformation CreateStack with a template containing AWS::ECS::Cluster, two AWS::ECS::TaskDefinition resources (RequiresCompatibilities [FARGATE], NetworkMode awsvpc, Cpu 256, Memory 512, ExecutionRoleArn from an AWS::IAM::Role), an ALB with listener/rules and target groups, and Outputs including {"BlockedTaskDefinitionArn": {"Value": {"Ref": "BlockedTaskDef"}}}; wait for CREATE_COMPLETE.
2. Read the BlockedTaskDefinitionArn output from DescribeStacks.
3. ecs CreateService(cluster=<ClusterName output>, serviceName=<new>, taskDefinition=<that ARN>, desiredCount=1, launchType="FARGATE", networkConfiguration={awsvpcConfiguration:{subnets:<SubnetIds output>, securityGroups:[<BlockedSecurityGroupId output>], assignPublicIp:"ENABLED"}}, loadBalancers=[{targetGroupArn:<BlockedTargetGroupArn output>, containerName:"app", containerPort:80}]).
4. Observe 400 ClientException "TaskDefinition not found." instead of a created service. ecs DescribeTaskDefinition(taskDefinition=<that ARN>) likewise does not resolve the revision.
