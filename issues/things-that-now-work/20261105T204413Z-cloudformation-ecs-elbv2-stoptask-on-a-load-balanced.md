Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ecs__elbv2.ecs-task-stopped-event-triggers-target-deregistration-event
Issue-Key: cloudformation__ecs__elbv2.ecs-task-stopped-event-triggers-target-deregistration-event|_poll_for_event#25|Timeout|time.sleep(interval_s)
Issue-Commit: e730f41abf8cc315135e4118582590f4999ad0e7

# cloudformation__ecs__elbv2: StopTask on a load-balanced ECS service publishes no ECS Task State Change STOPPED event and never drains the task's ALB target

> _Note: this issue was identified against the code base at commit `e730f41abf8cc315135e4118582590f4999ad0e7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StopTask

## Description
One net-new root cause, grounded in one failing behavioral check that the replay report listed WITHOUT an ALREADY FILED or OWNED marker. The open issue 20261105T132157Z-cloudformation-ecs-elbv2-an-ecs-service-with-a.md names this check in its trace list but does not carry the replay key it now fails on (its key for this check is `_poll_for_event#25|Timeout|...`), so this failure is not covered by it; it is filed here and that issue is named for cross-reference.

Cause 1 of 1: stopping a running task of a load-balanced ECS service produces neither the EventBridge notification nor the ELBv2 target transition that AWS produces, so the whole stop-propagation sequence never reaches a verdict.
AWS: Smithy models every surface the sequence reads — ecs#StopTask (input cluster/task/reason), ecs#Task.lastStatus / ecs#Task.attachments (ecs#Attachment.details carries the ENI id, subnet id and private IPv4 address for an ElasticNetworkInterface attachment), and elasticloadbalancingv2#DescribeTargetHealth -> TargetHealthDescriptions[].Target.Id with TargetHealth.State drawn from elasticloadbalancingv2#TargetHealthStateEnum (initial/healthy/unhealthy/unhealthy.draining/unused/draining/unavailable). The event payload itself is recorded behavior only: after StopTask, ECS publishes an EventBridge event with source `aws.ecs`, detail-type `ECS Task State Change` and a detail object whose `taskArn` is the stopped task's ARN, whose `clusterArn` is the cluster's ARN and whose `lastStatus` is `STOPPED`; an EventBridge rule matching that pattern delivers it to its SQS target. In the same window the task's registered `ip` target in the attached target group moves to TargetHealth.State `draining` and, after the target group's deregistration delay, disappears from DescribeTargetHealth entirely. The ARNs, cluster names, task ids and private IPs in the message/detail are values taken from the request and the emulator's own state; the fixed parts are the source, detail-type, the `lastStatus` value `STOPPED`, and the `draining` target state.
Emulator: the check is terminated by the harness with `Error: Check timed out after 60.0s` with no response to compare. Nothing in the sequence converges: the service/task/target-health surfaces it polls (ECS DescribeServices stability, ECS ListTasks/DescribeTasks, SQS ReceiveMessage for the rule's delivery, ELBv2 DescribeTargetHealth for the victim IP) never reach a terminal value, so neither the STOPPED event nor the `draining`/removed target state is ever observable. The 60 s figure is the suite's budget, not a requirement — the requirement is that those surfaces settle.
Why: a client that drains traffic before stopping a task, or that reacts to ECS Task State Change events to update routing, dashboards or alarms, gets silence. It cannot tell a stopped task from a running one through the load balancer, and any automation keyed on the STOPPED event never fires, so blue/green and scale-in flows written against AWS hang against the emulator.
Fix: the ECS task lifecycle in the StopTask handler and the task state it writes (lastStatus/stoppedReason/stopCode plus the ENI attachment details), the ECS -> EventBridge task-state-change publication path, and the ECS -> ELBv2 target deregistration side effect including the ELBv2 target-health state machine's `draining` transition and deregistration-delay removal.
Traces: cloudformation__ecs__elbv2.ecs-task-stopped-event-triggers-target-deregistration-event.

## Evidence
Failing check: cloudformation__ecs__elbv2.ecs-task-stopped-event-triggers-target-deregistration-event [mismatch]
  Error: Check timed out after 60.0s

The check had already (per its replayed source): built the VPC/subnets/SG, an execution role, an ECS cluster, an awsvpc task definition with container 'web' on port 80, an ALB, a target group with deregistration_delay=20 and a forwarding listener; created the service with desiredCount=1 and loadBalancers=[{targetGroupArn, containerName:'web', containerPort:80}]; created an SQS queue, a PutRule with EventPattern {"source":["aws.ecs"],"detail-type":["ECS Task State Change"],"detail":{"lastStatus":["STOPPED"],"clusterArn":[<cluster arn>]}}, authorized events.amazonaws.com on the queue and PutTargets'd the queue; then called StopTask.

Expected (AWS): within the poll budget, SQS ReceiveMessage yields a message whose JSON body has detail.taskArn == the stopped task's ARN and detail.lastStatus == "STOPPED" (source "aws.ecs", detail-type "ECS Task State Change"); and ELBv2 DescribeTargetHealth on the attached target group reports the task's ENI private IPv4 with TargetHealth.State "draining", then stops listing it after the deregistration delay.
Actual (emulator): no response at all — the check is cut off at the suite budget, so no event is delivered and no target-health transition is produced.

Smithy consulted: ecs#StopTask; ecs#Task.lastStatus; ecs#Attachment.details ("For elastic network interfaces, this includes the network interface ID, the MAC address, the subnet ID, and the private IPv4 address"); elasticloadbalancingv2#DescribeTargetHealth -> TargetHealthDescriptions[].Target.Id / TargetHealth.State; elasticloadbalancingv2#TargetHealthStateEnum = [initial, healthy, unhealthy, unhealthy.draining, unused, draining, unavailable]. The event envelope/detail contents and the convergence timing are recorded behavior only.

## Steps to Reproduce
1. Create a VPC with two subnets and a security group; create an ECS cluster; RegisterTaskDefinition (awsvpc, FARGATE) with container 'web' exposing containerPort 80.
2. Create an ELBv2 application load balancer in those subnets, an `ip` target group in that VPC with a 20 s deregistration delay, and a listener forwarding to it.
3. CreateService with desiredCount=1, awsvpcConfiguration on those subnets and loadBalancers=[{targetGroupArn, containerName:'web', containerPort:80}]; wait for the deployment to report stable and read the running task's ARN and ENI private IPv4.
4. CreateQueue; PutRule with EventPattern {"source":["aws.ecs"],"detail-type":["ECS Task State Change"],"detail":{"lastStatus":["STOPPED"],"clusterArn":[<cluster arn>]}}; SetQueueAttributes granting sqs:SendMessage to events.amazonaws.com for that rule ARN; PutTargets the queue.
5. StopTask that task, then poll SQS ReceiveMessage: expect a body with detail.taskArn == the task ARN and detail.lastStatus == "STOPPED". Observe none.
6. Poll ELBv2 DescribeTargetHealth on the target group for that private IP: expect TargetHealth.State "draining" and then its removal. Observe no such description and no transition; the check is killed at the 60 s budget.
7. Re-replay: cloudformation__ecs__elbv2.ecs-task-stopped-event-triggers-target-deregistration-event.
