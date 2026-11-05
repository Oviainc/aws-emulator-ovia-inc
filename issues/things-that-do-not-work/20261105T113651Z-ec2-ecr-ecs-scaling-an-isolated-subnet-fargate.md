Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__ecr__ecs.ecsdeploy-isolated-subnet-task-blocked-from-starting
Issue-Key: ec2__ecr__ecs.ecsdeploy-isolated-subnet-task-blocked-from-starting|_poll#27|Timeout|return last
Issue-Commit: 9fc0802c1fad78cd4c44ae6731253718bd889678

# ec2__ecr__ecs: scaling an isolated-subnet Fargate service never produces any task-state transition, so the check never reaches a verdict

> _Note: this issue was identified against the code base at commit `9fc0802c1fad78cd4c44ae6731253718bd889678`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateService

## Description
Cause 1 of 1: an ECS Fargate estate built in a VPC with one routed (public) subnet and two PRIVATE_ISOLATED subnets never advances any task state, so neither the positive control nor the isolated-placement failure is ever observable; the replay exhausts its whole budget polling ECS.

AWS: the scenario is a sequence of state transitions ECS performs on its own.
  - CreateService with desiredCount=0 settles immediately: Smithy ecs#Deployment.rolloutState — "When a service deployment is started, it begins in an IN_PROGRESS state. When the service reaches a steady state, the deployment transitions to a COMPLETED state. If the service fails to reach a steady state and circuit breaker is turned on, the deployment transitions to a FAILED state" — so DescribeServices().deployments[0].rolloutState reaches COMPLETED (or FAILED) and stops changing.
  - RunTask of a task in the ROUTED subnet with assignPublicIp=ENABLED and a public image reaches Smithy ecs#Task.lastStatus RUNNING (lastStatus: "The last known status for the task", with the documented PROVISIONING/PENDING/RUNNING lifecycle), and ecs#Task.startedAt is set ("the time when the task transitioned from the PENDING state to the RUNNING state").
  - UpdateService raising desiredCount to 1 on the service placed only in the isolated subnets makes the scheduler launch a task that cannot reach its registry, so the task becomes terminal: ListTasks(desiredStatus="STOPPED") reports it, DescribeTasks reports lastStatus=STOPPED with ecs#Task.stopCode=TaskFailedToStart (Smithy ecs#TaskStopCode; "The stop code indicating why a task was stopped") and a ecs#Task.stoppedReason describing the pull failure, NO startedAt member at all, exactly one attachment of type ElasticNetworkInterface whose details carry subnetId (Smithy ecs#Attachment.details: "For elastic network interfaces, this includes the network interface ID, the MAC address, the subnet ID, and the private IPv4 address") naming one of the service's subnets, both containers at lastStatus STOPPED with NO exitCode, and Smithy ecs#Service.runningCount ("The number of tasks in the cluster that are in the RUNNING state") still 0. The concrete stoppedReason text is recorded behavior only.

Emulator: no transition happens at all. Every poll in the check re-reads the same non-terminal state until the harness kills the replay — the only output is `Error: Check timed out after 60.0s`. Because nothing settles, the emulator cannot distinguish a task that AWS would refuse to start (no route to the registry) from a healthy one, and it also never produces the positive control's RUNNING task.

Why: this is the canonical "my task will not start" diagnosis. A client that drives desiredCount and then reads deployments[].rolloutState, ListTasks(desiredStatus=STOPPED), Task.stopCode/stoppedReason/startedAt and Service.runningCount to decide whether a rollout succeeded gets a service that is permanently mid-flight: no success, no failure, no reason. Any tool, waiter, or CI gate built on those reads hangs instead of reporting, and a network misconfiguration (isolated subnet, no NAT/endpoint) silently looks identical to a working estate.

Fix: the ECS task/service state machine in services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src — the scheduler path that advances a service deployment to a terminal rolloutState and the task lifecycle that must advance a launched task to RUNNING (routed placement) or to STOPPED with stopCode=TaskFailedToStart and a pull-failure stoppedReason (unroutable placement), populating the ElasticNetworkInterface attachment details and leaving startedAt/exitCode absent for a task that never ran. The open issue 20261104T062603Z-ec2-ecr-ecs-a-task-in-an-isolated.md describes a related symptom for this scenario, but the suite reports this failure as unowned — its recorded replay key for this check no longer matches, i.e. the replay now dies at a different poll site in the check — so this failure is not covered by it.

Traces: ec2__ecr__ecs.ecsdeploy-isolated-subnet-task-blocked-from-starting.

## Evidence
Failing check ec2__ecr__ecs.ecsdeploy-isolated-subnet-task-blocked-from-starting ends with `Error: Check timed out after 60.0s` (no assertion was ever reached).

Expected (AWS), after UpdateService(desiredCount=1) on the isolated service:
  - DescribeServices -> deployments[0].rolloutState in {COMPLETED, FAILED}; runningCount == 0
  - ListTasks(cluster, serviceName, desiredStatus="STOPPED") -> >= 1 taskArn
  - DescribeTasks -> lastStatus == "STOPPED", stopCode == "TaskFailedToStart", stoppedReason set, 'startedAt' member ABSENT, exactly one attachment with type == "ElasticNetworkInterface" whose details include subnetId == one of the service subnets, containers ["log-router", "service"] each lastStatus "STOPPED" with NO exitCode
  - the routed control task (public subnet, assignPublicIp=ENABLED, public image) -> lastStatus == "RUNNING"

Actual (emulator): none of these reads ever changes value; every poll returns the same non-terminal state until the replay budget is exhausted. No stopCode, no stoppedReason, no STOPPED task, and no RUNNING control task is ever produced.

## Steps to Reproduce
1. Create a VPC (10.96.0.0/16) with two PRIVATE_ISOLATED subnets (own route table, no 0.0.0.0/0 route) and one public subnet routed to an attached internet gateway, plus a security group.
2. Create an ECR repository, a log group, an IAM execution role trusting ecs-tasks.amazonaws.com, and an ECS cluster.
3. RegisterTaskDefinition: FARGATE, networkMode=awsvpc, cpu=2048/memory=4096, two containers (a non-essential FireLens log-router and an essential service container) referencing images in the (empty) ECR repository.
4. CreateService with desiredCount=0, launchType=FARGATE, the two ISOLATED subnets, assignPublicIp=DISABLED, deploymentCircuitBreaker enabled; poll DescribeServices for deployments[0].rolloutState to become COMPLETED or FAILED -> it never does.
5. RunTask a control task in the PUBLIC subnet with assignPublicIp=ENABLED and a public image; poll DescribeTasks -> lastStatus never becomes RUNNING or STOPPED.
6. UpdateService(desiredCount=1) on the isolated service; poll ListTasks(desiredStatus="STOPPED") -> no stopped task ever appears. The replay is killed at the 60s budget.
