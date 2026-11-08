Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs__elbv2__logs.ecs-service-behind-alb-serves-http-request ecs__elbv2__logs.ecs-update-service-to-a-different-network-mode-rejected
Issue-Key: ecs__elbv2__logs.ecs-service-behind-alb-serves-http-request|_poll_until#25|Timeout|time.sleep(interval_s)
Issue-Key: ecs__elbv2__logs.ecs-update-service-to-a-different-network-mode-rejected|_poll_until#25|Timeout|time.sleep(interval_s)
Issue-Commit: 99cd58a9732d3abbd4ae376bbbd8ca320e363114

# ecs__elbv2__logs: an ALB-fronted awsvpc Fargate service never converges to a served HTTP response, so both checks burn the whole 180s budget without reaching a verdict

> _Note: this issue was identified against the code base at commit `99cd58a9732d3abbd4ae376bbbd8ca320e363114`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateService

## Description
1 net-new root cause, grounded in 2 failing behavioral checks of this service. Related open issues whose replay keys do NOT carry these checks, so they do not cover them: 20261104T062603Z-ecs-elbv2-logs-an-awsvpc-task-never-gets.md, 20261105T143800Z-ecs-elbv2-logs-2-net-new-causes-an.md and 20261107T222655Z-ecs-elbv2-logs-an-alb-attached-awsvpc-fargate.md. A fixer closing this issue should treat them as the same underlying defect in the ECS/ALB data path.

Cause 1 of 1: an ALB-attached awsvpc Fargate service never reaches the state in which the load balancer serves the container's response, so every poll loop built on it runs until the harness kills the check at 180s and no verdict — positive or negative — is ever produced.
Failing check ids: ecs__elbv2__logs.ecs-service-behind-alb-serves-http-request, ecs__elbv2__logs.ecs-update-service-to-a-different-network-mode-rejected.
AWS: recorded behavior only for the data plane; the refusal in the second check is modeled. The recording shows an ECS service with networkMode=awsvpc, launchType=FARGATE, a loadBalancers attachment and an awslogs log configuration converge within the budget: the task reaches RUNNING with a private IP, the IP is registered in the target group and turns healthy, a GET to the load balancer's DNSName returns 200 with the exact body the container wrote at startup, and the container's stdout appears in CloudWatch Logs. On that live service, ECS UpdateService onto a revision whose network mode differs (awsvpc -> bridge) is refused with the modeled InvalidParameterException (a load-balanced service cannot change network mode), the service keeps answering HTTP through its load balancer, and the same call succeeds once only the network mode is repaired. The error class and message wording are the requirement; the cluster, service and revision names in any one run are request values.
Emulator: neither check reaches any assertion — both are killed with `Check timed out after 180.0s`. The ALB-fronted service never produces the served response the first check waits for, and because the second check's precondition is that same live, HTTP-answering service, the InvalidParameterException refusal it exists to verify is never exercised at all.
Why: a client cannot run an ECS-behind-ALB workload on this emulator: the service reports progress but nothing is ever served, so every readiness wait hangs instead of failing fast with a diagnosis. Worse, the refusal half is masked: a mutation AWS rejects is never even attempted, so an invalid network-mode change could be silently accepted and no one would see it.
Fix: the ECS task/service convergence path for awsvpc Fargate tasks (private IP assignment, target registration, the container actually serving) together with the ELBv2 data-plane forward that must proxy to it; and, once reachable, the UpdateService validation that must refuse a network-mode change on a load-balanced service with InvalidParameterException.
Traces: ecs__elbv2__logs.ecs-service-behind-alb-serves-http-request, ecs__elbv2__logs.ecs-update-service-to-a-different-network-mode-rejected.

## Evidence
Failing check: [mismatch] ecs__elbv2__logs.ecs-service-behind-alb-serves-http-request
  Error: Check timed out after 180.0s
  Check intent: "A GET to the ALB's DNS name returns the exact body the ECS service task wrote at startup, which proves the service converged, its awsvpc IP was registered and passed health checks, and the listener forwarded; the container's stdout also reaches CloudWatch Logs."

Failing check: [mismatch] ecs__elbv2__logs.ecs-update-service-to-a-different-network-mode-rejected
  Error: Check timed out after 180.0s
  Check intent: "UpdateService refuses to move a live awsvpc, ALB-attached Fargate service onto a bridge-mode revision with InvalidParameterException, the service keeps answering HTTP through its load balancer, and the same call succeeds once only the network mode is repaired."

Expected (AWS): within the budget — task RUNNING with a private IP, IP registered and healthy in the target group, GET http://<lb-dns>/ -> 200 with the container's startup body, container stdout in the awslogs group; then UpdateService onto a bridge-mode revision refused with Error.Code='InvalidParameterException' while the service keeps serving, and accepted once the revision's network mode is awsvpc again.
Actual (emulator): no step of either scenario completes; the harness kills both checks at the 180s budget, so no status, error code or body is ever produced.

## Steps to Reproduce
Replay the behavioral suite and re-run exactly the checks in trace_ids, or by hand:
1. Pick the shared-network VPC and two subnets in different AZs. Create an ALB security group admitting TCP 80 from the client CIDRs, and a task security group admitting TCP 80 from the ALB security group. CreateLogGroup for the task's awslogs output and create the ECS task-execution role.
2. elbv2 CreateLoadBalancer (Type='application') on the two subnets with the ALB security group; wait for State.Code='active' and read DNSName. CreateTargetGroup TargetType='ip', Protocol='HTTP', Port=80, VpcId=<vpc>, HealthCheckPath='/'; CreateListener Protocol='HTTP' Port=80 forwarding to it.
3. ecs RegisterTaskDefinition with networkMode='awsvpc', FARGATE compatibility, the execution role, a container that writes a known marker body and serves it on port 80, and logConfiguration awslogs (group from step 1, a stream prefix).
4. ecs CreateService desiredCount=1, launchType='FARGATE', awsvpcConfiguration with the two subnets and the task security group, loadBalancers=[{targetGroupArn:<tg>, containerName:<c>, containerPort:80}].
5. Poll DescribeServices for runningCount=1, DescribeTasks for the task's private IP, and elbv2 DescribeTargetHealth for that IP reaching State='healthy'; then GET http://<lb-dns>/. EXPECT 200 with the marker body (and logs DescribeLogStreams/GetLogEvents showing the container's stdout). OBSERVED: the scenario never gets there — the check is killed at 180s.
6. With that service live and serving, RegisterTaskDefinition a sibling revision identical except networkMode='bridge' and call ecs UpdateService(service=<svc>, taskDefinition=<bridge revision>). EXPECT refusal with Error.Code='InvalidParameterException' naming the network-mode change, the service still answering HTTP through the ALB, and the same UpdateService succeeding once the revision's network mode is awsvpc again. OBSERVED: unreachable — the precondition in step 5 never holds, so this call is never made.
