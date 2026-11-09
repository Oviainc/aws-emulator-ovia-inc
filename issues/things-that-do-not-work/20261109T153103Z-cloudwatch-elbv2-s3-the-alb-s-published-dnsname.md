Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__elbv2__s3.access-log-error-reason-names-the-lambda-target-failure-class cloudwatch__elbv2__s3.access-log-lambda-target-entries-carry-the-function-status-and-no-target-address cloudwatch__elbv2__s3.lambda-target-request-body-over-1-mb-answered-413-without-invoking-the-function cloudwatch__elbv2__s3.lambda-user-error-counts-crashed-and-malformed-invocations-not-target-500s cloudwatch__elbv2__s3.metric-math-5xx-rate-derives-from-alb-target-counts cloudwatch__elbv2__s3.target-500s-count-as-target-5xx-not-elb-5xx cloudwatch__elbv2__s3.target-5xx-metric-narrows-to-the-target-group-that-answered cloudwatch__elbv2__s3.target-5xx-rate-alarm-stays-ok-under-a-low-error-rate-that-fires-a-count-alarm
Issue-Key: cloudwatch__elbv2__s3.access-log-error-reason-names-the-lambda-target-failure-class|_call#9|PublishedUrlRejected|url, route = ctx.endpoint_route(f"http://{alb['dns']}{path}")
Issue-Key: cloudwatch__elbv2__s3.access-log-lambda-target-entries-carry-the-function-status-and-no-target-address|_call#9|PublishedUrlRejected|url, route = ctx.endpoint_route(f"http://{alb['dns']}{path}")
Issue-Key: cloudwatch__elbv2__s3.lambda-target-request-body-over-1-mb-answered-413-without-invoking-the-function|_call#7|PublishedUrlRejected|url, route = ctx.endpoint_route(f"http://{alb['dns']}{path}")
Issue-Key: cloudwatch__elbv2__s3.lambda-user-error-counts-crashed-and-malformed-invocations-not-target-500s|_call#7|PublishedUrlRejected|url, route = ctx.endpoint_route(f"http://{alb['dns']}{path}")
Issue-Key: cloudwatch__elbv2__s3.metric-math-5xx-rate-derives-from-alb-target-counts|_call#7|PublishedUrlRejected|url, route = ctx.endpoint_route(f"http://{alb['dns']}{path}")
Issue-Key: cloudwatch__elbv2__s3.target-500s-count-as-target-5xx-not-elb-5xx|_call#7|PublishedUrlRejected|url, route = ctx.endpoint_route(f"http://{alb['dns']}{path}")
Issue-Key: cloudwatch__elbv2__s3.target-5xx-metric-narrows-to-the-target-group-that-answered|_call#13|PublishedUrlRejected|url, route = ctx.endpoint_route(f"http://{alb['dns']}{path}")
Issue-Key: cloudwatch__elbv2__s3.target-5xx-rate-alarm-stays-ok-under-a-low-error-rate-that-fires-a-count-alarm|_call#7|PublishedUrlRejected|url, route = ctx.endpoint_route(f"http://{alb['dns']}{path}")
Issue-Commit: 6bab9504f4457a639e729a9dc2b08c46f536304f

# cloudwatch__elbv2__s3: the ALB's published DNSName is an elb.amazonaws.com host the emulator does not serve, so no ALB/Lambda-target request metric or access-log record can ever be produced

