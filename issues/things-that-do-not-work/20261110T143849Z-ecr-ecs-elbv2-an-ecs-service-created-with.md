Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr__ecs__elbv2.elbv2-no-target-registration-without-service-lb-config
Issue-Key: ecr__ecs__elbv2.elbv2-no-target-registration-without-service-lb-config|run#14|Timeout|time.sleep(2)
Issue-Commit: 8ffd4f499c6d6f6b1d86ee4455070f9416ebee70

# ecr__ecs__elbv2: an ECS service created with a loadBalancers block never registers its running task in the target group, so the no-loadBalancers control check never reaches a verdict

> _Note: this issue was identified against the code base at commit `8ffd4f499c6d6f6b1d86ee4455070f9416ebee70`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTargetHealth

## Description
Cause 1 of 1: the positive control of the negative-delivery check never happens — an ECS service that DOES declare a loadBalancers block never gets its running task registered as a target, so the check cannot proceed to assert that the service WITHOUT that block registers nothing.
AWS: recorded behavior only (the ECS model describes the loadBalancers member, not the registration side effect). A service created with loadBalancers=[{targetGroupArn, containerName, containerPort}] and an awsvpc network configuration has each running task's ENI IP registered into that target group as the task reaches RUNNING: elbv2 DescribeTargetHealth(TargetGroupArn=<wired group>) then returns one TargetHealthDescription per running task, while a target group no service references returns an empty TargetHealthDescriptions list. The contrast is the whole point: registration is caused by the loadBalancers wiring.
Emulator: the check creates the network, cluster and task definition, an unreferenced target group, an ALB plus a wired target group, then two identical services — one without loadBalancers and one with — waits for both to report a running task, and waits for the wired target group to contain exactly one target. The wired group never reaches one target; the check is terminated by its 60 s budget with no verdict, so DescribeTargetHealth keeps answering an empty list for a group a running service is wired to.
Why: a client that deploys an ECS service behind an ALB gets no targets and therefore 503s from the load balancer, with nothing in the ECS or ELBv2 responses to explain it; and the negative case (a service with no loadBalancers must not register) can never be distinguished from the broken positive case, so wiring mistakes are undiagnosable.
Fix: the ECS service/task placement path that must register an awsvpc task's address into each configured target group (and deregister it when the task stops), and the ELBv2 target state DescribeTargetHealth reads. Nearest open issue: 20261109T013920Z-cloudwatch-ecs-elbv2-logs-an-ecs-service-with.md records the same registration gap for a different service group and does not carry this check's replay key.
Traces: ecr__ecs__elbv2.elbv2-no-target-registration-without-service-lb-config.

## Evidence
Check ecr__ecs__elbv2.elbv2-no-target-registration-without-service-lb-config [mismatch]:
  Error: Check timed out after 60.0s
  Body: two services on the same cluster/task definition/subnets — one with tg_arn=None, one with tg_arn=<wired group> — both waited to runningCount=1, then `_wait_target_count(ctx, tg_wired, 1, timeout_s=180)` (the positive control), then a 60 s window asserting `describe_target_health(TargetGroupArn=tg_unused)['TargetHealthDescriptions'] == []`.
  Expected: DescribeTargetHealth on the wired group lists exactly one target (the running task's awsvpc IP); the unreferenced group stays [].
  Actual: the wired group never lists a target, so the control wait never satisfies and the check produces no verdict.

## Steps to Reproduce
1. Create a VPC with two subnets and a security group; create an ECS cluster and register an awsvpc task definition.
2. Create target group A (never referenced) and an ALB with target group B in the same VPC.
3. CreateService S1 with no loadBalancers and CreateService S2 with loadBalancers=[{targetGroupArn: B, containerName, containerPort}], both desiredCount=1, awsvpc networkConfiguration in those subnets.
4. Wait until DescribeServices reports runningCount=1 for both, then poll elbv2 DescribeTargetHealth(TargetGroupArn=B): it never contains the running task's target (AWS registers exactly one).
