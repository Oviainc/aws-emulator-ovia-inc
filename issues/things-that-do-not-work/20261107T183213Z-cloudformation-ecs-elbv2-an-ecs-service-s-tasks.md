Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ecs__elbv2.ecs-service-target-group-arn-attribute-not-set-negative cloudformation__ecs__elbv2.ecs-task-network-mode-awsvpc-ip-target-registration-positive cloudformation__ecs__elbv2.elbv2-listener-rule-path-based-routing-to-multiple-ecs-services-positive cloudformation__ecs__elbv2.elbv2-security-group-mismatch-blocks-health-checks-negative
Issue-Key: cloudformation__ecs__elbv2.ecs-service-target-group-arn-attribute-not-set-negative|_poll_until#18|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s; last={last!r}")
Issue-Key: cloudformation__ecs__elbv2.ecs-task-network-mode-awsvpc-ip-target-registration-positive|_poll_until#13|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s; last={last!r}")
Issue-Key: cloudformation__ecs__elbv2.elbv2-listener-rule-path-based-routing-to-multiple-ecs-services-positive|_poll_until#18|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s; last={last!r}")
Issue-Key: cloudformation__ecs__elbv2.elbv2-security-group-mismatch-blocks-health-checks-negative|_poll_until#11|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s; last={last!r}")
Issue-Commit: e0dc03155a79e3340fd5a6f413ff0e6f3f485ded

# cloudformation__ecs__elbv2: an ECS service's tasks are never registered as ELBv2 targets — DescribeTargetHealth stays empty for every wired service, so registration, awsvpc IP targets, path-based routing and the unhealthy-target negative case all reach no verdict

> _Note: this issue was identified against the code base at commit `e0dc03155a79e3340fd5a6f413ff0e6f3f485ded`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTargetHealth

## Description
One net-new root cause (1 of 1) behind the four surviving cloudformation__ecs__elbv2 failures.

Related open filings that do NOT carry these replay keys: 20261104T045949Z-cloudformation-ecs-elbv2-cloudformation-provisioned-ecs-elbv2-resources.md, 20261105T132157Z-cloudformation-ecs-elbv2-an-ecs-service-with-a.md and 20261107T152217Z-cloudformation-ecs-elbv2-a-cross-vpc-elbv2-target.md own other checks in this service; the four checks below are listed unmarked in the replay report and survive.

Cause 1 of 1: an ECS service created with a loadBalancers entry never registers its running tasks into the named target group, so elbv2 target health is permanently empty.
AWS: recorded behavior, with the representation modeled. Smithy: elbv2 DescribeTargetHealth(TargetGroupArn required, Targets, Include) -> DescribeTargetHealthOutput{TargetHealthDescriptions}; TargetHealth.State is TargetHealthStateEnum [initial, healthy, unhealthy, unhealthy.draining, unused, draining, unavailable] with Reason from TargetHealthReasonEnum (the model documents Elb.RegistrationInProgress/Elb.InitialHealthChecking for 'initial', Target.Timeout/Target.FailedHealthChecks/Target.ResponseCodeMismatch for 'unhealthy'). Recorded AWS behavior: once an ECS service with loadBalancers=[{targetGroupArn, containerName, containerPort}] has a RUNNING task, ECS registers that task into the target group — the task's ENI private IP for networkMode 'awsvpc' against a TargetType='ip' group, the container instance for 'bridge' — and DescribeTargetHealth lists one TargetHealthDescription per task, moving initial -> healthy, or staying/becoming unhealthy (Target.Timeout / Target.FailedHealthChecks) when a security group blocks the load balancer's health-check traffic. Scale-down deregisters (state 'draining').
Emulator: DescribeTargetHealth answers an empty TargetHealthDescriptions list for the wired target group for the entire waiter window (90 s / 120 s in these checks), even though ECS reports the task RUNNING and describe_services reports the loadBalancers entry with the right targetGroupArn. Nothing is ever registered, in any of the four shapes exercised: the positive control of the negative check, awsvpc/IP target registration, the two services behind path-based listener rules, and the security-group-mismatch case that must produce an UNHEALTHY target rather than no target at all.
Why: the ECS-behind-an-ALB contract is unobservable. A deployment cannot wait for a task to become a healthy target before shifting traffic; scale-up/scale-down target counts cannot be read; and — worse for the negative cases — 'blocked by a security group' and 'never registered' are the same empty list, so a client cannot tell a misconfigured security group from a broken service. An empty list is also the legitimate answer for a service with no tasks, so callers hang instead of receiving an error.
Fix: the ECS service/task-placement path in the ecs service crate and its ELBv2 registration hop — when a task of a service with loadBalancers reaches RUNNING, register the correct target (task ENI IP + containerPort for awsvpc/ip groups, instance + host port otherwise) into elbv2 state, advance its TargetHealth through the group's health-check settings (including unhealthy with Target.Timeout when the task's security group does not admit the load balancer), and deregister on task stop/scale-in.
Traces: cloudformation__ecs__elbv2.ecs-service-target-group-arn-attribute-not-set-negative, cloudformation__ecs__elbv2.ecs-task-network-mode-awsvpc-ip-target-registration-positive, cloudformation__ecs__elbv2.elbv2-listener-rule-path-based-routing-to-multiple-ecs-services-positive, cloudformation__ecs__elbv2.elbv2-security-group-mismatch-blocks-health-checks-negative.

