Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ecs__elbv2__logs.alb-health-check-reflects-ecs-task-health cloudwatch__ecs__elbv2__logs.alb-routes-traffic-only-to-healthy-ecs-targets cloudwatch__ecs__elbv2__logs.alb-security-group-mismatch-blocks-health-checks cloudwatch__ecs__elbv2__logs.alb-target-group-registers-ecs-tasks
Issue-Key: cloudwatch__ecs__elbv2__logs.alb-health-check-reflects-ecs-task-health|_poll_until#22|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s; last state={last!r}")
Issue-Key: cloudwatch__ecs__elbv2__logs.alb-routes-traffic-only-to-healthy-ecs-targets|run#25|AssertionError|assert by_id[bogus_ip]["state"] == "unhealthy", f"unexpected state for unreachable target: {by_id[bogus_ip]}"
Issue-Key: cloudwatch__ecs__elbv2__logs.alb-security-group-mismatch-blocks-health-checks|run#19|AssertionError|assert t["state"] != "healthy", f"target became healthy despite SG blocking ALB ingress: {t}"
Issue-Key: cloudwatch__ecs__elbv2__logs.alb-target-group-registers-ecs-tasks|_poll_until#21|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s; last state={last!r}")
Issue-Commit: b668b970d0d9c7dee5ca992802cd99dd932aec82

# cloudwatch__ecs__elbv2__logs: an ALB answers 502 for a healthy ECS target, target health is synthesized 'healthy' with no health check performed, and awslogs produces no log stream (3 causes)

> _Note: this issue was identified against the code base at commit `b668b970d0d9c7dee5ca992802cd99dd932aec82`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTargetHealth

## Description
THREE net-new behavioral root causes in the ECS-behind-ALB path. The replay report marks none of these four checks ALREADY FILED or OWNED. Nearest open issues, named for the fixer but carrying none of these check keys: 20261104T062603Z-cloudwatch-ecs-elbv2-logs-task-attachments-expose-no.md, 20261105T143800Z-cloudwatch-ecs-elbv2-logs-3-net-new-causes.md, 20261105T204413Z-cloudwatch-ecs-elbv2-logs-an-unreachable-ip-target.md, 20261107T183213Z-cloudwatch-ecs-elbv2-logs-an-ecs-service-s.md and 20261105T050431Z-elbv2-describetargethealth-reports-a-constant-healthy-for-an.md (that last one describes the same synthesized-healthy mechanism for a bare in-VPC ip target, under the elbv2 service, not this service's ECS path). In these four checks ECS task launch, private-IP attachment and target registration all SUCCEEDED, so what is left is the three causes below.

Cause 1 of 3: an ALB whose target group holds a healthy registered ECS task target answers HTTP 502 at its own DNS name forever, never the container's response.
AWS: not a modeled contract (the load-balancer data plane is not in the Smithy model), so recorded behavior only: once the target group reports State healthy for the task's private IP and the load balancer is active, a GET through the ALB's DNS name returns HTTP 200 and the body the container itself serves. AWS only answers 502 when the target's response is unusable, and this target's response is a normal nginx 200.
Emulator: every GET through the ALB's DNS name answers HTTP 502 with an empty body for the full 120s polling window, while target health reads healthy.
Why: the listener/forward data path is the whole point of putting an ECS service behind an ALB — a client cannot reach its own application through the emulated load balancer, so every end-to-end scenario that drives traffic through an ALB (smoke tests, blue/green cutovers, WAF/auth checks) fails even though the control plane reports a healthy, routable target.
Fix: the ELBv2 listener's forward action in the load-balancer data plane — proxy the request to the registered target and return its response.
Traces: cloudwatch__ecs__elbv2__logs.alb-health-check-reflects-ecs-task-health (target health reached healthy; the ALB GET stayed at (502, '')).

Cause 2 of 3: DescribeTargetHealth reports 'healthy' for targets that cannot possibly pass a health check — the configured health check is never performed.
AWS: Smithy: TargetHealth.State targets TargetHealthStateEnum (initial, healthy, unhealthy, unhealthy.draining, unused, draining, unavailable) and TargetHealth.Reason targets TargetHealthReasonEnum, whose doc states that an unhealthy state carries Target.Timeout when health-check requests time out or Target.FailedHealthChecks when the load balancer cannot establish a connection, and that a healthy state carries no Reason. Recorded behavior: an ip target that nothing answers on (an unused in-VPC address) and a target whose security group admits no ingress from the load balancer's security group both leave 'initial' for 'unhealthy' with one of those reason codes; they never read healthy.
Emulator: both read State 'healthy' with Reason None — the unreachable address {'id': <unused in-VPC ip>, 'port': 80, 'state': 'healthy', 'reason': None} and the SG-isolated task {'id': <task ip>, 'state': 'healthy', 'reason': None}. Health is synthesized from registration alone; neither connectivity nor the security-group path is evaluated.
Why: a client cannot tell a reachable target from an unreachable one, so the two things target groups exist for both break: unhealthy targets are never excluded from routing, and every negative test that proves a misconfiguration (no listener on the port, a missing security-group rule) passes as if the configuration were correct — exactly the class of bug users run these scenarios to catch.
Fix: the ELBv2 target-health evaluator — actually perform the target group's configured health check against the registered target (honouring the security-group path) and serialize unhealthy with the matching TargetHealthReasonEnum reason.
Traces: cloudwatch__ecs__elbv2__logs.alb-routes-traffic-only-to-healthy-ecs-targets, cloudwatch__ecs__elbv2__logs.alb-security-group-mismatch-blocks-health-checks.

Cause 3 of 3: a running Fargate task whose container definition uses the awslogs driver creates no CloudWatch Logs stream in its configured log group.
AWS: not modeled (the log-driver behaviour is ECS runtime behaviour), so recorded behavior only: a task whose logConfiguration is awslogs with an existing log group creates a log stream in that group and the container's stdout appears as events in it, so DescribeLogStreams returns at least one stream and GetLogEvents at least one event.
Emulator: DescribeLogStreams on the configured log group returns [] for the full 120s polling window, although the service reported runningCount 1 and the task's private IP was registered as a target.
Why: a RUNNING task with no log stream means the task is reported alive without actually running a container, so every log-based assertion — application diagnostics, metric filters over task logs, and the standard 'is the container really up' probe — has nothing to read.
Fix: the ECS task runtime's awslogs log-driver integration (create the stream from the task's logConfiguration and forward container output into CloudWatch Logs).
Traces: cloudwatch__ecs__elbv2__logs.alb-target-group-registers-ecs-tasks.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from this checkout; each line is the failing assertion the suite reported. None of the four checks is marked ALREADY FILED or OWNED.

