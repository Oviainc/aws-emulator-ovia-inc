Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__ecr__ecs.ecsdeploy-isolated-subnet-task-blocked-from-starting
Issue-Key: ec2__ecr__ecs.ecsdeploy-isolated-subnet-task-blocked-from-starting|_poll#26|Timeout|time.sleep(interval_s)
Issue-Commit: e730f41abf8cc315135e4118582590f4999ad0e7

# ec2__ecr__ecs: no ECS task state ever advances — an isolated-subnet Fargate service produces neither a RUNNING control task nor a STOPPED/TaskFailedToStart task (1 cause)

> _Note: this issue was identified against the code base at commit `e730f41abf8cc315135e4118582590f4999ad0e7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateService

## Description
Cause 1 of 1: an ECS Fargate estate built in a VPC that has one routed (public) subnet and two isolated subnets never advances any task or deployment state, so neither the positive control (a routed task that must RUN) nor the subject failure (an unroutable task that must fail to start) is ever observable. The replay spends its whole budget polling ECS and never reaches an assertion.

AWS: the whole scenario is a sequence of state transitions ECS performs on its own.
  - A task placed in a ROUTED subnet with assignPublicIp=ENABLED pulling a public image advances to Smithy ecs#Task.lastStatus RUNNING ("The last known status for the task", with the documented PROVISIONING/PENDING/RUNNING lifecycle) and gets ecs#Task.startedAt set ("the time when the task transitioned from the PENDING state to the RUNNING state").
  - Raising desiredCount to 1 via UpdateService on a service placed ONLY in subnets with no route to the registry makes the scheduler launch a task that cannot pull its image, so that task becomes TERMINAL: ListTasks(desiredStatus="STOPPED") reports it, and DescribeTasks reports lastStatus="STOPPED" with ecs#Task.stopCode="TaskFailedToStart" (Smithy ecs#TaskStopCode values: TaskFailedToStart, EssentialContainerExited, UserInitiated, ServiceSchedulerInitiated, SpotInterruption, TerminationNotice, InfrastructureHealth; ecs#Task.stopCode: "The stop code indicating why a task was stopped. The stoppedReason might contain additional details.") plus a ecs#Task.stoppedReason describing the pull failure. Because the task never ran, ecs#Task.startedAt is ABSENT and no container carries an exitCode; every container is at lastStatus "STOPPED". An awsvpc task carries exactly one ecs#Attachment with type=="ElasticNetworkInterface" whose details carry subnetId (Smithy ecs#Attachment.details: "For elastic network interfaces, this includes the network interface ID, the MAC address, the subnet ID, and the private IPv4 address") naming one of the service's subnets, and ecs#Service.runningCount stays 0. The exact stoppedReason text (a CannotPullContainerError naming the image reference taken from the task definition) is recorded behavior only — the fixed text is the error class, the image reference/registry host are values from the request and the emulator's state.

Emulator: no transition happens at all. Every poll re-reads the same non-terminal state; the control task never reaches RUNNING or STOPPED and the scaled-up service never records a stopped task. The only output is `Error: Check timed out after 60.0s` — no assertion is ever reached, so the emulator never produces a lastStatus, stopCode, stoppedReason or ENI attachment to compare.

Why: this is the canonical "my task will not start" diagnosis. A client that raises desiredCount and then reads ListTasks(desiredStatus=STOPPED), Task.lastStatus/stopCode/stoppedReason/startedAt and Service.runningCount to decide whether a rollout succeeded gets a service that is permanently mid-flight: no success, no failure, no reason. Waiters and CI gates hang instead of reporting, and a network misconfiguration (isolated subnet, no NAT or VPC endpoint) is indistinguishable from a healthy estate.

Fix: the ECS task/service state machine under services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src — the scheduler path that advances a launched task to RUNNING for a routed placement, and to STOPPED with stopCode=TaskFailedToStart plus a pull-failure stoppedReason for an unroutable placement, populating the single ElasticNetworkInterface attachment's subnetId detail and leaving startedAt and container exitCode absent for a task that never ran.

Related open issues that do NOT cover this failure: 20261105T113651Z-ec2-ecr-ecs-scaling-an-isolated-subnet-fargate.md and 20261104T062603Z-ec2-ecr-ecs-a-task-in-an-isolated.md describe the same scenario, but the suite reports this check as UNOWNED — their recorded replay keys for it (_poll#27 / _poll#28) no longer match the site at which the replay now dies, so neither carries this check's current replay key.

Traces: ec2__ecr__ecs.ecsdeploy-isolated-subnet-task-blocked-from-starting.

## Evidence
Failing check ec2__ecr__ecs.ecsdeploy-isolated-subnet-task-blocked-from-starting ends with:
  Error: Check timed out after 60.0s
(no assertion in the check body was ever reached).

Expected (AWS):
  - RunTask in the routed/public subnet with assignPublicIp=ENABLED and a public image -> DescribeTasks().tasks[0].lastStatus == "RUNNING", startedAt present.
  - After UpdateService(desiredCount=1) on the service placed only in the isolated subnets:
      ListTasks(cluster, serviceName, desiredStatus="STOPPED").taskArns -> at least 1 arn
      DescribeTasks -> lastStatus == "STOPPED", stopCode == "TaskFailedToStart", stoppedReason set to the pull-failure text (CannotPullContainerError naming the image reference from the task definition), the 'startedAt' member ABSENT, taskDefinitionArn == the service's definition, exactly one attachment with type == "ElasticNetworkInterface" whose details include subnetId equal to one of the service's subnets, containers ["log-router", "service"] each at lastStatus "STOPPED" with NO exitCode
      DescribeServices -> runningCount == 0

Actual (emulator): none of those reads ever changes value. No task reaches RUNNING, no task reaches STOPPED, no stopCode/stoppedReason is ever produced, and the replay is killed at its budget.

## Steps to Reproduce
1. Create a VPC (e.g. 10.96.0.0/16) with two isolated subnets (own route table, no 0.0.0.0/0 route) in two AZs and one public subnet routed to an attached internet gateway, plus a security group.
2. Create an ECR repository, a CloudWatch log group, an IAM execution role trusting ecs-tasks.amazonaws.com, and an ECS cluster.
3. RegisterTaskDefinition: requiresCompatibilities=[FARGATE], networkMode=awsvpc, two containers (a non-essential log-router and an essential service container) referencing images in the (empty) ECR repository.
4. CreateService with desiredCount=0, launchType=FARGATE, the two ISOLATED subnets, assignPublicIp=DISABLED.
5. RunTask a control task in the PUBLIC subnet with assignPublicIp=ENABLED and a public image; poll DescribeTasks -> lastStatus never becomes RUNNING or STOPPED.
6. UpdateService(cluster, service, desiredCount=1) on the isolated service; poll ListTasks(desiredStatus="STOPPED") -> no stopped task ever appears, and DescribeTasks never reports stopCode=TaskFailedToStart. The replay exhausts its budget.
