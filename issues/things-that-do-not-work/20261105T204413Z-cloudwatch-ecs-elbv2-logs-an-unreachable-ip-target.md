Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ecs__elbv2__logs.alb-routes-traffic-only-to-healthy-ecs-targets
Issue-Key: cloudwatch__ecs__elbv2__logs.alb-routes-traffic-only-to-healthy-ecs-targets|_poll_until#22|Timeout|last = probe()
Issue-Commit: e730f41abf8cc315135e4118582590f4999ad0e7

# cloudwatch__ecs__elbv2__logs: an unreachable ip target never leaves `initial` for `unhealthy` and the ALB never serves the healthy ECS task, so health-based routing exclusion is unobservable

> _Note: this issue was identified against the code base at commit `e730f41abf8cc315135e4118582590f4999ad0e7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTargetHealth

## Description
One net-new root cause, grounded in one failing behavioral check that the replay report listed WITHOUT an ALREADY FILED or OWNED marker. The open issue 20261105T143800Z-cloudwatch-ecs-elbv2-logs-3-net-new-causes.md names this check in its trace list but does not carry the replay key it now fails on (its key for this check is `_poll_until#22|Timeout|...`), so this failure is not covered by it; it is filed here and that issue is named for cross-reference. 20261104T062603Z-cloudwatch-ecs-elbv2-logs-task-attachments-expose-no.md covers adjacent task-attachment causes under the same heading and likewise does not carry this key.

Cause 1 of 1: a target group holding one ECS-task target and one deliberately unreachable `ip` target never resolves either target's health, and the listener never serves a request, so the ALB's core guarantee — route only to healthy targets — produces no verdict.
AWS: Smithy models the surfaces the check reads — elasticloadbalancingv2#RegisterTargets (Targets[].Id/Port), elasticloadbalancingv2#DescribeTargetHealth -> TargetHealthDescriptions[].Target.Id with TargetHealth.State from elasticloadbalancingv2#TargetHealthStateEnum = [initial, healthy, unhealthy, unhealthy.draining, unused, draining, unavailable], and TargetHealth.Reason, whose documentation states the reason codes for each state: `initial` carries Elb.RegistrationInProgress or Elb.InitialHealthChecking, and `unhealthy` carries Target.Timeout when "the health check requests timed out" or Target.FailedHealthChecks. So on AWS a freshly registered target starts `initial` and must LEAVE it: the ECS service's task ENI private IPv4 becomes `healthy`, while an in-CIDR IP attached to no ENI becomes `unhealthy` (Reason Target.Timeout / Target.FailedHealthChecks). Recorded behavior only for the data plane: once the load balancer is active, an HTTP GET to its DNS name returns 200 with the healthy task's body, and every repeat request also returns 200 with that body — the unhealthy target is excluded from rotation entirely, never producing a 5xx or a connection failure. The body, the IPs and the DNS name are values from the request and state; the requirements are the status 200, the state values `healthy`/`unhealthy`, and that no request is routed to the unhealthy target.
Emulator: the check is terminated by the harness with `Error: Check timed out after 60.0s` and no response to compare. None of the polled surfaces settle: the ECS service never reports a running task to read an ENI IP from, DescribeTargetHealth never reports two non-`initial` descriptions for the two registered targets, and the HTTP probe through the listener never returns a response. The 60 s figure is the suite's budget, not a requirement — the requirement is that target health reaches a terminal state and the listener answers.
Why: a client cannot tell a broken target from a serving one, and cannot verify that traffic is kept off a failing task. Health-check-driven rollouts, canaries and alarm thresholds built on DescribeTargetHealth read an unresolved target group forever, and a load test against the listener gets no response at all where AWS returns 200 from the healthy task.
Fix: the ELBv2 target-group health-check evaluator (it must advance a registered target out of `initial` to `healthy` or `unhealthy` with the matching TargetHealth.Reason), the ECS -> ELBv2 registration of a service task's ENI private IPv4 that it evaluates, and the listener's forward action, which must select only healthy targets.
Traces: cloudwatch__ecs__elbv2__logs.alb-routes-traffic-only-to-healthy-ecs-targets.

## Evidence
Failing check: cloudwatch__ecs__elbv2__logs.alb-routes-traffic-only-to-healthy-ecs-targets [mismatch]
  Error: Check timed out after 60.0s

The check had already (per its replayed source): read the shared VPC and two subnets, created an ALB security group with ingress on 80 and a task security group with ingress on 80 from the ALB SG, created the ALB, an `ip` target group on port 80 with health-check path "/", a forwarding listener, an execution role, a CloudWatch Logs group, an ECS cluster and an awsvpc/FARGATE task definition whose nginx container writes a fixed marker body; created the service with desiredCount=1 and that target group; then registered a second target: an address inside the subnet CIDR attached to no ENI.

Expected (AWS): DescribeTargetHealth reports exactly two descriptions, neither in state `initial` — the task's ENI private IPv4 `healthy`, the unattached address `unhealthy` (TargetHealth.Reason Target.Timeout or Target.FailedHealthChecks). Then, with the load balancer active, an HTTP GET to its DNS name returns status 200 and the marker body, and six further GETs each return 200 and the same body.
Actual (emulator): no response at all — the check is cut off at the suite budget, so target health never leaves its unresolved value for either target and the listener never answers a request.

Smithy consulted: elasticloadbalancingv2#RegisterTargets; elasticloadbalancingv2#DescribeTargetHealth -> TargetHealthDescriptions[].Target.Id / TargetHealth.State / TargetHealth.Reason; elasticloadbalancingv2#TargetHealthStateEnum = [initial, healthy, unhealthy, unhealthy.draining, unused, draining, unavailable]; TargetHealth.Reason docs: `initial` => Elb.RegistrationInProgress | Elb.InitialHealthChecking, `unhealthy` => Target.ResponseCodeMismatch | Target.Timeout | Target.FailedHealthChecks | Elb.InternalError. The 200/body routing behaviour and the convergence timing are recorded behavior only.

## Steps to Reproduce
1. In a VPC with two subnets, create an ALB security group (ingress tcp/80) and a task security group (ingress tcp/80 from the ALB SG).
2. CreateLoadBalancer (application) in those subnets; CreateTargetGroup with TargetType 'ip', port 80, health-check path '/'; CreateListener on 80 forwarding to it.
3. Create an ECS cluster, an execution role and a CloudWatch Logs group; RegisterTaskDefinition (awsvpc/FARGATE) whose container writes a fixed marker body and serves it on port 80.
4. CreateService with desiredCount=1 and loadBalancers bound to that target group; wait for runningCount 1 and read the task's ENI private IPv4.
5. RegisterTargets with a second Id: an address inside the subnet CIDR that is attached to no ENI, port 80.
6. Poll DescribeTargetHealth: expect two descriptions with no state equal to 'initial' — the task IP 'healthy' and the unattached address 'unhealthy'. Observe that the states never resolve.
7. HTTP GET the load balancer DNS name repeatedly: expect 200 and the marker body every time. Observe no response; the check is killed at the 60 s budget.
8. Re-replay: cloudwatch__ecs__elbv2__logs.alb-routes-traffic-only-to-healthy-ecs-targets.
