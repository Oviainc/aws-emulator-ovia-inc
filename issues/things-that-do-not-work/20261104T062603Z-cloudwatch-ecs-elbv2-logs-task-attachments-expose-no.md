Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ecs__elbv2__logs.alb-routes-traffic-only-to-healthy-ecs-targets cloudwatch__ecs__elbv2__logs.alb-target-group-registers-ecs-tasks cloudwatch__ecs__elbv2__logs.ecs-service-without-load-balancer-config-no-targets-registered cloudwatch__ecs__elbv2__logs.alb-deregisters-targets-on-service-scale-down cloudwatch__ecs__elbv2__logs.alb-health-check-reflects-ecs-task-health cloudwatch__ecs__elbv2__logs.alb-security-group-mismatch-blocks-health-checks cloudwatch__ecs__elbv2__logs.ecs-log-group-name-mismatch-no-delivery cloudwatch__ecs__elbv2__logs.ecs-log-retention-policy-applied-to-loggroup cloudwatch__ecs__elbv2__logs.ecs-log-stream-prefix-embeds-task-id cloudwatch__ecs__elbv2__logs.ecs-multi-container-task-creates-distinct-log-streams cloudwatch__ecs__elbv2__logs.ecs-task-fails-log-delivery-without-logs-permission cloudwatch__ecs__elbv2__logs.ecs-container-insights-disabled-no-detailed-metrics cloudwatch__ecs__elbv2__logs.ecs-task-execution-role-trusts-ecs-tasks-service
Issue-Key: cloudwatch__ecs__elbv2__logs.alb-routes-traffic-only-to-healthy-ecs-targets|_task_private_ip#17|KeyError|return task["containers"][0]["networkInterfaces"][0]["privateIpv4Address"]
Issue-Key: cloudwatch__ecs__elbv2__logs.alb-target-group-registers-ecs-tasks|_task_private_ip#16|KeyError|return task["containers"][0]["networkInterfaces"][0]["privateIpv4Address"]
Issue-Key: cloudwatch__ecs__elbv2__logs.ecs-service-without-load-balancer-config-no-targets-registered|_task_private_ip#16|KeyError|return task["containers"][0]["networkInterfaces"][0]["privateIpv4Address"]
Issue-Key: cloudwatch__ecs__elbv2__logs.alb-deregisters-targets-on-service-scale-down|_poll_until#16|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__ecs__elbv2__logs.alb-health-check-reflects-ecs-task-health|_poll_until#17|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__ecs__elbv2__logs.alb-security-group-mismatch-blocks-health-checks|run#19|Timeout|time.sleep(5.0)
Issue-Key: cloudwatch__ecs__elbv2__logs.ecs-log-group-name-mismatch-no-delivery|_wait_for_task_stopped#10|Timeout|return last
Issue-Key: cloudwatch__ecs__elbv2__logs.ecs-log-retention-policy-applied-to-loggroup|_wait_for_task_stopped#9|Timeout|return last
Issue-Key: cloudwatch__ecs__elbv2__logs.ecs-log-stream-prefix-embeds-task-id|_wait_for_task_stopped#13|Timeout|return last
Issue-Key: cloudwatch__ecs__elbv2__logs.ecs-multi-container-task-creates-distinct-log-streams|_wait_for_task_stopped#14|Timeout|return last
Issue-Key: cloudwatch__ecs__elbv2__logs.ecs-task-fails-log-delivery-without-logs-permission|_wait_for_task_stopped#7|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__ecs__elbv2__logs.ecs-container-insights-disabled-no-detailed-metrics|_poll_metric_datapoints#11|Timeout|time.sleep(2)
Issue-Key: cloudwatch__ecs__elbv2__logs.ecs-task-execution-role-trusts-ecs-tasks-service|_wait_for_task_stopped#16|Timeout|time.sleep(interval_s)
Issue-Commit: b64c3d45927e2833db57dde0eaeae2a33a5b497f

# cloudwatch__ecs__elbv2__logs: task attachments expose no networkInterfaces, ECS tasks never register as ALB targets, awslogs streams are never created, and Container Insights publishes nothing (5 causes)

