Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ecs__elbv2.cli-elbv2-create-target-group-wrong-vpc-negative
Issue-Key: cloudformation__ecs__elbv2.cli-elbv2-create-target-group-wrong-vpc-negative|_target_states#30|Timeout|return [(d["Target"]["Id"], d["TargetHealth"]["State"]) for d in descs]
Issue-Commit: f4417a46337ab8dd5885f2c8c9ce0b65463e861c

# cloudformation__ecs__elbv2: a cross-VPC ELBv2 target group behind an ECS service never reaches a terminal target-health verdict (negative and positive control both hang)

> _Note: this issue was identified against the code base at commit `f4417a46337ab8dd5885f2c8c9ce0b65463e861c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTargetHealth

## Description
One net-new behavioral root cause of the `cloudformation__ecs__elbv2` suite, grounded in one failing behavioral check. A fixer must close it before submitting.

Cause 1 of 1: an ECS Fargate service whose `loadBalancers` entry points at a target group in a DIFFERENT VPC from the service's awsvpc subnets never produces a terminal target-health verdict, and neither does the same-VPC positive control — the check is cut off with no response to compare.

AWS: the shapes are modeled — ELBv2 CreateTargetGroup takes `VpcId` and `TargetType` (`ip` here) and DescribeTargetHealth returns `TargetHealthDescriptions[].Target.Id` plus `TargetHealth.State`, whose Smithy enum `TargetHealthStateEnum` is exactly {initial, healthy, unhealthy, unhealthy.draining, unused, draining, unavailable}, with `TargetHealth.Reason` documented as `Target.NotRegistered` / `Target.NotInUse` for `unused`, `Elb.RegistrationInProgress` / `Elb.InitialHealthChecking` for `initial`, and `Target.Timeout` / `Target.FailedHealthChecks` for `unhealthy`; ECS CreateService models `loadBalancers[].targetGroupArn/containerName/containerPort` and `networkConfiguration.awsvpcConfiguration`. The convergence TIMING is recorded behavior only: on AWS each of the surfaces this scenario polls settles well inside the suite's budget — ECS ListTasks reports the service's task as launched, DescribeTargetHealth on the wrong-VPC target group settles on a non-`healthy` state (the internal ALB in VPC B cannot reach the task ENI IP that lives in VPC A, so the target stays `initial`/`unhealthy` with `Target.Timeout`, never `healthy`), and the identical task definition and subnets behind a target group created in the service's OWN VPC does reach exactly one description whose `Target.Id` is the task ENI private IP and whose `TargetHealth.State` is `healthy`.

Emulator: the whole scenario is cut off with `Error: Check timed out after 60.0s` and no response to diff. The 60 s figure is the suite's budget, not a requirement — the requirement is that the polled surfaces reach a terminal value: ELBv2 DescribeLoadBalancers `State.Code` -> `active` for both ALBs, ECS ListTasks (desiredStatus RUNNING and STOPPED) showing the service's launched task, and ELBv2 DescribeTargetHealth settling on `healthy` for the same-VPC group while staying off `healthy` for the cross-VPC group. Hypothesis: the same missing ECS->ELBv2 target registration and inert target-health state machine already filed as open issue 20261105T132157Z-cloudformation-ecs-elbv2-an-ecs-service-with-a.md is what this check waits on; that issue does NOT carry this check's replay key, so it is filed separately and must be confirmed by re-replaying this check id.

Why: a client cannot distinguish "my target group is in the wrong VPC" from "my target group is still warming up" — DescribeTargetHealth never settles, so a deployment script or health gate hangs instead of being told the target is unused/unhealthy, and the VPC-scoping rule that makes a cross-VPC target group useless is unobservable.

Fix: the ECS service/task lifecycle code that must register each awsvpc task's ENI private IP as a target of the attached target group, and the ELBv2 target-health state machine that DescribeTargetHealth reads (initial -> healthy, and unhealthy/unused with the documented Reason when the load balancer's VPC cannot reach the registered IP), plus the DescribeLoadBalancers state transition to `active`.

Traces: cloudformation__ecs__elbv2.cli-elbv2-create-target-group-wrong-vpc-negative.

## Evidence
Cause 1 — cloudformation__ecs__elbv2.cli-elbv2-create-target-group-wrong-vpc-negative [mismatch]:
  Error: Check timed out after 60.0s
  (no response captured; the check is the only failing check in this service not already owned by an open issue or blocked by an unimplemented stub)

What the check drives before it is cut off: a shared VPC A with subnets plus an ALB SG and a task SG; an ECS execution role, cluster and awsvpc Fargate task definition exposing container port 80; a second, isolated VPC B; an INTERNAL application ALB in VPC B's subnets with VPC B's default SG; `aws elbv2 create-target-group --vpc-id <vpc-B> --target-type ip --protocol HTTP --port 80 --health-check-protocol HTTP --health-check-path / --healthy-threshold-count 2 --unhealthy-threshold-count 2 --health-check-interval-seconds 10`; a listener on port 80 forwarding to it; then the same pair (ALB + target group + listener) inside VPC A as the positive control; then CreateService (desiredCount=1, launchType FARGATE, loadBalancers=[{targetGroupArn: wrong-VPC TG, containerName 'app', containerPort 80}], awsvpcConfiguration on VPC A's subnets).

Expected on AWS: DescribeLoadBalancers reports `State.Code` `active` for both ALBs; ECS ListTasks reports >= 1 launched task for the service; DescribeTargetHealth on the VPC-B target group never contains a description with `TargetHealth.State == "healthy"` (it settles on `initial`/`unhealthy`, Reason `Target.Timeout`, or `unused` with Reason `Target.NotRegistered`); DescribeTargetHealth on the VPC-A target group converges to exactly one description whose `Target.Id` is the task's ENI private IP and whose `TargetHealth.State` is `healthy`.

Observed: no verdict at all — the check is terminated at the suite budget, so none of those surfaces was observed to settle.

Smithy consulted (services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10): DescribeTargetHealth -> TargetHealthDescriptions[].Target (TargetDescription.Id), HealthCheckPort, TargetHealth{State,Reason,Description}; TargetHealthStateEnum = initial|healthy|unhealthy|unhealthy.draining|unused|draining|unavailable; TargetHealth.Reason docs for Elb.RegistrationInProgress, Elb.InitialHealthChecking, Target.Timeout, Target.FailedHealthChecks, Target.NotRegistered, Target.NotInUse; CreateTargetGroup.VpcId / TargetType. ECS CreateService.loadBalancers and networkConfiguration.awsvpcConfiguration. Timing is recorded behavior only.

Related (does NOT cover this check id): open issue 20261105T132157Z-cloudformation-ecs-elbv2-an-ecs-service-with-a.md.

## Steps to Reproduce
1. Create VPC A with two subnets, an ALB security group and a task security group; create an ECS cluster, an execution role, and register an awsvpc Fargate task definition with container 'app' exposing port 80.
2. Create a second, isolated VPC B with its own subnets and default security group.
3. In VPC B: create an internal application load balancer on VPC B's subnets, then `aws elbv2 create-target-group --name <tg> --protocol HTTP --port 80 --vpc-id <vpc-B> --target-type ip --health-check-protocol HTTP --health-check-path / --healthy-threshold-count 2 --unhealthy-threshold-count 2 --health-check-interval-seconds 10`, and create a listener on port 80 forwarding to it. Poll DescribeLoadBalancers until `State.Code` is `active`.
4. In VPC A: create an ALB, a target group (TargetType ip) and a port-80 listener forwarding to it, as the positive control. Poll until the ALB is `active`.
5. ECS CreateService with desiredCount=1, launchType FARGATE, awsvpcConfiguration on VPC A's subnets/task SG, and loadBalancers=[{targetGroupArn: the VPC-B target group, containerName 'app', containerPort 80}].
6. Poll ECS ListTasks (desiredStatus RUNNING and STOPPED) until the service has launched >= 1 task — expect this to settle.
7. Poll ELBv2 DescribeTargetHealth on the VPC-B target group: expect every `TargetHealth.State` to be a terminal non-`healthy` value (initial/unhealthy with Reason Target.Timeout, or unused with Reason Target.NotRegistered), never `healthy`.
8. CreateService a second service with the same task definition and subnets but loadBalancers pointing at the VPC-A target group, and poll DescribeTargetHealth there: expect exactly one description whose `Target.Id` is the task ENI private IP and whose `TargetHealth.State` is `healthy`. Observe that neither surface settles and the check is cut off.
9. Re-replay to confirm the fix: cloudformation__ecs__elbv2.cli-elbv2-create-target-group-wrong-vpc-negative.
