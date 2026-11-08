Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ecs__elbv2.alb-listener-forwards-request-to-ecs-task-ip autoscaling__ecs__elbv2.target-group-port-mismatch-container-port-no-healthy-targets
Issue-Key: autoscaling__ecs__elbv2.alb-listener-forwards-request-to-ecs-task-ip|_poll_until#4|AssertionError|raise AssertionError(f"timed out waiting for {desc}; last observed: {last!r}")
Issue-Key: autoscaling__ecs__elbv2.target-group-port-mismatch-container-port-no-healthy-targets|_poll_pair#17|AssertionError|assert mismatch_state != "healthy", "target with a mismatched health-check port must never report healthy"
Issue-Commit: 99cd58a9732d3abbd4ae376bbbd8ca320e363114

# autoscaling__ecs__elbv2: 2 net-new causes — an ALB answers 502 for a 'healthy' ECS awsvpc ip target, and a health check pointed at a port nothing listens on still reports healthy

> _Note: this issue was identified against the code base at commit `99cd58a9732d3abbd4ae376bbbd8ca320e363114`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTargetHealth

## Description
2 net-new root causes, grounded in 2 failing behavioral checks of this service. Related open issues that do NOT carry these replay keys and so do not cover them: 20261107T080429Z-autoscaling-ecs-elbv2-6-net-new-causes-asg.md (ASG/target-group convergence) and 20261105T050431Z-elbv2-describetargethealth-reports-a-constant-healthy-for-an.md (the same probe defect, but scoped to the plain `elbv2` service heading and to one elbv2-only check). A fixer closing this issue should treat cause 2 as the same underlying defect in the ELBv2 target-health probe.

Cause 1 of 2: an ALB listener whose default action forwards to a target group holding a RUNNING ECS awsvpc task's private IP — a target the emulator itself reports as `healthy` — answers HTTP 502 instead of the container's response.
Failing check ids: autoscaling__ecs__elbv2.alb-listener-forwards-request-to-ecs-task-ip.
AWS: recorded behavior only — the ELBv2 Smithy model describes the control plane (CreateListener `forward` action with a TargetGroupArn, `TargetHealth.State`) and says nothing about the data plane. The recording shows that once DescribeTargetHealth reports the registered ip target `healthy`, a GET to the load balancer's DNSName on the listener port is proxied to that address and port and the ALB returns the target's own status line and body (HTTP 200 for an nginx task serving `/`). A 502 is what AWS returns only when the target cannot be connected to or returns a malformed response, and in that case the target is not simultaneously reported `healthy`.
Emulator: the target-health poll succeeds (`['healthy']`), but every GET to the ALB's DNS name over a 120s window answers 502. Replay: `AssertionError: timed out waiting for ALB to forward request to the task and return 200; last observed: 502`.
Why: a client that reads target health as proof the path is wired sees a fully healthy ALB that cannot serve a single request. Every end-to-end assertion that an ECS service is reachable through its load balancer fails while the control plane claims success, so a deployment gate, smoke test or canary built on 'healthy targets + HTTP 200' can never pass, and the emulator's own status signal contradicts its data plane.
Fix: the ELBv2 data-plane listener/forward path — the component that accepts a connection on the load balancer endpoint, selects a target from the forward action's target group and proxies to the ECS awsvpc task's address/port (and the ECS side that must make that address actually serveable). The 502 must be reserved for a target that genuinely cannot answer, and must not coexist with State='healthy'.
Traces: autoscaling__ecs__elbv2.alb-listener-forwards-request-to-ecs-task-ip.

Cause 2 of 2: a target group whose HealthCheckPort is a port the container never listens on reports its target `healthy`; the health check is never performed.
Failing check ids: autoscaling__ecs__elbv2.target-group-port-mismatch-container-port-no-healthy-targets.
AWS: Smithy — `TargetHealth.State` is `TargetHealthStateEnum` {initial, healthy, unhealthy, unhealthy.draining, unused, draining, unavailable}; `TargetHealth.Reason` is `TargetHealthReasonEnum` and its documentation states that for State='unhealthy' the Reason is one of `Target.ResponseCodeMismatch`, `Target.Timeout`, `Target.FailedHealthChecks`, `Elb.InternalError`, that for State='healthy' "a reason code is not provided", and that `Description` carries additional detail for any non-healthy state. Beyond the model this is recorded behavior only: with two identical awsvpc tasks on container port 80, the target group whose health check uses the traffic port converges to `healthy`, while the sibling target group created with HealthCheckPort explicitly set to a port nothing listens on (8888 in this recording) never reports `healthy` — it settles on `unhealthy` with Reason `Target.Timeout` or `Target.FailedHealthChecks` and a non-empty Description. The state and reason names are the fixed required text; the ports, interval and threshold of any one run are request values filling the template.
Emulator: both target groups report State='healthy'. Replay: `AssertionError: target with a mismatched health-check port must never report healthy`. Because the verdict is a constant, the required Reason and Description for the failing half are never emitted either.
Why: health is the only signal a client has that a target is actually serving the configured health-check port. Reported as a constant, a misconfigured HealthCheckPort — one of the most common real ALB/ECS misconfigurations — is indistinguishable from a correct one: negative checks silently 'pass', health-based rollback never trips, and ECS/Auto Scaling replacement driven by ELB health never fires.
Fix: the ELBv2 target-health state machine and its probe (the target-health state the DescribeTargetHealth handler reads) must derive the terminal verdict from an actual check against the effective health-check port (HealthCheckPort when set, otherwise the traffic port) instead of defaulting to `healthy` after the settling span, and must emit the matching Reason/Description. A constant `unhealthy` is the same defect with another literal: the verdict must follow whether the configured port is served.
Traces: autoscaling__ecs__elbv2.target-group-port-mismatch-container-port-no-healthy-targets.

