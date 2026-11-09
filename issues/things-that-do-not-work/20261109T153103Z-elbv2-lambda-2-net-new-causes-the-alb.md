Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2__lambda.malformed-lambda-target-response-becomes-a-502-after-the-invocation elbv2__lambda.xsvc-elbv2-3-lambda-invoke-permission-removed-is-denied-with-502-until-regranted elbv2__lambda.xsvc-elbv2-3-lambda-health-check-500-and-raise-mark-only-those-targets-unhealthy
Issue-Key: elbv2__lambda.malformed-lambda-target-response-becomes-a-502-after-the-invocation|_http#21|PublishedUrlRejected|_url, _route = ctx.endpoint_route(url)
Issue-Key: elbv2__lambda.xsvc-elbv2-3-lambda-invoke-permission-removed-is-denied-with-502-until-regranted|_http#27|PublishedUrlRejected|target, route = ctx.endpoint_route(url)
Issue-Key: elbv2__lambda.xsvc-elbv2-3-lambda-health-check-500-and-raise-mark-only-those-targets-unhealthy|_poll_until#24|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Commit: 6bab9504f4457a639e729a9dc2b08c46f536304f

# elbv2__lambda: 2 net-new causes — the ALB's published DNSName is an unserved elb.amazonaws.com host, and every Lambda target is reported healthy regardless of what the function returns

> _Note: this issue was identified against the code base at commit `6bab9504f4457a639e729a9dc2b08c46f536304f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateLoadBalancer

## Description
Two net-new root causes (Cause 1 of 2, Cause 2 of 2). Related open filings that do NOT carry these replay keys: 20261104T044826Z-elbv2-lambda-target-revoking-and-regranting-the-invoke.md, 20261105T093535Z-elbv2-lambda-a-lambda-target-that-breaks-the.md and 20261106T151148Z-elbv2-lambda-a-malformed-lambda-target-response-never-c548cb.md sit under this heading; the suite marks none of the three ids below OWNED.

Cause 1 of 2: ELBv2 publishes LoadBalancer.DNSName in the real-AWS elb.amazonaws.com domain — an address other than the one the emulator is reached on.
AWS: Smithy (elasticloadbalancingv2) CreateLoadBalancer -> LoadBalancers[].DNSName, "The public DNS name of the load balancer"; LoadBalancer.Scheme documents that an internet-facing load balancer's DNS name resolves to the load balancer nodes, so a client request to that name is invoked against the lambda-type target group and the load balancer turns a malformed function response into its own 502. Recorded behavior only for the wire details.
Emulator: DNSName is <lb-name>-<id>.<region>.elb.amazonaws.com (name and digits are per-run values; the defect is the domain) and the harness refuses it with simparity.published.PublishedUrlRejected "endpoint_url was given http://<published-host>/<path> (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener".
Why: the whole ALB/Lambda contract — the event the function receives, the 502 the load balancer generates for a malformed response, and the 502 it generates once lambda:Invoke is denied — is only observable through a request to the load balancer's own name, which the emulator never answers on.
Fix: the ELBv2 load-balancer state/serializer path that mints and reports DNSName, plus the listener socket path that must serve on that address and invoke the lambda-type target group.
Traces: elbv2__lambda.malformed-lambda-target-response-becomes-a-502-after-the-invocation; elbv2__lambda.xsvc-elbv2-3-lambda-invoke-permission-removed-is-denied-with-502-until-regranted.

Cause 2 of 2: DescribeTargetHealth reports State 'healthy' for every Lambda target in a lambda-type target group, including functions whose health-check invocation returns 500 or raises.
AWS: Smithy (elasticloadbalancingv2) DescribeTargetHealth -> TargetHealthDescriptions[].TargetHealth.State/Reason/Description. For a lambda-type target group with HealthCheckEnabled, AWS invokes the function as the health check and reports 'unhealthy' with Reason 'Target.FailedHealthChecks' when the invocation errors or returns a non-matching status, leaving only the passing function 'healthy'.
Emulator: all three targets report ({'State': 'healthy'}, {'State': 'healthy'}, {'State': 'healthy'}) for the full 600 s window — the one that answers 500, the one that raises, and the good one alike; the observed buckets stay {'bad': [], 'good': [], 'err': []}.
Why: health is synthesized from registration rather than from the invocation, so a client cannot tell a broken function from a working one and the load balancer will keep sending traffic to a target AWS would have withdrawn.
Fix: the ELBv2 target-health component for lambda-type target groups must perform the health-check invocation and set TargetHealth.State/Reason from its result (Target.FailedHealthChecks on error or non-matching status).
Traces: elbv2__lambda.xsvc-elbv2-3-lambda-health-check-500-and-raise-mark-only-those-targets-unhealthy.

## Evidence
Replay of suite simparity-behavioral-aws-v1: elbv2__lambda 0/3 (0%), 3 failed; none OWNED.

Cause 1:
[mismatch] elbv2__lambda.malformed-lambda-target-response-becomes-a-502-after-the-invocation
  simparity.published.PublishedUrlRejected: endpoint_url was given http://sim2real-behav-lb-3953968e1187-8273984436.us-west-2.elb.amazonaws.com/<path>/ok (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:44909/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener
[mismatch] elbv2__lambda.xsvc-elbv2-3-lambda-invoke-permission-removed-is-denied-with-502-until-regranted — same frame, host sim2real-xsvc-lb-b7a25581dc47-2597574348.us-west-2.elb.amazonaws.com

Cause 2:
[mismatch] elbv2__lambda.xsvc-elbv2-3-lambda-health-check-500-and-raise-mark-only-those-targets-unhealthy
  AssertionError: 500/raise targets unhealthy and good target healthy (observed {'bad': [], 'good': [], 'err': []}) not met within 600s. Last result: ({'State': 'healthy'}, {'State': 'healthy'}, {'State': 'healthy'})
  Expected: the 500 and raising targets 'unhealthy' with Reason 'Target.FailedHealthChecks', the passing one 'healthy'. Actual: all three 'healthy'.

Smithy consulted: elasticloadbalancingv2 CreateLoadBalancer output LoadBalancers[] (LoadBalancer.DNSName, LoadBalancer.Scheme); DescribeTargetHealth output TargetHealthDescriptions[].TargetHealth {State, Reason, Description}.

## Steps to Reproduce
1. Clone the repo and replay the behavioral suite; the three ids in trace_ids reproduce.
2. Cause 1: create an ALB, a lambda-type target group registering a function, and a listener forwarding to it; read DNSName and GET http://<DNSName>/<path>. The emulator publishes an elb.amazonaws.com host it does not serve.
3. Cause 2: create three lambda-type target groups with HealthCheckEnabled=True, registering (a) a function returning statusCode 500, (b) a function that raises, (c) a function returning 200, and poll elbv2.describe_target_health on each. Expect (a) and (b) 'unhealthy' with Reason 'Target.FailedHealthChecks' and (c) 'healthy'; the emulator reports 'healthy' for all three.
4. Re-replay the ids in trace_ids to confirm the fix.