> _Note: this issue was identified against the code base at commit `6bab9504f4457a639e729a9dc2b08c46f536304f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateLoadBalancer

## Description
One net-new root cause (Cause 1 of 1). Related open filings that do NOT carry these replay keys: 20261104T055008Z-cloudwatch-elbv2-s3-5-net-new-root-causes.md and 20261107T183213Z-cloudwatch-elbv2-s3-enabling-alb-access-logs-writes.md own other checks in this heading; the eight ids below are refused earlier, in the harness's published-address gate, so the suite marks none of them OWNED.

Cause 1 of 1: ELBv2 publishes LoadBalancer.DNSName in the real-AWS elb.amazonaws.com domain — an address other than the one the emulator is reached on — so every metric and access-log assertion that first needs a request to reach the load balancer is unreachable.
AWS: Smithy (elasticloadbalancingv2) CreateLoadBalancer -> LoadBalancers[].DNSName, "The public DNS name of the load balancer"; LoadBalancer.Scheme documents that an internet-facing load balancer's DNS name resolves to the load balancer nodes. On AWS a client sends requests to that name, the load balancer answers or forwards to its targets, and AWS/ApplicationELB RequestCount / HTTPCode_ELB_5XX_Count / HTTPCode_Target_5XX_Count / TargetResponseTime and the S3 access-log records are produced as a side effect of those requests. Recorded behavior only for the wire details.
Emulator: CreateLoadBalancer answers 200 with DNSName of the form <lb-name>-<id>.<region>.elb.amazonaws.com (name and digits are per-run values; the defect is the domain). That host is not one of the emulator's own hosts, so the harness refuses it with simparity.published.PublishedUrlRejected "endpoint_url was given http://<published-host>/health (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener".
Why: a client that wants to observe ALB telemetry has no way to generate traffic: the only address the API gives it is one the emulator never answers on. The 5xx attribution split between ELB-generated and target-generated codes, Lambda user-error accounting, metric-math rates, per-target-group narrowing and alarm behaviour all become untestable while the control plane reports success.
Fix: the ELBv2 load-balancer state/serializer path that mints and reports DNSName (CreateLoadBalancer / DescribeLoadBalancers must publish the emulator's own reachable host[:port]), plus the listener socket path that must serve requests on that address and emit the AWS/ApplicationELB datapoints and access-log records for them.
Traces: the eight ids in trace_ids, e.g. cloudwatch__elbv2__s3.target-500s-count-as-target-5xx-not-elb-5xx and cloudwatch__elbv2__s3.metric-math-5xx-rate-derives-from-alb-target-counts.

## Evidence
Replay of suite simparity-behavioral-aws-v1: cloudwatch__elbv2__s3 1/17 (6%), 16 failed; eight are not OWNED and all fail with the identical frame.

[mismatch] cloudwatch__elbv2__s3.target-500s-count-as-target-5xx-not-elb-5xx
  File "context.py", line 572, in endpoint_route -> self._published_target(url, "endpoint_url")
  simparity.published.PublishedUrlRejected: endpoint_url was given http://sim2real-alb-92e1a8cb7dec-7629281682.us-west-2.elb.amazonaws.com/health (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:36561/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener

Same frame and message, different per-run host, for: access-log-error-reason-names-the-lambda-target-failure-class, access-log-lambda-target-entries-carry-the-function-status-and-no-target-address, lambda-target-request-body-over-1-mb-answered-413-without-invoking-the-function, lambda-user-error-counts-crashed-and-malformed-invocations-not-target-500s, metric-math-5xx-rate-derives-from-alb-target-counts, target-5xx-metric-narrows-to-the-target-group-that-answered, target-5xx-rate-alarm-stays-ok-under-a-low-error-rate-that-fires-a-count-alarm.

Expected: DNSName is an address the load balancer answers on, the probe reaches the listener and the resulting AWS/ApplicationELB datapoints and S3 access-log lines are readable. Actual: the published DNSName is an AWS-owned elb.amazonaws.com name the emulator does not serve.

Smithy consulted: elasticloadbalancingv2 CreateLoadBalancer output LoadBalancers[] (LoadBalancer.DNSName, LoadBalancer.Scheme).

## Steps to Reproduce
1. Clone the repo and replay the behavioral suite; all eight ids in trace_ids reproduce.
2. Create a VPC with two subnets and a security group; elbv2.create_load_balancer(Type='application', Scheme='internet-facing', Subnets=[...]).
3. Read LoadBalancers[0].DNSName — the emulator returns <name>-<id>.<region>.elb.amazonaws.com.
4. Create a listener forwarding to a target group (an ip target or a Lambda target) and send GET http://<DNSName>/health. On AWS the request is served and AWS/ApplicationELB metrics plus an S3 access-log record follow; here the published address is not one of the emulator's own hosts, so the request is refused before connect.
5. Re-replay the ids in trace_ids to confirm the fix.