## Evidence
Failing check: [mismatch] autoscaling__ecs__elbv2.alb-listener-forwards-request-to-ecs-task-ip
  File "check_autoscaling__ecs__elbv2_alb_listener_forwards_request_to_ecs_task_ip.py", line 32, in run
    status = _poll_until(...)
  AssertionError: timed out waiting for ALB to forward request to the task and return 200; last observed: 502
Expected (AWS): after DescribeTargetHealth reports the ECS task's private IP as State='healthy', GET http://<lb-dns>/ returns 200 with the container's body.
Actual (emulator): 502 on every attempt for 120s, while target health reads ['healthy'].

Failing check: [mismatch] autoscaling__ecs__elbv2.target-group-port-mismatch-container-port-no-healthy-targets
  File "check_autoscaling__ecs__elbv2_target_group_port_mismatch_container_port_no_healthy_targets.py", line 58, in _poll_pair
    assert mismatch_state != "healthy", "target with a mismatched health-check port must never report healthy"
  AssertionError: target with a mismatched health-check port must never report healthy
Expected (AWS): the control target group (health check on the traffic port 80) converges to TargetHealth={State:'healthy'}; the sibling target group created with HealthCheckPort=<a port the container does not listen on> never reports 'healthy' and settles on {State:'unhealthy', Reason:'Target.Timeout'|'Target.FailedHealthChecks', Description:<non-empty>}.
Smithy: TargetHealthStateEnum = [initial, healthy, unhealthy, unhealthy.draining, unused, draining, unavailable]; TargetHealth.Reason doc — for 'unhealthy' the reason is Target.ResponseCodeMismatch | Target.Timeout | Target.FailedHealthChecks | Elb.InternalError; for 'healthy' no reason code is provided; Description is not provided for 'healthy'.
Actual (emulator): both target groups report {State:'healthy'} with no Reason and no Description.

## Steps to Reproduce
Replay the behavioral suite and re-run exactly the checks in trace_ids, or by hand:
Cause 1:
1. In the shared-network VPC, create a security group that admits itself (so the ALB's health checks and traffic reach the tasks), and pick two subnets in different AZs.
2. elbv2 CreateLoadBalancer (Type='application') on those subnets with that security group; wait for State.Code='active'.
3. elbv2 CreateTargetGroup TargetType='ip', Protocol='HTTP', Port=80, VpcId=<vpc>, HealthCheckProtocol='HTTP', HealthCheckPath='/'; CreateListener Protocol='HTTP' Port=80 with DefaultActions=[{Type:'forward', TargetGroupArn:<tg>}].
4. ecs RegisterTaskDefinition (awsvpc, Fargate, an image that serves HTTP on container port 80) and CreateService with desiredCount=1 and loadBalancers=[{targetGroupArn:<tg>, containerName:<c>, containerPort:80}]; wait runningCount=1.
5. Poll elbv2 DescribeTargetHealth until the task IP is State='healthy' (this passes today).
6. GET http://<lb-dns>/. EXPECT 200 with the container's body. OBSERVED: 502 for the full 120s window.
Cause 2:
1. Same VPC/security group/cluster; RegisterTaskDefinition with container port 80.
2. elbv2 CreateTargetGroup #1 with no HealthCheckPort override (traffic-port health check) and CreateTargetGroup #2 with HealthCheckPort='8888' (nothing listens there); attach each to its own listener on the same ALB (ports 80 and 8080).
3. CreateService for each target group with desiredCount=1; wait runningCount=1 for both.
4. Poll DescribeTargetHealth on both. EXPECT target group #1 -> State='healthy'; target group #2 -> never 'healthy', converging to State='unhealthy' with Reason 'Target.Timeout' or 'Target.FailedHealthChecks' and a non-empty Description. OBSERVED: both report State='healthy'.
