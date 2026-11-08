Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2.e2e-alb-traffic-lifecycle-flow
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elbv2.e2e-alb-traffic-lifecycle-flow|run#56|AssertionError|assert reached is True, "ALB never forwarded a successful HTTP response from the registered target"
Issue-Commit: 99cd58a9732d3abbd4ae376bbbd8ca320e363114

# elbv2: an ALB never forwards a successful HTTP response from an EC2 instance target it reports 'healthy', so the weighted-forward data path is unusable

> _Note: this issue was identified against the code base at commit `99cd58a9732d3abbd4ae376bbbd8ca320e363114`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateListener

## Description
1 net-new root cause, grounded in 1 failing behavioral check. Related open issue that does not carry this check's replay key and so does not cover it: 20261104T094656Z-elbv2-seven-target-health-routing-and-data-plane-causes-still-open.md (adjacent target-health/data-plane causes); see also 20261107T080944Z-elbv2-2-net-new-data-plane-root-causes.md. A fixer closing this issue should treat them as the same underlying defect in the ELBv2 data plane.

Cause 1 of 1: with an `instance`-type target group holding a running EC2 instance that serves HTTP on port 80, a listener and a priority-10 path-pattern rule both forwarding 100% weight to that target group, and DescribeTargetHealth reporting State='healthy', no request to the load balancer's DNSName ever returns the target's response.
Failing check ids: elbv2.e2e-alb-traffic-lifecycle-flow.
AWS: recorded behavior only — the ELBv2 Smithy model describes the control plane (CreateListener/CreateRule `forward` with `ForwardConfig.TargetGroups[].Weight`, `TargetHealth.State` in TargetHealthStateEnum) and nothing about the data plane. The recording shows that after the load balancer reaches State.Code='active', the rule is readable from DescribeRules with its priority and path-pattern condition, and the instance target reports State='healthy', a GET to `http://<DNSName>/` is proxied to the instance on the registered port and the ALB returns the target's own status line and body (HTTP 200 carrying the bytes the instance serves).
Emulator: the control plane all passes — the load balancer becomes active, DescribeRules shows Priority '10' and Conditions[0].Values ['/*'], the target converges to State='healthy' — but the request never succeeds over a 120s window. Replay: `AssertionError: ALB never forwarded a successful HTTP response from the registered target`.
Why: a client that provisions an ALB, a target group, a listener and a weighted forward rule, and then waits for healthy targets, has every control-plane signal that traffic will flow — and gets nothing. Any end-to-end flow (smoke test, blue/green weight shift, canary, deployment gate) is unverifiable, and the emulator's own health verdict contradicts its data plane, which is worse than an honest failure because it points debugging at the wrong layer.
Fix: the ELBv2 data-plane listener path — the component that accepts a connection on the load balancer endpoint, evaluates the listener's rules, selects a target group by weight and proxies the request to the registered instance target's address and port — and whatever makes an EC2 instance target's served port reachable from it. The health verdict and the forward path must agree.
Traces: elbv2.e2e-alb-traffic-lifecycle-flow.

## Evidence
Failing check: [mismatch] elbv2.e2e-alb-traffic-lifecycle-flow
  File "check_elbv2_e2e_alb_traffic_lifecycle_flow.py", line 221, in run
    assert reached is True, "ALB never forwarded a successful HTTP response from the registered target"
  AssertionError: ALB never forwarded a successful HTTP response from the registered target

Phases that DID pass, locating the defect in the data plane rather than the control plane:
  - CreateLoadBalancer (application, internet-facing, 2 subnets) reached State.Code='active'
  - CreateListener HTTP:80 with ForwardConfig TargetGroups[{tg, Weight:100}] accepted
  - CreateRule Priority=10, Conditions=[path-pattern '/*'] accepted and read back by DescribeRules with Priority '10' and Values ['/*']
  - DescribeTargetHealth converged to TargetHealth.State='healthy' for the registered instance target (Id=<instance-id>, Port=80)

Expected (AWS): GET http://<lb-dns>/ returns HTTP 200 whose body contains the bytes the instance serves from its user-data-installed web server.
Actual (emulator): the GET never succeeds within 120s (no 200 with the target's body), while the same target is reported 'healthy'.

## Steps to Reproduce
Replay the behavioral suite and re-run exactly the check in trace_ids (elbv2.e2e-alb-traffic-lifecycle-flow), or by hand:
1. ec2 CreateVpc + two subnets in different AZs, internet gateway/route, and a security group admitting TCP 80 from the test client CIDRs.
2. ec2 RunInstances with user data that installs and starts a web server returning a known marker body on port 80; wait for the instance to be running.
3. elbv2 CreateTargetGroup TargetType='instance', Protocol='HTTP', Port=80, VpcId=<vpc>, HealthCheckProtocol='HTTP', HealthCheckPath='/', HealthCheckIntervalSeconds=10, HealthyThresholdCount=2; ModifyTargetGroupAttributes deregistration_delay.timeout_seconds=10; RegisterTargets with {Id:<instance-id>, Port:80}.
4. elbv2 CreateLoadBalancer (Type='application', Scheme='internet-facing', both subnets, that security group); poll DescribeLoadBalancers until State.Code='active' and read DNSName.
5. CreateListener Protocol='HTTP' Port=80 with DefaultActions=[{Type:'forward', ForwardConfig:{TargetGroups:[{TargetGroupArn:<tg>, Weight:100}]}}]; CreateRule Priority=10, Conditions=[{Field:'path-pattern', Values:['/*']}], same forward action; confirm DescribeRules returns it.
6. Poll DescribeTargetHealth until State='healthy' (this passes today).
7. GET http://<DNSName>/. EXPECT HTTP 200 whose body contains the marker the instance serves. OBSERVED: no successful response for the full 120s window.
