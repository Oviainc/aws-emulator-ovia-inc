Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ecr__ecs__logs.deploy-ecs-fargate-stack-with-ecr-and-log-group
Issue-Key: cloudwatch__ecr__ecs__logs.deploy-ecs-fargate-stack-with-ecr-and-log-group|_running_count#21|Timeout|return -1
Issue-Commit: 563433aa8ea88fa1a8e8ad3e10fc27d869dea4a3

# cloudwatch__ecr__ecs__logs: a CFN-provisioned ECS Fargate service never reaches runningCount>=1 and its container's stdout never reaches CloudWatch Logs, so the check exhausts its budget ungraded

> _Note: this issue was identified against the code base at commit `563433aa8ea88fa1a8e8ad3e10fc27d869dea4a3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeServices

## Description
One net-new root cause, grounded in the failing behavioral check listed under the suite heading `cloudwatch__ecr__ecs__logs`.

Cause 1 of 1: a CloudFormation-provisioned ECS Fargate service never becomes steady — DescribeServices keeps reporting runningCount 0 for a service whose desiredCount is 1, and no awslogs stream/event is ever produced for the container — so a client that waits for the service to stabilize never observes the transition and the check never reaches a graded assertion.

AWS: Smithy (com.amazonaws.ecs): `Service.desiredCount` is "The desired number of instantiations of the task definition to keep running on the service"; `Service.runningCount` is "The number of tasks in the cluster that are in the RUNNING state"; `Service.pendingCount` is "The number of tasks in the cluster that are in the PENDING state"; `Service.deployments` is "The current state of deployments for the service" and `Deployment.rolloutState` (enum DeploymentRolloutState) is documented as "When a service deployment is started, it begins in an IN_PROGRESS state. When the service reaches a steady state, the deployment transitions to a COMPLETED state." So for a REPLICA/FARGATE service created with desiredCount=1 against a task definition whose image exists in ECR, AWS schedules a task, moves it PENDING -> RUNNING, and DescribeServices converges to runningCount == desiredCount (pendingCount 0) with the PRIMARY deployment's rolloutState COMPLETED, within a bounded time. Separately, a container whose logConfiguration.logDriver is `awslogs` creates the stream `<awslogs-stream-prefix>/<container>/<task-id>` in the configured log group, so DescribeLogStreams lists that stream and GetLogEvents returns the container's stdout lines (recorded behavior only for the exact stream naming/ingestion latency). Both of those are 200 responses — there is no error code in play here; the divergence is a state machine that never advances.

Emulator: after the stack reports CREATE_COMPLETE and ECR DescribeRepositories resolves the stack's repository, DescribeServices(cluster=<stack cluster>, services=[<stack>-svc]) keeps returning a service whose runningCount stays 0, and DescribeLogStreams/GetLogEvents on the stack's log group never yield the container's marker line. The check polls runningCount>=1 (its own budget 300s, interval 10s) and then the log marker (180s, interval 10s) and is killed by the harness with "Error: Check timed out after 180.0s" without ever reaching a graded assertion. Note the failure mode for this check has MOVED: the open issue 20261104T060239Z-cloudwatch-ecr-ecs-logs-tasks-never-settle-and.md records this same check id failing earlier, at ECR DescribeRepositories with RepositoryNotFoundException (its Cause 3), and that call now succeeds; the service/task/awslogs convergence behind it is what is still missing and that issue's replay keys do not cover this check, so it is filed here. It is adjacent to that issue's Cause 1 (tasks never reach a terminal state and deliver no awslogs events) but is observed through the SERVICE scheduler path (DescribeServices.runningCount for a service the stack created) rather than RunTask/DescribeTasks.

Why: a client cannot wait for a Fargate service to stabilize after an IaC deploy — every `aws ecs wait services-stable`, deployment-gate, autoscaling or alarm workflow built on runningCount converging to desiredCount hangs forever, and nothing downstream that reads the container's stdout from CloudWatch Logs (log-driven health checks, rollout verification, diagnosis) can ever succeed. Because the state never advances, a broken deployment is indistinguishable from one still in progress.

Fix: the ECS service scheduler that must place tasks for a service with desiredCount>0 and advance runningCount/pendingCount and the PRIMARY Deployment's rolloutState (the DescribeServices/CreateService handlers and the task state they own in services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src), together with the awslogs delivery that writes the container's stdout into the cloudwatchlogs service's state (stream `<prefix>/<container>/<task-id>` in the configured group).

Traces: cloudwatch__ecr__ecs__logs.deploy-ecs-fargate-stack-with-ecr-and-log-group.

## Evidence
Failing check: [mismatch] cloudwatch__ecr__ecs__logs.deploy-ecs-fargate-stack-with-ecr-and-log-group
Reported detail: "Error: Check timed out after 180.0s" — the check was killed while polling, so no assertion was graded.

Expected (AWS): after the stack reaches CREATE_COMPLETE, DescribeServices(cluster=<ClusterName output>, services=["<stack-name>-svc"]) returns one service whose runningCount reaches >= 1 (== desiredCount, pendingCount 0, PRIMARY deployment rolloutState COMPLETED) within the poll budget; then DescribeLogStreams(logGroupName=<LogGroupName output>, orderBy=LastEventTime, descending=true) lists the container's awslogs stream and GetLogEvents on it returns an event whose message contains the container's stdout marker line.

Actual (emulator): runningCount never leaves 0 for the service the stack created (the service IS returned — `services` is non-empty, so the check does not hit the -1 "service missing" path), and no log stream carrying the container's stdout ever appears in the stack's log group. Both polls spin at 10s intervals until the harness budget ends.

The variable parts of the above are all request/state values — cluster name, service name `<stack-name>-svc`, log group name and the generated marker string come from the stack outputs and the request; the requirement is the convergence of runningCount to desiredCount and the ingestion of the container's stdout, not any particular generated name, task id, stream name suffix or timestamp.

## Steps to Reproduce
1. Deploy a CloudFormation stack declaring an AWS::ECR::Repository, an AWS::Logs::LogGroup, an AWS::ECS::Cluster, a FARGATE AWS::ECS::TaskDefinition whose container runs `sh -c 'echo <marker>; sleep ...'` with logConfiguration.logDriver=awslogs pointing at that log group, and an AWS::ECS::Service with desiredCount 1 and awsvpc networkConfiguration (SubnetId/SecurityGroupId parameters). Wait for CREATE_COMPLETE.
2. Read the ClusterName, LogGroupName, EcrRepositoryName and EcrRepositoryUri stack outputs; confirm ecr DescribeRepositories(repositoryNames=[<EcrRepositoryName>]) returns a repositoryUri equal to the EcrRepositoryUri output (this step now passes).
3. Poll ecs DescribeServices(cluster=<ClusterName>, services=["<stack-name>-svc"]) every 10s and read services[0].runningCount — it stays 0 indefinitely instead of reaching 1.
4. Poll logs DescribeLogStreams(logGroupName=<LogGroupName>, orderBy="LastEventTime", descending=True, limit=5) and GetLogEvents on each returned stream — the container's stdout marker never appears.
