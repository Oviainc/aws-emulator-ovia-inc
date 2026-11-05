Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2.health-check-transitions-to-unhealthy-with-reason
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elbv2.health-check-transitions-to-unhealthy-with-reason|run#25|AssertionError|assert final.get("State") == "unhealthy", (
Issue-Commit: 778841c19d7a134e4f15d723511a5118bd1b828c

# elbv2: DescribeTargetHealth reports a constant 'healthy' for an in-VPC ip target with nothing listening — the health check is never performed (1 cause)

> _Note: this issue was identified against the code base at commit `778841c19d7a134e4f15d723511a5118bd1b828c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTargetHealth

## Description
One net-new root cause, grounded in one failing behavioral check.

Related open issue: 20261104T094656Z-elbv2-seven-target-health-routing-and-data-plane-causes-still-open.md describes an adjacent symptom under its "Cause 2 of 10 (the PROBE half)" and lists this check's name in its prose, but its replay keys cover only `elbv2.target-health-check-state-transition-unhealthy-to-healthy`, so the replay key for `elbv2.health-check-transitions-to-unhealthy-with-reason` is carried by no open issue and this check is reported as unowned. This issue carries it. It does not supersede that issue; a fixer closing both should treat them as the same underlying defect in the target-health probe.

Cause 1 of 1: an ip target that cannot answer a health check still converges to State='healthy'.
Failing check ids: elbv2.health-check-transitions-to-unhealthy-with-reason.

AWS: Smithy — `TargetHealth.State` is `TargetHealthStateEnum` {initial, healthy, unhealthy, unhealthy.draining, unused, draining, unavailable}; `TargetHealth.Reason` is `TargetHealthReasonEnum`, and the member documentation states that for State='unhealthy' the Reason is one of `Target.ResponseCodeMismatch`, `Target.Timeout`, `Target.FailedHealthChecks`, `Elb.InternalError`, that for State='healthy' "a reason code is not provided", and that `TargetHealth.Description` carries additional detail for any non-healthy state. Beyond the model this is recorded behavior only: once an `ip` target group with `HealthCheckProtocol=HTTP`, `HealthCheckPath=/`, `Matcher.HttpCode=200`, `HealthCheckIntervalSeconds=5` and `UnhealthyThresholdCount=2` is attached to a listener, real ELBv2 actually probes the registered address; an address inside the VPC with no listener on the health-check port is driven from `unused` (Reason `Target.NotInUse`) through `initial` (Reason `Elb.RegistrationInProgress` / `Elb.InitialHealthChecking`) to `unhealthy` with Reason `Target.Timeout` or `Target.FailedHealthChecks` and a non-empty `Description`, and never sooner than UnhealthyThresholdCount × HealthCheckIntervalSeconds after attachment. The state and reason names are the fixed, required text; the target address, port, interval and threshold in any one run are request values that fill the template.

Emulator: the `unused` and `initial` phases are produced correctly (the check's steps (a) and (b) pass, including Reason `Target.NotInUse` and `Elb.RegistrationInProgress`), but after the settling span every registered target is reported as `{'State': 'healthy'}` regardless of what is behind the address. The replay asserts at step (c) with `AssertionError: an in-VPC IP with nothing listening reported {'State': 'healthy'}; expected 'unhealthy'`. Because the terminal verdict is a constant, the required Reason (`Target.Timeout` / `Target.FailedHealthChecks`) and `Description` are never emitted either, and the threshold × interval timing assertion in step (d) and the `InvalidConfigurationRequest` assertion in step (e) are never reached.

Why: a client relying on AWS's behavior reads health as evidence that the target is reachable and serving the matcher code. With health reported as a constant, every negative health scenario silently "passes": an ALB appears to have healthy targets it can never route to, a deployment gate or health-based rollback that waits for `unhealthy` never trips, autoscaling/ECS logic that replaces targets on `unhealthy` never fires, and operators cannot distinguish a working target group from a completely broken one.

Fix: the ELBv2 target-health state machine and its probe — the component that derives `TargetHealth` for a registered target (the target-health state module read by the DescribeTargetHealth handler) must derive the terminal verdict from an actual check of the registered target rather than defaulting past the settling span to `healthy`, and must emit the matching Reason and Description. Note the constraint recorded in the related issue: a raw connection to an arbitrary registered address is egress this project forbids, and a constant `unhealthy` is the same defect with another literal; the verdict must come from whether this process can actually serve the registered address/port (for example a target the emulator itself hosts, versus an address nothing in the emulator serves, which must become `unhealthy` with `Target.Timeout`/`Target.FailedHealthChecks`).

Traces: elbv2.health-check-transitions-to-unhealthy-with-reason.

## Evidence
Failing check: [mismatch] elbv2.health-check-transitions-to-unhealthy-with-reason

Replay output:
  File "check_elbv2_health_check_transitions_to_unhealthy_with_reason.py", line 272, in run
    assert final.get("State") == "unhealthy", (
  AssertionError: an in-VPC IP with nothing listening reported {'State': 'healthy'}; expected 'unhealthy'

Expected (AWS): DescribeTargetHealth for the registered ip target converges to
  TargetHealthDescriptions[0].TargetHealth = {State: 'unhealthy', Reason: 'Target.Timeout' | 'Target.FailedHealthChecks', Description: <non-empty>}
no sooner than UnhealthyThresholdCount (2) × HealthCheckIntervalSeconds (5) = 10s after the target group is attached to the listener.
Smithy: TargetHealthStateEnum = [initial, healthy, unhealthy, unhealthy.draining, unused, draining, unavailable]; TargetHealth.Reason doc: for State='unhealthy' the reason is Target.ResponseCodeMismatch | Target.Timeout | Target.FailedHealthChecks | Elb.InternalError; for State='healthy' no reason code is provided.

Actual (emulator): TargetHealth = {'State': 'healthy'} — no Reason, no Description — for an address inside the VPC with nothing listening on the health-check port.

Phases that did pass, locating the defect in the probe rather than the state machine:
  (a) pre-attachment: State='unused', Reason='Target.NotInUse'  — correct
  (b) post-attachment first read: State='initial', Reason='Elb.RegistrationInProgress' — correct
  (c) terminal verdict: 'healthy' instead of 'unhealthy' — DIVERGENCE

## Steps to Reproduce
Replay the behavioral suite and re-run exactly the check in trace_ids (elbv2.health-check-transitions-to-unhealthy-with-reason), or by hand:
1. Pick the shared-network VPC and two available AZ subnets (ec2 DescribeVpcs / DescribeSubnets).
2. elbv2 CreateTargetGroup with TargetType='ip', Protocol='HTTP', Port=80, VpcId=<vpc>, HealthCheckProtocol='HTTP', HealthCheckPort='80', HealthCheckPath='/', HealthCheckIntervalSeconds=5, HealthCheckTimeoutSeconds=2, HealthyThresholdCount=2, UnhealthyThresholdCount=2, Matcher={HttpCode:'200'}.
3. elbv2 RegisterTargets with an address inside the first subnet's CIDR that nothing listens on (e.g. broadcast_address - 3), Port=80.
4. Poll DescribeTargetHealth: expect State='unused', Reason='Target.NotInUse' (this passes today).
5. CreateLoadBalancer (Type='application', Scheme='internal', the two subnets), wait for State.Code='active', then CreateListener Protocol='HTTP' Port=80 with DefaultActions=[{Type:'forward', TargetGroupArn:<tg>}].
6. Poll DescribeTargetHealth: expect State='initial' with Reason in {Elb.RegistrationInProgress, Elb.InitialHealthChecking} (this passes today).
7. Keep polling DescribeTargetHealth for a terminal verdict. EXPECT State='unhealthy' with Reason in {Target.Timeout, Target.FailedHealthChecks} and a non-empty Description, reached no sooner than 10s after step 5. OBSERVED: State='healthy' with no Reason and no Description.
8. (Blocked by step 7, assert once it is fixed) ModifyTargetGroup with HealthCheckEnabled=false on this ALB-attached ip target group must be refused with code 'InvalidConfigurationRequest' and message "Health check enabled must be true for target groups with target type 'ip'", must leave HealthCheckEnabled=true, and must not perturb the target's health state.
