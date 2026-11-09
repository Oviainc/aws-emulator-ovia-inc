Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2__wafv2.associated-web-acl-blocks-the-request-before-the-alb-reaches-the-target
Issue-Key: elbv2__wafv2.associated-web-acl-blocks-the-request-before-the-alb-reaches-the-target|_http#23|PublishedUrlRejected|_url, _route = ctx.endpoint_route(url)
Issue-Commit: 6bab9504f4457a639e729a9dc2b08c46f536304f

# elbv2__wafv2: the ALB's published DNSName is an elb.amazonaws.com host the emulator does not serve, so an associated WebACL can never be exercised on the data path

> _Note: this issue was identified against the code base at commit `6bab9504f4457a639e729a9dc2b08c46f536304f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateLoadBalancer

## Description
One net-new root cause (Cause 1 of 1). Related open filing that does NOT carry this replay key: 20261104T094656Z-elbv2-wafv2-an-alb-with-a-lambda-target.md is under this heading but its recorded key is the ALB never answering; this replay is refused earlier, in the harness's published-address gate, so the suite does not mark the check OWNED.

Cause 1 of 1: ELBv2 publishes LoadBalancer.DNSName in the real-AWS elb.amazonaws.com domain — an address other than the one the emulator is reached on.
AWS: Smithy (elasticloadbalancingv2) CreateLoadBalancer -> LoadBalancers[].DNSName, "The public DNS name of the load balancer"; LoadBalancer.Scheme documents that an internet-facing load balancer's DNS name resolves to the load balancer nodes. On AWS a request to that name is evaluated by the WebACL associated with the load balancer before the listener forwards to any target, so a blocked request is answered 403 by WAF and never reaches the target. Recorded behavior only for the wire details.
Emulator: CreateLoadBalancer answers 200 with DNSName of the form <lb-name>-<id>.<region>.elb.amazonaws.com (name and digits are per-run values; the defect is the domain), which is not one of the emulator's own hosts, so the probe is refused with simparity.published.PublishedUrlRejected "endpoint_url was given http://<published-host>/<path> (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener".
Why: a client cannot send the one request that would prove the WebACL is enforced, so AssociateWebACL reporting success is indistinguishable from a WebACL that is entirely inert.
Fix: the ELBv2 load-balancer state/serializer path that mints and reports DNSName (CreateLoadBalancer / DescribeLoadBalancers must publish the emulator's own reachable host[:port]), plus the listener request path that must evaluate the associated WebACL before forwarding.
Traces: elbv2__wafv2.associated-web-acl-blocks-the-request-before-the-alb-reaches-the-target.

## Evidence
Replay of suite simparity-behavioral-aws-v1: elbv2__wafv2 0/1 (0%), 1 failed, not OWNED.

[mismatch] elbv2__wafv2.associated-web-acl-blocks-the-request-before-the-alb-reaches-the-target
  simparity.published.PublishedUrlRejected: endpoint_url was given http://sim2real-behav-lb-127964026a07-7140342865.us-west-2.elb.amazonaws.com/<path>/probe (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:40953/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener

Expected: the published DNSName answers, and with the WebACL associated the blocked request is answered 403 by WAF without reaching the target. Actual: the published DNSName is an AWS-owned elb.amazonaws.com host the emulator does not serve.

Smithy consulted: elasticloadbalancingv2 CreateLoadBalancer output LoadBalancers[] (LoadBalancer.DNSName, LoadBalancer.Scheme).

## Steps to Reproduce
1. Clone the repo and replay the behavioral suite; the id in trace_ids reproduces.
2. Create an ALB with a listener and a target; read LoadBalancers[0].DNSName — the emulator returns <name>-<id>.<region>.elb.amazonaws.com.
3. wafv2.create_web_acl with a blocking rule and wafv2.associate_web_acl(ResourceArn=<alb arn>), then GET http://<DNSName>/<path>/probe. On AWS the request is answered 403 by WAF; here the published address is not one of the emulator's own hosts and the request is refused before connect.
4. Re-replay the id in trace_ids to confirm the fix.