## Evidence
Replay of suite simparity-behavioral-aws-v1: 202/554 passed; cloudformation__ecs__elbv2 1/26 (4%), 25 failed. 21 are marked OWNED; the four below are not marked.

- [mismatch] .ecs-service-target-group-arn-attribute-not-set-negative — "File check_...py, line 62, in run / registered = _poll_until(...) ... AssertionError: condition not met within 120s; last=[]". The negative half passes (a service created WITHOUT loadBalancers shows loadBalancers == [] and registers nothing); the failure is the POSITIVE CONTROL at line 62-63, where the same task definition/cluster wired to the same target group (describe_services confirms exactly one loadBalancers entry with the expected targetGroupArn) must yield exactly 1 registered target. The emulator yields [].
- [mismatch] .ecs-task-network-mode-awsvpc-ip-target-registration-positive — "AssertionError: condition not met within 90s; last=[]" (awsvpc task expected to register its ENI private IP into a TargetType='ip' group).
- [mismatch] .elbv2-listener-rule-path-based-routing-to-multiple-ecs-services-positive — "AssertionError: condition not met within 120s; last=[]" (two services, two target groups behind path-based listener rules; no target ever registers, so no routing can be exercised).
- [mismatch] .elbv2-security-group-mismatch-blocks-health-checks-negative — "File check_...py, line 44, in run / unhealthy = _poll_until(...) ... AssertionError: condition not met within 120s; last=[]". Expected: a registered target reported UNHEALTHY (Target.Timeout / Target.FailedHealthChecks) because the task security group does not admit the ALB's health-check traffic. Actual: no target is listed at all, so the blocked-health-check conclusion cannot be drawn.

Smithy consulted: elbv2 DescribeTargetHealth -> TargetHealthDescriptions; TargetHealthStateEnum/TargetHealthReasonEnum as quoted above.

## Steps to Reproduce
1. Create a VPC with two subnets and a security group, an ECS cluster, a task execution role, a task definition (networkMode 'awsvpc', container 'web' port 80) and an elbv2 target group (TargetType='ip' for the awsvpc case) plus an ALB and a listener forwarding to it.
2. ecs.create_service(..., desiredCount=1, loadBalancers=[{'targetGroupArn': <tg>, 'containerName': 'web', 'containerPort': 80}]) and wait until list_tasks/describe_tasks reports the task RUNNING; ecs.describe_services confirms the loadBalancers entry.
3. Poll elbv2.describe_target_health(TargetGroupArn=<tg>): AWS lists one TargetHealthDescription for the task (the ENI private IP for awsvpc), initial -> healthy. The emulator returns TargetHealthDescriptions=[] for the whole window.
4. Repeat with a task security group that does NOT allow the ALB's health-check port: AWS still REGISTERS the target and reports it unhealthy (Target.Timeout/Target.FailedHealthChecks); the emulator reports no target.
5. Repeat with two services/target groups behind path-based listener rules.
6. Re-replay the four check ids in trace_ids.