Cause 1 — cloudwatch__ecs__elbv2__logs.alb-health-check-reflects-ecs-task-health: the check first polled target health to healthy (that assertion passed), then AssertionError: condition not met within 120.0s; last state=(502, '') for GET http://<alb dns>/ — expected HTTP 200 with the container's own body.

Cause 2 — cloudwatch__ecs__elbv2__logs.alb-routes-traffic-only-to-healthy-ecs-targets: AssertionError: unexpected state for unreachable target: {'id': '<unused in-VPC ip>', 'port': 80, 'state': 'healthy', 'reason': None} — expected state 'unhealthy'; cloudwatch__ecs__elbv2__logs.alb-security-group-mismatch-blocks-health-checks: AssertionError: target became healthy despite SG blocking ALB ingress: {'id': '<task ip>', 'state': 'healthy', 'reason': None} — expected a non-healthy state. (The literal addresses are per-run values.)

Cause 3 — cloudwatch__ecs__elbv2__logs.alb-target-group-registers-ecs-tasks: the private-IP target-registration assertions passed, then AssertionError: condition not met within 120.0s; last state=[] for logs.DescribeLogStreams(logGroupName=<task's awslogs group>) — expected at least one stream, with at least one event from the container.

Model citations: smithy_query shape=TargetHealth,TargetHealthStateEnum,TargetHealthReasonEnum on services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/smithy-model/com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10.json (healthy carries no Reason; unhealthy carries Target.Timeout / Target.FailedHealthChecks / Target.ResponseCodeMismatch). The ALB data plane and the awslogs driver are not modeled; both are cited as recorded behavior only.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from this checkout.
2. Replay the simparity-behavioral-aws-v1 behavioral suite (run_behavioral_suite).
3. Cause 1: replay cloudwatch__ecs__elbv2__logs.alb-health-check-reflects-ecs-task-health — create an ALB + target group + forward listener, run a Fargate nginx service with a loadBalancers attachment, wait for target state healthy, then GET the ALB's DNS name: it must answer HTTP 200 with the container's body, not 502.
4. Cause 2: replay cloudwatch__ecs__elbv2__logs.alb-routes-traffic-only-to-healthy-ecs-targets (register an unused in-VPC ip alongside the task target) and cloudwatch__ecs__elbv2__logs.alb-security-group-mismatch-blocks-health-checks (give the task a security group with no ingress from the ALB's security group) — DescribeTargetHealth must report those targets unhealthy with a TargetHealthReasonEnum reason, and the ALB must not route to them.
5. Cause 3: replay cloudwatch__ecs__elbv2__logs.alb-target-group-registers-ecs-tasks — after the service reaches runningCount 1, logs.DescribeLogStreams on the task's awslogs log group must return at least one stream and GetLogEvents at least one event.
6. Re-replay all four check ids listed in trace_ids after the fix; every one must pass.
