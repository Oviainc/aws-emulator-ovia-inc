Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ecs__elbv2__logs.alb-target-group-registers-ecs-tasks cloudwatch__ecs__elbv2__logs.alb-routes-traffic-only-to-healthy-ecs-targets cloudwatch__ecs__elbv2__logs.ecs-service-without-load-balancer-config-no-targets-registered
Issue-Key: cloudwatch__ecs__elbv2__logs.alb-target-group-registers-ecs-tasks|_poll_until#17|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__ecs__elbv2__logs.alb-routes-traffic-only-to-healthy-ecs-targets|_poll_until#22|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__ecs__elbv2__logs.ecs-service-without-load-balancer-config-no-targets-registered|_poll_until#22|Timeout|time.sleep(interval_s)
Issue-Commit: 68ec0c9cae77cfbc133f0186a4c923306bef5402

# cloudwatch__ecs__elbv2__logs: 3 net-new causes — ECS tasks never become ALB targets, so health-based routing and the no-loadBalancers negative control are both unreachable

> _Note: this issue was identified against the code base at commit `68ec0c9cae77cfbc133f0186a4c923306bef5402`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTargetHealth

## Description
Three net-new root causes. All three checks were listed as failing WITHOUT an ALREADY FILED or OWNED marker. The open issue 20261104T062603Z-cloudwatch-ecs-elbv2-logs-task-attachments-expose-no.md covers adjacent causes in this service heading but does not carry these three replay keys.

Cause 1 of 3: a loadBalancers-bound ECS service never registers its Fargate task's private IP, and the task produces no CloudWatch Logs output to prove it is alive.
AWS: an ECS service created with loadBalancers auto-registers the running task's awsvpc ENI private IPv4 (Smithy: ecs#NetworkInterface.privateIpv4Address, ecs#Attachment.details) as an `ip` target of the bound target group; the container's stdout is delivered by the awslogs driver to the configured log group, so GetLogEvents returns the container's output. Recorded behavior only for timing.
Emulator: the check is killed with "Check timed out after 60.0s" while still polling for the target to appear; no target is ever registered and no log event is ever delivered.
Why: a caller cannot tell that a service is serving — neither the ALB target list nor the task's logs ever show evidence the container exists.
Fix: ECS task-attachment state (populate ENI id + private IPv4), the ECS→ELBv2 registration side effect, and the awslogs delivery path.
Traces: cloudwatch__ecs__elbv2__logs.alb-target-group-registers-ecs-tasks.

Cause 2 of 3: the ALB never excludes an unhealthy target from routing, because no ECS target is ever healthy to compare against.
AWS: with one reachable ECS-task target and one permanently unreachable target in the same target group, the ALB health check marks the unreachable one 'unhealthy' and routes 100% of requests to the healthy target.
Emulator: the check is killed with "Check timed out after 60.0s"; neither target health transition nor request distribution is ever observable.
Why: health-check-based traffic exclusion — the core ALB guarantee — cannot be relied on or even inspected.
Fix: the ELBv2 target-group health-check evaluator and the listener's forwarding decision, on top of Cause 1.
Traces: cloudwatch__ecs__elbv2__logs.alb-routes-traffic-only-to-healthy-ecs-targets.

Cause 3 of 3: the negative control "a service without loadBalancers registers no target" cannot conclude, because its positive twin never registers either.
AWS: a service with NO loadBalancers entry registers nothing in the target group, while an otherwise identical service WITH the entry does register — which is what proves the missing wiring, not the network path, is the blocker.
Emulator: the check is killed with "Check timed out after 60.0s" waiting on the positive twin to register. The emulator produces the same empty target list for both services, so the distinction AWS makes is not represented.
Why: a client cannot distinguish "I forgot the loadBalancers block" from "registration is broken" — both look identical.
Fix: same registration path as Cause 1; the no-loadBalancers case must stay empty while the configured case registers.
Traces: cloudwatch__ecs__elbv2__logs.ecs-service-without-load-balancer-config-no-targets-registered.

## Evidence
Expected (AWS): DescribeTargetHealth lists the Fargate task's ENI private IPv4 as a registered target (and the task's stdout appears in CloudWatch Logs); an unreachable sibling target goes 'unhealthy' and receives no requests; a service with no loadBalancers entry registers nothing while its configured twin does.
Actual (emulator): all three checks are terminated by the harness with `Check timed out after 60.0s`; the target group stays empty in every case, so health state, routing distribution and the positive/negative distinction are all unobservable.

## Steps to Reproduce
1. Create a VPC/subnets, two security groups, an ALB, an `ip` target group and a CloudWatch Logs group.
2. RegisterTaskDefinition (awsvpc/FARGATE, awslogs driver pointed at the log group).
3. CreateService with loadBalancers=[{targetGroupArn, containerName, containerPort}], desiredCount=1.
4. Poll DescribeTargetHealth → target group stays empty; poll GetLogEvents → no stream/event. (alb-target-group-registers-ecs-tasks times out at 60s.)
5. Add a second, permanently unreachable target and poll target health / issue repeated GETs to the ALB DNS name → no health transition, no routing evidence (alb-routes-traffic-only-to-healthy-ecs-targets times out).
6. Create a second service WITHOUT loadBalancers and compare → both are empty, so the negative control cannot conclude (ecs-service-without-load-balancer-config-no-targets-registered times out).
