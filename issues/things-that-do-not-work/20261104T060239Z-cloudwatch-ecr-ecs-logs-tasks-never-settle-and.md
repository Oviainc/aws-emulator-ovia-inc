Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ecr__ecs__logs.cli-ecs-task-log-delivery-blocked-without-permission-rejected cloudwatch__ecr__ecs__logs.cli-log-group-retention-set-and-verified cloudwatch__ecr__ecs__logs.deploy-ecs-service-rollback-on-missing-ecr-image cloudwatch__ecr__ecs__logs.deploy-stack-fails-without-log-group-resource-policy cloudwatch__ecr__ecs__logs.deploy-cloudwatch-alarm-on-ecs-service-from-template cloudwatch__ecr__ecs__logs.deploy-ecs-fargate-stack-with-ecr-and-log-group
Issue-Key: cloudwatch__ecr__ecs__logs.cli-ecs-task-log-delivery-blocked-without-permission-rejected|_poll_until#24|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__ecr__ecs__logs.cli-log-group-retention-set-and-verified|_poll_until#26|Timeout|return last
Issue-Key: cloudwatch__ecr__ecs__logs.deploy-ecs-service-rollback-on-missing-ecr-image|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__ecr__ecs__logs.deploy-stack-fails-without-log-group-resource-policy|_poll_until#15|Timeout|return last
Issue-Key: cloudwatch__ecr__ecs__logs.deploy-cloudwatch-alarm-on-ecs-service-from-template|run#11|IndexError|svc = ecs.describe_services(cluster=cluster_name, services=[service_name])[
Issue-Key: cloudwatch__ecr__ecs__logs|DescribeRepositories|RepositoryNotFoundException
Issue-Commit: b64c3d45927e2833db57dde0eaeae2a33a5b497f

# cloudwatch__ecr__ecs__logs: tasks never settle and deliver no awslogs events, and CloudFormation-created ECS services and ECR repositories are invisible to their own services (3 causes)

> _Note: this issue was identified against the code base at commit `b64c3d45927e2833db57dde0eaeae2a33a5b497f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTasks

## Description
Three net-new root causes, all grounded in failing behavioral checks under the suite heading `cloudwatch__ecr__ecs__logs`.

Cause 1 of 3: a Fargate task never reaches STOPPED and its container writes no awslogs events.
AWS: Smithy: ecs#Task.lastStatus walks to STOPPED and ecs#Task carries stoppedAt/stopCode/stoppedReason; a container with logConfiguration.logDriver=awslogs creates the stream <awslogs-stream-prefix>/<container>/<task-id> in the configured group and FilterLogEvents returns the container's stdout. A task whose execution role has no logs:CreateLogStream/PutLogEvents stops with stopCode TaskFailedToStart and a ResourceInitializationError stoppedReason and creates no stream (recorded behavior only for the exact reason text).
Emulator: every check that polls DescribeTasks for a terminal task, or FilterLogEvents for the container's marker line, runs out of budget ("Check timed out after 60.0s"): no task ever stops and no log event is ever ingested, so neither the positive delivery nor the permission-denied non-delivery can be told apart from a service that simply does nothing.
Why: a client cannot wait for a task, read its exit code, or read its logs; log-driven diagnosis and rollback logic built on awslogs never fires.
Fix: the ECS task state machine behind RunTask/DescribeTasks and the awslogs delivery it owns (services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src, writing into the cloudwatchlogs service's state).
Traces: cloudwatch__ecr__ecs__logs.cli-log-group-retention-set-and-verified, cloudwatch__ecr__ecs__logs.cli-ecs-task-log-delivery-blocked-without-permission-rejected, cloudwatch__ecr__ecs__logs.deploy-ecs-service-rollback-on-missing-ecr-image, cloudwatch__ecr__ecs__logs.deploy-stack-fails-without-log-group-resource-policy.

Cause 2 of 3: a CloudFormation-created AWS::ECS::Service is absent from ECS DescribeServices.
AWS: after a stack reaches CREATE_COMPLETE, DescribeServices(cluster=<stack cluster>, services=[<stack service>]) returns that service in `services` (and nothing in `failures`).
Emulator: DescribeServices returns an empty `services` list for the service the stack reported as created - the check dies with IndexError indexing services[0].
Why: an alarm/scaling/deployment client that resolves a template-provisioned service by describe cannot find it, so every follow-up control-plane call is made against a nonexistent service.
Fix: the CloudFormation resource provider for AWS::ECS::Service must register the service in the ECS service's state (services/service-com-amazonaws-cloudformation-cloudformation/src). Related but distinct from the open 20261104T045949Z-cloudformation-ecs-elbv2-cloudformation-provisioned-ecs-elbv2-resources.md, which carries other checks.
Traces: cloudwatch__ecr__ecs__logs.deploy-cloudwatch-alarm-on-ecs-service-from-template.

Cause 3 of 3: a CloudFormation-created AWS::ECR::Repository is unknown to ECR.
AWS: after the stack completes, ecr DescribeRepositories(repositoryNames=[<the stack's repository>]) returns the repository; RepositoryNotFoundException (HTTP 400) is reserved for a name that does not exist in the registry.
Emulator: DescribeRepositories is refused with RepositoryNotFoundException and the message "The repository with name '<repositoryName>' does not exist in the registry with id '<registryId>'" for the repository the stack declared - the fixed text is everything but <repositoryName> (from the request) and <registryId> (the caller's account).
Why: the push/pull chain after an IaC deploy is impossible: a client gets a repository URI from stack outputs that ECR denies exists.
Fix: the CloudFormation resource provider for AWS::ECR::Repository must create the repository in the ECR service's state (services/service-com-amazonaws-cloudformation-cloudformation/src).
Traces: cloudwatch__ecr__ecs__logs.deploy-ecs-fargate-stack-with-ecr-and-log-group.

## Evidence
Cause 1: four checks end with "Check timed out after 60.0s" while polling DescribeTasks for lastStatus == "STOPPED" (timeout_s=300) and then FilterLogEvents for the container marker (timeout_s=180); no task ever stops and no event is ingested.
Cause 2: IndexError: list index out of range at `svc = ecs.describe_services(cluster=cluster_name, services=[service_name])[...][0]` after the stack completed.
Cause 3: botocore.errorfactory.RepositoryNotFoundException: An error occurred (RepositoryNotFoundException) when calling the DescribeRepositories operation: The repository with name 'sim2real-xsvc-stk-159bd0acfb4f-ecrrepository-310781a7bda0' does not exist in the registry with id '000000000000'.

## Steps to Reproduce
1. Create a log group with retention 1, register a FARGATE task definition whose container uses logDriver awslogs into that group with command ["sh","-c","echo <marker>; sleep 3"], RunTask, then poll DescribeTasks for STOPPED and FilterLogEvents for <marker> - neither ever happens (Cause 1).
2. Deploy a CloudFormation stack declaring an ECS cluster + task definition + service, then call ecs DescribeServices for the stack's service - `services` is empty (Cause 2).
3. Deploy a CloudFormation stack declaring AWS::ECR::Repository (plus ECS/logs resources), then call ecr DescribeRepositories with the created repository name - RepositoryNotFoundException (Cause 3).
