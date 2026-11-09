Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ecs__elbv2__logs.alb-target-group-registers-ecs-tasks
Issue-Key: cloudwatch__ecs__elbv2__logs.alb-target-group-registers-ecs-tasks|_poll_until#21|Timeout|time.sleep(interval_s)
Issue-Commit: e3ea9c3d18c5ac215ea88ebad8a0c22355767e25

# cloudwatch__ecs__elbv2__logs: an ECS service with a loadBalancers config never gets its awsvpc task registered as an ALB target and its container never produces an awslogs stream

> _Note: this issue was identified against the code base at commit `e3ea9c3d18c5ac215ea88ebad8a0c22355767e25`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateService

## Description
One root cause, grounded in the failing check `cloudwatch__ecs__elbv2__logs.alb-target-group-registers-ecs-tasks`, which the report marks neither ALREADY FILED nor OWNED by an open issue.

Cause 1 of 1: an ECS service created with a `loadBalancers` entry binding an ELBv2 target group never reaches the state the check grades — the service's awsvpc Fargate task's private IP is never registered as a target in that target group, and the container never writes a CloudWatch Logs stream proving it actually ran — so the check exhausts its harness budget without producing a verdict.
  AWS: CreateService with `loadBalancers: [{targetGroupArn, containerName, containerPort}]` and `networkConfiguration.awsvpcConfiguration` makes ECS register each task's ENI private IP into the target group as an `ip`-type target; `elbv2 describe-target-health` then lists that IP with `Target.Port` equal to the container port, and the service's `runningCount` reaches the desired count. The task's `awslogs` log configuration creates one log stream per container under the configured group, and `logs get-log-events` on it returns the container's stdout. Smithy (com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113): CreateService input carries `loadBalancers: LoadBalancers` (LoadBalancer {targetGroupArn, loadBalancerName, containerName, containerPort}) and `networkConfiguration`; the model states the shapes, not the registration or log-delivery timing, so the timing is recorded behavior only — in the recording the target appears and the log stream carries events well inside the check's budget.
  Emulator: the check produces no verdict at all — `Error: Check timed out after 60.0s`. It is cut inside the polls that wait on the data plane rather than on an API answer: `_wait_running_count` for the service, `_wait_target_states(tg_arn, timeout_s=180)` for at least one registered target, and `_poll_until(describe_log_streams, len>=1, timeout_s=120)` are each longer than the harness budget, so the run ends mid-poll. Hypothesis: the ECS->ELBv2 target registration for a service's awsvpc tasks never happens and the container never executes, so neither the target nor the log stream ever appears; on the evidence available the only established fact is that none of those gates is reached.
  Why: a client deploying the standard ALB-fronted ECS service pattern sees a service that is reported created but whose tasks are never behind the load balancer and never log, so health checks, routing and log-based verification all silently never converge instead of succeeding or failing.
  Fix: the ECS service's load-balancer attachment path (register each awsvpc task's ENI IP as an `ip` target in the bound target group, and deregister on task stop) together with the task/container execution and `awslogs` delivery that produce the log stream — i.e. the CreateService handler's loadBalancers handling plus the task state/log plumbing in `crate::state`.
  Related open issues for this service group, neither of which carries this check's replay key: 20261108T191814Z-cloudwatch-ecs-elbv2-logs-an-alb-answers-502.md (ALB answers 502) and 20261104T062603Z-cloudwatch-ecs-elbv2-logs-task-attachments-expose-no.md (task attachments expose no ENI details). Closing either without replaying this check would leave this failure unrun.
  Failing check id: cloudwatch__ecs__elbv2__logs.alb-target-group-registers-ecs-tasks.

## Evidence
Expected (recorded AWS): `ecs create-service --load-balancers targetGroupArn=<tg>,containerName=app,containerPort=80 --network-configuration awsvpcConfiguration={subnets=[...],securityGroups=[...]}` -> service runningCount 1; `elbv2 describe-target-health --target-group-arn <tg>` lists the task's ENI private IP with port 80; `logs describe-log-streams --log-group-name <lg>` returns >=1 stream and `logs get-log-events` on it returns >=1 event.
Actual (emulator): `Error: Check timed out after 60.0s` — no assertion in the check body is ever reached. The body's gates, in order, are `_wait_running_count(cluster, service, 1)`, then `_wait_target_states(tg_arn, timeout_s=180.0, interval_s=10.0, predicate=lambda s: len(s) >= 1)` with `assert any(t["id"] == private_ip and t["port"] == 80 for t in states)`, then `_poll_until(describe_log_streams, lambda s: len(s) >= 1, timeout_s=120.0)` with `assert len(events) >= 1`.

## Steps to Reproduce
1. In the default VPC create two security groups (ALB ingress, task ingress from the ALB on port 80) and authorize the task SG from the ALB SG.
2. `elbv2 create-load-balancer` across the default subnets with the ALB SG; `elbv2 create-target-group --target-type ip --port 80 --health-check-path /` in that VPC; `elbv2 create-listener` forwarding to it.
3. Create an ECS task execution role, a CloudWatch log group and an ECS cluster; `ecs register-task-definition` for `public.ecr.aws/docker/library/nginx:alpine` with containerPort 80 and an `awslogs` logConfiguration pointing at the log group.
4. `ecs create-service --desired-count 1 --launch-type FARGATE --network-configuration awsvpcConfiguration={subnets=[...],securityGroups=[<task sg>]} --load-balancers targetGroupArn=<tg>,containerName=<name>,containerPort=80`.
5. Poll `ecs describe-services` for runningCount 1, then `elbv2 describe-target-health --target-group-arn <tg>` for the task's private IP, then `logs describe-log-streams --log-group-name <lg>` and `logs get-log-events` -> none of these gates is reached; the check is cut at its 60s budget.
Replay: cloudwatch__ecs__elbv2__logs.alb-target-group-registers-ecs-tasks.
