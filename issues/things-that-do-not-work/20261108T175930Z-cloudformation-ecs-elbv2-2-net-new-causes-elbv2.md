Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ecs__elbv2.cli-elbv2-create-target-group-wrong-vpc-negative cloudformation__ecs__elbv2.elbv2-target-health-check-path-affects-ecs-target-status-positive cloudformation__ecs__elbv2.elbv2-listener-rule-path-based-routing-to-multiple-ecs-services-positive
Issue-Key: cloudformation__ecs__elbv2.cli-elbv2-create-target-group-wrong-vpc-negative|run#30|AssertionError|assert "healthy" not in states, (
Issue-Key: cloudformation__ecs__elbv2.elbv2-target-health-check-path-affects-ecs-target-status-positive|_poll_until#13|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s; last={last!r}")
Issue-Key: cloudformation__ecs__elbv2.elbv2-listener-rule-path-based-routing-to-multiple-ecs-services-positive|_poll_http_body#20|Timeout|time.sleep(interval_s)
Issue-Commit: c0b127c099d8543a6fa74c4b10d6d49cf92e859e

# cloudformation__ecs__elbv2: 2 net-new causes — ELBv2 target health is synthesized 'healthy' with no health check performed, and the ALB path-routing scenario never converges

> _Note: this issue was identified against the code base at commit `c0b127c099d8543a6fa74c4b10d6d49cf92e859e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTargetHealth

## Description
This issue carries 2 net-new root causes in the `cloudformation__ecs__elbv2` suite section. Each must be closed before submitting.

Cause 1 of 2: a registered target is reported `healthy` unconditionally — the emulator never evaluates the target group's health check, so neither an unreachable target nor a failing health-check path can ever produce an unhealthy state.
Failing check ids for this cause: cloudformation__ecs__elbv2.cli-elbv2-create-target-group-wrong-vpc-negative, cloudformation__ecs__elbv2.elbv2-target-health-check-path-affects-ecs-target-status-positive.
AWS: Smithy (services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/smithy-model/com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10.json): TargetHealth.State is TargetHealthStateEnum (initial | healthy | unhealthy | unhealthy.draining | unused | draining | unavailable) and TargetHealth.Reason is TargetHealthReasonEnum, documented per state — "If the target state is unhealthy, the reason code can be one of ... Target.ResponseCodeMismatch - The health checks did not return an expected HTTP code ... Target.Timeout ... Target.FailedHealthChecks" — plus Target.IpUnusable for an IP the load balancer cannot use, and "If the target state is healthy, a reason code is not provided". The health state is therefore DERIVED from the configured health check (protocol, path, interval, thresholds) against the target's address. Recorded behavior: (a) an ip-target-type target group in a VPC the target's IP does not belong to never reports that target healthy; (b) ModifyTargetGroup(HealthCheckPath='/this-path-does-not-exist') against a container that answers 404 there flips the target to State 'unhealthy' with Reason 'Target.ResponseCodeMismatch' within the configured interval x unhealthy threshold.
Emulator: DescribeTargetHealth reports `{'TargetHealth': {'State': 'healthy'}}` in both cases and keeps reporting it — for the wrong-VPC target group over a 240 s window, and for 90 s after the health-check path is pointed at a 404. No Reason and no Description are ever emitted, and no state other than 'healthy' is ever observed.
Why: target health is the primary signal every ALB-fronted deployment depends on. Because it is synthesized rather than derived, a client sees a target as serving traffic when it cannot be reached at all (wrong VPC / unroutable IP) or is answering the health check with an error — so health-check configuration, cross-VPC misconfiguration, blue/green cutovers and premature-deregistration guards are all unverifiable and all read the opposite of AWS.
Fix: the ELBv2 target-health derivation behind DescribeTargetHealth (the elasticloadbalancingv2 crate's target-health state, and ModifyTargetGroup's effect on it) — evaluate the group's health-check settings against the registered target's reachability and response instead of returning a constant 'healthy', and populate TargetHealth.Reason (Target.ResponseCodeMismatch, Target.Timeout, Target.FailedHealthChecks, Target.IpUnusable) with no Reason on 'healthy'.

Cause 2 of 2: the ALB path-based-routing scenario over two ECS services never settles, so it produces no verdict at all.
Failing check ids for this cause: cloudformation__ecs__elbv2.elbv2-listener-rule-path-based-routing-to-multiple-ecs-services-positive.
AWS: recorded behavior only (no cross-service coupling is modelled). Against AWS the scenario converges inside its own per-step budgets: two ECS services created with loadBalancers wired to their own target groups reach steady state, each group reports exactly one healthy target within 120 s, and HTTP GETs on the ALB's DNS name return svc-a's body under /svc-a*, svc-b's body under /svc-b*, no cross-leak, and the listener's default fixed-response (404, body containing 'no-match') for an unmatched path.
Emulator: the check is killed by the harness with `Check timed out after 180.0s` without reaching any assertion — the polled conditions (service steady state, then one healthy target per group, then a usable HTTP response from the ALB DNS name) never converge. Hypothesis: the same ECS/ALB data-plane gap as cause 1 plus the absence of an ALB listener data plane; the recording says only that the scenario converges and the emulator never does.
Why: path-based routing — the main reason to put an ALB in front of several ECS services — cannot be exercised or observed at all, and the check yields no signal rather than a wrong one.
Fix: the ECS service/task stabilization path together with the ELBv2 target registration/health derivation of cause 1 and the ALB listener-rule request routing, so the scenario reaches an observable terminal state instead of hanging.

Related open issue, named for the fixer but NOT covering these (it does not carry their current replay keys): 20261104T045949Z-cloudformation-ecs-elbv2-cloudformation-provisioned-ecs-elbv2-resources.md lists all three check ids above, but its causes 5/6/8 are that DescribeTargetHealth returned ZERO target descriptions, that the first poll was already healthy, and that nine checks timed out at 60 s. Targets now DO register and the budgets have changed; what remains is the synthesized-healthy derivation above (now observable as a target that stays healthy when it must go unhealthy) and this one scenario still failing to converge at 180 s. Fixing that issue's causes 5/6 should help, but these ids must be re-replayed.

Traces: the three check ids in trace_ids, grouped per cause above.

## Evidence
Cause 1 — cloudformation__ecs__elbv2.cli-elbv2-create-target-group-wrong-vpc-negative:
`AssertionError: target unexpectedly went healthy in the wrong-VPC target group: ['healthy']`
Expected (AWS): an ip-target-type target group created in VPC B (with its own internal ALB and listener in VPC B) that an ECS service in VPC A registers its task IP into never reports that target 'healthy' over a 240 s window — the IP is not usable from that load balancer.
Actual (emulator): DescribeTargetHealth reports State 'healthy'.

Cause 1 — cloudformation__ecs__elbv2.elbv2-target-health-check-path-affects-ecs-target-status-positive:
`AssertionError: condition not met within 90s; last={'Target': {'Id': '<task ip>', 'Port': 80, 'AvailabilityZone': '<az>'}, 'HealthCheckPort': '80', 'TargetHealth': {'State': 'healthy'}}`
Expected (AWS): after elbv2 ModifyTargetGroup(TargetGroupArn, HealthCheckPath='/this-path-does-not-exist') against an httpd container that 404s there, DescribeTargetHealth reports State 'unhealthy' with Reason 'Target.ResponseCodeMismatch'.
Actual (emulator): State stays 'healthy' for the whole 90 s poll; no Reason is ever present. (The preceding poll for 'healthy' passes, so registration itself is working — only the derivation is wrong.) The target IP and AZ in the line are per-run values.

Cause 2 — cloudformation__ecs__elbv2.elbv2-listener-rule-path-based-routing-to-multiple-ecs-services-positive:
`Error: Check timed out after 180.0s`
Expected (AWS): both services stabilize, each target group reports exactly 1 target with State 'healthy' within 120 s, then GET http://<alb dns>/svc-a/ returns svc-a's body ('It works'), GET /svc-b/ returns svc-b's body ('sim2real-hello-from-b'), neither body leaks into the other, and GET /no-such-path returns 404 with the listener default action's 'no-match' body.
Actual (emulator): the harness kills the check at 180 s with no assertion reached.

## Steps to Reproduce
Cause 1 (wrong VPC): create VPC A with subnets/IGW, an ECS cluster and a FARGATE/awsvpc task definition; create an ISOLATED VPC B with its own subnets and default SG; in VPC B create an internal ALB plus (via `aws elbv2 create-target-group --vpc-id <vpc B> --target-type ip --health-check-protocol HTTP --health-check-path / --health-check-interval-seconds 10 --healthy-threshold-count 2 --unhealthy-threshold-count 2`) a target group and attach a listener; then ecs CreateService in VPC A's subnets with loadBalancers=[{targetGroupArn: <VPC B group>, containerName, containerPort}]. Poll elbv2 DescribeTargetHealth on the VPC-B group: no description may ever report State 'healthy'.

Cause 1 (health-check path): create one VPC/cluster/httpd task definition, an ALB, a target group with HealthCheckPath '/' and interval 5, a listener, and an ECS service with desiredCount 1 wired to that group. Poll DescribeTargetHealth to 'healthy', then elbv2 ModifyTargetGroup(HealthCheckPath='/this-path-does-not-exist') and poll again: expect State 'unhealthy' with Reason 'Target.ResponseCodeMismatch'.

Cause 2: create one VPC/cluster, two task definitions each serving content under /svc-a and /svc-b, an ALB with a listener whose default action is a 404 fixed response containing 'no-match' and two path-pattern rules (/svc-a*, /svc-b*) to two target groups, and two ECS services wired one per group. Wait for both services stable, both groups to report one healthy target, then GET /svc-a/, /svc-b/ and /no-such-path on the ALB DNS name.

Or replay the three check ids in trace_ids.