> _Note: this issue was identified against the code base at commit `b64c3d45927e2833db57dde0eaeae2a33a5b497f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTasks

## Description
Cause 1 of 5: an awsvpc task exposes no network interface, so every consumer that reads the task's ENI crashes.
AWS: Smithy com.amazonaws.ecs#Attachment.details documents that an elastic-network-interface attachment carries the network interface ID, MAC address, subnet ID and private IPv4 address; clients read the task's networkInterfaces/privateIPv4Address from there.
Emulator: the attachment holds only subnetId, so the lookup fails outright — `KeyError: 'networkInterfaces'`.
Fix: the ECS task attachment/ENI materialisation (see the companion ecs issue on PRECREATED attachments).
Traces: cloudwatch__ecs__elbv2__logs.alb-routes-traffic-only-to-healthy-ecs-targets, cloudwatch__ecs__elbv2__logs.alb-target-group-registers-ecs-tasks, cloudwatch__ecs__elbv2__logs.ecs-service-without-load-balancer-config-no-targets-registered.

Cause 2 of 5: an ECS service with a loadBalancers entry never registers (or deregisters) its tasks as ELBv2 targets and target health never tracks the task.
AWS: the service registers each task IP/instance into the target group, DescribeTargetHealth reports it healthy once the health check passes, a scale-down deregisters it, and a security-group mismatch keeps it unhealthy.
Emulator: all three checks poll until the 60s budget expires.
Fix: the ECS service-to-ELBv2 target registration path and the target-group health evaluation.
Traces: cloudwatch__ecs__elbv2__logs.alb-deregisters-targets-on-service-scale-down, cloudwatch__ecs__elbv2__logs.alb-health-check-reflects-ecs-task-health, cloudwatch__ecs__elbv2__logs.alb-security-group-mismatch-blocks-health-checks.

Cause 3 of 5: the awslogs log driver never creates a stream or writes events.
AWS: each container gets a stream named <prefix>/<container>/<task-id> in the configured group, a multi-container task produces one stream per container, a group-name mismatch delivers nothing, a retention policy applies to the created group, and a task whose execution role lacks logs permissions fails delivery.
Emulator: all five checks poll until the 60s budget expires — no stream is ever created.
Fix: the ECS awslogs log-driver delivery component.
Traces: cloudwatch__ecs__elbv2__logs.ecs-log-group-name-mismatch-no-delivery, cloudwatch__ecs__elbv2__logs.ecs-log-retention-policy-applied-to-loggroup, cloudwatch__ecs__elbv2__logs.ecs-log-stream-prefix-embeds-task-id, cloudwatch__ecs__elbv2__logs.ecs-multi-container-task-creates-distinct-log-streams, cloudwatch__ecs__elbv2__logs.ecs-task-fails-log-delivery-without-logs-permission.

Cause 4 of 5: Container Insights publishes no metrics, so the disabled-vs-enabled distinction is unobservable.
AWS: with containerInsights enabled the cluster publishes ECS/ContainerInsights metrics; with it disabled it publishes none.
Emulator: the check polls until the budget expires — neither side can be observed.
Fix: the cluster-setting-driven metric publication into CloudWatch.
Trace: cloudwatch__ecs__elbv2__logs.ecs-container-insights-disabled-no-detailed-metrics.

Cause 5 of 5: the execution-role trust requirement is never exercised because the task never settles.
AWS: a task whose execution role trusts ecs-tasks.amazonaws.com starts; one that does not is refused.
Emulator: the check polls until the budget expires.
Fix: the ECS task-launch role-assumption evaluation plus the task runtime.
Trace: cloudwatch__ecs__elbv2__logs.ecs-task-execution-role-trusts-ecs-tasks-service.

## Evidence
1. `KeyError: 'networkInterfaces'` (x3) while reading the task's attachment details.
2-5. `Error: Check timed out after 60.0s` on the listed checks. The sibling elbv2 recording in the same run shows the related `TargetGroupNotFound` / empty DescribeTargetHealth shape.

## Steps to Reproduce
1. RunTask awsvpc, DescribeTasks and read attachments[0].details for networkInterfaceId/privateIPv4Address -> absent.
2. CreateTargetGroup(target-type ip) + CreateService with loadBalancers, then poll DescribeTargetHealth -> never registers; scale the service down -> never deregisters.
3. RunTask with awslogs logConfiguration (awslogs-stream-prefix set) and poll DescribeLogStreams -> no stream.
4. CreateCluster with containerInsights enabled, run a task, poll CloudWatch ListMetrics for ECS/ContainerInsights -> nothing.
5. Launch a task with an execution role lacking the ecs-tasks trust -> no refusal and no settled state.
