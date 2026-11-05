Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr__ecs__elbv2.elbv2-target-group-registers-ecs-service-tasks ecr__ecs__elbv2.elbv2-no-target-registration-without-service-lb-config
Issue-Key: ecr__ecs__elbv2.elbv2-target-group-registers-ecs-service-tasks|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Key: ecr__ecs__elbv2.elbv2-no-target-registration-without-service-lb-config|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Commit: 68ec0c9cae77cfbc133f0186a4c923306bef5402

# ecr__ecs__elbv2: a Fargate service with desiredCount=1 never reaches runningCount=1, so neither the ALB target-registration check nor its negative control can reach a verdict (1 cause)

> _Note: this issue was identified against the code base at commit `68ec0c9cae77cfbc133f0186a4c923306bef5402`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateService

## Description
Cause 1 of 1: an ECS Fargate service created with desiredCount=1 and networkMode=awsvpc never converges — DescribeServices keeps reporting a runningCount below desiredCount, so no task ever becomes readable as RUNNING, no awsvpc ENI private IPv4 address is exposed, and a target group named in the service's loadBalancers block is never populated. Both checks of this service spend their whole budget in the convergence poll and are cut off (`Error: Check timed out after 60.0s`): the positive check cannot assert the registered target, and the negative-delivery check cannot even establish its positive control (the wired service registering) before asserting the unwired service's target group stays empty, so the absence it is meant to prove is unprovable.

AWS: CreateService with desiredCount=1, launchType=FARGATE, networkMode=awsvpc and loadBalancers=[{targetGroupArn, containerName, containerPort}] launches one task. Smithy com.amazonaws.ecs#Service.runningCount is "The number of tasks in the cluster that are in the RUNNING state" and reaches the desiredCount; com.amazonaws.ecs#Deployment.rolloutState documents that a deployment "begins in an IN_PROGRESS state. When the service reaches a steady state, the deployment transitions to a COMPLETED state. If the service fails to reach a steady state and circuit breaker is turned on, the deployment transitions to a FAILED state" — i.e. the service always settles into a terminal, observable state. DescribeTasks then exposes the task's attachment (com.amazonaws.ecs#Attachment.details: "For elastic network interfaces, this includes the network interface ID, the MAC address, the subnet ID, and the private IPv4 address"), and ELBv2 DescribeTargetHealth on the wired target group reports exactly one TargetHealthDescription whose Target.Id is that privateIPv4Address and whose Target.Port is the container port; a target group NOT named by any service's loadBalancers block reports no descriptions.

Emulator: neither a terminal nor a converged state is ever reached. runningCount never equals desiredCount, DescribeTasks lists no RUNNING task to read an ENI from, and DescribeTargetHealth on the wired target group stays empty — the replay produces no status code or error, only the harness cut-off after 60s while polling DescribeServices.

Why: a load-balanced ECS service is unusable and undiagnosable. Any client that waits for runningCount == desiredCount (the botocore services_stable waiter, CloudFormation's ECS service stabilization, a deploy script, a blue/green rollout) blocks indefinitely instead of succeeding or failing, and a client that confirms a rollout via DescribeTargetHealth sees an empty target group with no reason given. A hang is strictly worse for a caller than a wrong answer, since no error path is taken.

Fix: the ECS service scheduler / task lifecycle (the CreateService handler and the task state machine it drives) must launch desiredCount tasks for a FARGATE service, advance them to lastStatus RUNNING while updating Service.runningCount/pendingCount and the PRIMARY Deployment's counts and rolloutState to a terminal value, synthesize the awsvpc ElasticNetworkInterface attachment with a privateIPv4Address drawn from the task's subnet, and register that IP:containerPort into the target group named by the service's loadBalancers entry (and ONLY into that one) so ELBv2 DescribeTargetHealth reflects it. The open issue 20261105T113651Z-ecr-ecs-elbv2-a-fargate-service-with-desiredcount.md describes this same non-convergence and 20261104T060239Z-ecr-ecs-elbv2-untrusted-execution-roles-and-unpullable.md lists a cause about task ENIs carrying no private IP, but the suite reports both of these checks as owned by neither (their recorded replay keys do not match where the replay now stops), and the negative-delivery check elbv2-no-target-registration-without-service-lb-config is not carried by any open issue.

Traces: ecr__ecs__elbv2.elbv2-target-group-registers-ecs-service-tasks, ecr__ecs__elbv2.elbv2-no-target-registration-without-service-lb-config.

## Evidence
Suite: service ecr__ecs__elbv2 scored 4/16 (25%), 10 failed; these two are the failures not already owned.

[mismatch] ecr__ecs__elbv2.elbv2-target-group-registers-ecs-service-tasks
  Error: Check timed out after 60.0s   (blocked in _wait_running_count(..., 1, timeout_s=360) before `assert len(ips) == 1`)

[mismatch] ecr__ecs__elbv2.elbv2-no-target-registration-without-service-lb-config
  Error: Check timed out after 60.0s   (blocked in the same convergence poll for the two services it creates, before the positive control _wait_target_count(tg_wired, 1) and the 60s sustained-emptiness window on tg_unused)

Expected (AWS), after CreateService(desiredCount=1, launchType=FARGATE, networkMode=awsvpc, subnets=2 public subnets, assignPublicIp=ENABLED, loadBalancers=[{targetGroupArn, containerName='app', containerPort=80}]):
  - DescribeServices -> services[0].runningCount == 1, PRIMARY deployment rolloutState COMPLETED
  - ListTasks(desiredStatus='RUNNING') -> one taskArn; DescribeTasks -> attachments[].details contains name='privateIPv4Address'
  - DescribeTargetHealth(wired TargetGroupArn) -> exactly one TargetHealthDescription, Target.Id == that privateIPv4Address, Target.Port == 80
  - DescribeTargetHealth(target group not referenced by any service) -> [] (sustained)

Actual (emulator): runningCount never reaches 1 for either service, so no RUNNING task, no privateIPv4Address detail and no TargetHealthDescription is ever produced; both checks are killed at the harness budget with no captured response.

## Steps to Reproduce
1. Create a VPC (10.99.0.0/16) with DNS support and hostnames, an internet gateway attached, two public subnets (MapPublicIpOnLaunch=true) in two AZs, a route table with 0.0.0.0/0 -> IGW associated to both subnets, and a self-referencing security group allowing tcp/0-65535.
2. Create an ECS cluster and RegisterTaskDefinition: networkMode=awsvpc, requiresCompatibilities=[FARGATE], cpu=256, memory=512, one essential container 'app' (busybox httpd) with portMappings containerPort=80/tcp.
3. Create a target group (Protocol=HTTP, Port=80, VpcId, TargetType=ip) that no service will reference, plus an internet-facing application load balancer across both subnets, a second target group, and an HTTP listener forwarding to it.
4. CreateService A with desiredCount=1, launchType=FARGATE, the two subnets, the security group and NO loadBalancers block; CreateService B identically but with loadBalancers=[{targetGroupArn=<wired tg>, containerName='app', containerPort=80}].
5. Poll DescribeServices for runningCount == 1 on each service.
   -> AWS: both reach 1 and their PRIMARY deployments reach a terminal rolloutState.
   -> Emulator: runningCount never reaches 1; the replay is killed at the 60s budget.
6. (Unreachable on the emulator) DescribeTargetHealth on the wired target group -> one target whose Id is the task ENI's privateIPv4Address and Port 80; DescribeTargetHealth on the unreferenced target group -> [] for a sustained 60s window.
