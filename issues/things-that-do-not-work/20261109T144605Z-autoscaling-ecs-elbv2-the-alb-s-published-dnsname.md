Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ecs__elbv2.alb-listener-forwards-request-to-ecs-task-ip
Issue-Key: autoscaling__ecs__elbv2.alb-listener-forwards-request-to-ecs-task-ip|_http_get_status#4|PublishedUrlRejected|target, route = ctx.endpoint_route(url)
Issue-Commit: 5252be52e0cc9f5d73bee5aa197523afb271707f

# autoscaling__ecs__elbv2: the ALB's published DNSName is an elb.amazonaws.com host the emulator does not serve, so no request can reach an ECS task through a listener

> _Note: this issue was identified against the code base at commit `5252be52e0cc9f5d73bee5aa197523afb271707f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateLoadBalancer

## Description
Cause 1 of 1: ELBv2 publishes a load balancer DNSName under the real-AWS `*.us-west-2.elb.amazonaws.com` domain, which is not one of the addresses the emulator is reachable on, so the ALB data plane is unusable.

AWS: Smithy (elasticloadbalancingv2): CreateLoadBalancer/DescribeLoadBalancers return LoadBalancer.DNSName, the name a client resolves and connects to in order to reach the listener. On real AWS that name resolves to the load balancer the caller just created and an HTTP GET through an HTTP:80 listener with a forward action returns the target's response. The requirement is that the announced address is the address the service answers on.

Emulator: the DNSName handed back is `http://sim2real-alb-<id>-<id>.us-west-2.elb.amazonaws.com/`, a host the emulator does not own and does not listen on. The harness refuses the egress: `simparity.published.PublishedUrlRejected: endpoint_url was given http://sim2real-alb-<id>-<id>.us-west-2.elb.amazonaws.com/ (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener`. The fixed part of that requirement is 'the published DNSName must be a host/port the emulator serves'; the host, port and name are per-environment values.

Why: a client that does what every ALB client does — read DNSName and issue a request — is sent to an address outside the emulator. No ALB-fronted workload can be exercised end to end, and the failure looks like a network/DNS problem rather than an emulator gap.

Fix: the ELBv2 load-balancer serializer and its data-plane listener binding: DNSName must be synthesized from the emulator's own host and the per-resource listener port it actually binds, not from the AWS production domain template.

Traces: autoscaling__ecs__elbv2.alb-listener-forwards-request-to-ecs-task-ip. (The same root cause is filed for cognito-idp__elbv2 as 20261109T124858Z-cognito-idp-elbv2-an-alb-s-published-dnsname.md; that issue does not carry this check.)

## Evidence
Expected: DescribeLoadBalancers DNSName is an address the emulator serves, and an HTTP GET to it through the HTTP:80 listener returns the ECS task's response body.
Actual: DNSName is `sim2real-alb-<id>-<id>.us-west-2.elb.amazonaws.com`; the request is refused with `PublishedUrlRejected: ... its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>`.

## Steps to Reproduce
1. elbv2 CreateLoadBalancer (application, internet-facing) + CreateTargetGroup(target_type=ip) + CreateListener(protocol=HTTP, port=80, forward).
2. Register an awsvpc ECS task IP as a target.
3. Read DNSName from DescribeLoadBalancers and GET http://<DNSName>/.
Observed: the name is an elb.amazonaws.com host the emulator does not serve.
