Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2.alb-generated-responses-excluded-from-requestcount-and-target-codes elbv2.deploy-elbv2-2-rule-priority-collision-update-rejected elbv2.deregistration-delay-serves-in-flight-requests-while-draining elbv2.desync-mitigation-strictest-blocks-ambiguous-framing-before-the-target elbv2.drop-invalid-header-fields-strips-the-header-before-the-target elbv2.e2e-alb-target-routing-lifecycle elbv2.e2e-alb-traffic-lifecycle-flow elbv2.e2e-blue-green-weighted-target-group-shift elbv2.e2e-http-to-https-redirect-traffic-flow elbv2.elbv2attr-deep-elbv2-1-mtls-passthrough-clientcert-chain elbv2.elbv2attr-deep-elbv2-1-mtls-passthrough-forged-clientcert-header-stripped elbv2.elbv2attr-deep-elbv2-1-server-header-removed-and-hsts-injected elbv2.elbv2shape-deep-elbv2-3-app-cookie-stickiness-pins-with-cookie-spreads-without elbv2.elbv2shape-deep-elbv2-3-least-outstanding-requests-starves-slow-backend-vs-round-robin elbv2.elbv2shape-deep-elbv2-3-oversize-request-line-and-headers-rejected-by-alb elbv2.elbv2shape-deep-elbv2-3-slow-start-ramps-late-target-but-not-one-call-batch elbv2.elbv2shape-deep-elbv2-3-slow-start-with-least-outstanding-requests-rejected elbv2.forward-weighted-and-sticky-target-groups-data-plane elbv2.imported-certificate-is-presented-and-sni-selects-per-host elbv2.instance-target-serves-http-and-unhealthy-stops-routing elbv2.lambda-target-request-response-contract elbv2.lb-cookie-stickiness-pins-a-client-to-one-target-in-the-group elbv2.listener-rule-condition-type-matrix-routing elbv2.listener-rule-priority-decides-which-rule-serves-the-request elbv2.listener-rule-priority-determines-match-winner elbv2.listener-rule-priority-precedence-data-plane elbv2.listener-ssl-policy-bounds-the-negotiated-tls-version-and-cipher elbv2.load-balancer-active-state-transition elbv2.nlb-tcp-listener-preserves-client-source-ip-to-instance-target elbv2.redirect-action-placeholder-substitution elbv2.routing-http2-attribute-decides-the-alpn-protocol-the-alb-selects elbv2.source-ip-condition-uses-the-layer3-address-not-x-forwarded-for elbv2.target-response-exceeding-idle-timeout-yields-504 elbv2.unmatched-request-falls-through-to-the-default-action elbv2.xff-header-processing-mode-changes-what-the-target-receives elbv2.zero-weight-target-group-is-never-selected elbv2.elbv2-target-reaches-healthy elbv2.health-check-transitions-to-unhealthy-with-reason elbv2.server-side-periodic-health-check-executes-without-client-action
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elbv2.alb-generated-responses-excluded-from-requestcount-and-target-codes|_http#28|PublishedUrlRejected|_url, _route = ctx.endpoint_route(url)
Issue-Key: elbv2.deploy-elbv2-2-rule-priority-collision-update-rejected|_http_get#19|PublishedUrlRejected|_url, _route = ctx.endpoint_route(url)
Issue-Key: elbv2.deregistration-delay-serves-in-flight-requests-while-draining|_http#27|PublishedUrlRejected|_url, _route = ctx.endpoint_route(url)
Issue-Key: elbv2.desync-mitigation-strictest-blocks-ambiguous-framing-before-the-target|_raw_request#22|PublishedUrlRejected|_t = urllib.parse.urlsplit(ctx.endpoint_url(f"http://{host}"))
Issue-Key: elbv2.drop-invalid-header-fields-strips-the-header-before-the-target|_http#25|PublishedUrlRejected|_url, _route = ctx.endpoint_route(url)
Issue-Key: elbv2.e2e-alb-target-routing-lifecycle|run#22|PublishedUrlRejected|base = ctx.endpoint_url(f"http://{dns_name}")
Issue-Key: elbv2.e2e-alb-traffic-lifecycle-flow|_fetch_ok#55|PublishedUrlRejected|_url, _route = ctx.endpoint_route(f"http://{dns_name}/")
Issue-Key: elbv2.e2e-blue-green-weighted-target-group-shift|_poll_http_get#23|PublishedUrlRejected|_url, _route = ctx.endpoint_route(f"http://{host}:{port}")
Issue-Key: elbv2.e2e-http-to-https-redirect-traffic-flow|_poll_http_get#21|PublishedUrlRejected|_url, _route = ctx.endpoint_route(f"http://{host}:{port}")
Issue-Key: elbv2.elbv2attr-deep-elbv2-1-mtls-passthrough-clientcert-chain|_https#29|PublishedUrlRejected|_url, _route = ctx.endpoint_route(f"https://{host}")
Issue-Key: elbv2.elbv2attr-deep-elbv2-1-mtls-passthrough-forged-clientcert-header-stripped|_https#28|PublishedUrlRejected|_url, _route = ctx.endpoint_route(f"https://{host}")
Issue-Key: elbv2.elbv2attr-deep-elbv2-1-server-header-removed-and-hsts-injected|_https#27|PublishedUrlRejected|_url, _route = ctx.endpoint_route(f"https://{host}")
Issue-Key: elbv2.elbv2shape-deep-elbv2-3-app-cookie-stickiness-pins-with-cookie-spreads-without|_http#19|PublishedUrlRejected|_url, _route = ctx.endpoint_route(url)
Issue-Key: elbv2.elbv2shape-deep-elbv2-3-least-outstanding-requests-starves-slow-backend-vs-round-robin|_http#23|PublishedUrlRejected|_url, _route = ctx.endpoint_route(url)
Issue-Key: elbv2.elbv2shape-deep-elbv2-3-oversize-request-line-and-headers-rejected-by-alb|_http#17|PublishedUrlRejected|_url, _route = ctx.endpoint_route(url)
Issue-Key: elbv2.elbv2shape-deep-elbv2-3-slow-start-ramps-late-target-but-not-one-call-batch|_http#22|PublishedUrlRejected|_url, _route = ctx.endpoint_route(url)
Issue-Key: elbv2.elbv2shape-deep-elbv2-3-slow-start-with-least-outstanding-requests-rejected|_http#18|PublishedUrlRejected|_url, _route = ctx.endpoint_route(url)
Issue-Key: elbv2.forward-weighted-and-sticky-target-groups-data-plane|_http#25|PublishedUrlRejected|_url, _route = ctx.endpoint_route(url)
Issue-Key: elbv2.imported-certificate-is-presented-and-sni-selects-per-host|_presented#18|PublishedUrlRejected|_t = urllib.parse.urlsplit(ctx.endpoint_url(f"https://{host}"))
Issue-Key: elbv2.instance-target-serves-http-and-unhealthy-stops-routing|_http#19|PublishedUrlRejected|_url, _route = ctx.endpoint_route(url)
Issue-Key: elbv2.lambda-target-request-response-contract|_http#17|PublishedUrlRejected|_url, _route = ctx.endpoint_route(url)
Issue-Key: elbv2.lb-cookie-stickiness-pins-a-client-to-one-target-in-the-group|_http#19|PublishedUrlRejected|_url, _route = ctx.endpoint_route(url)
Issue-Key: elbv2.listener-rule-condition-type-matrix-routing|_http#22|PublishedUrlRejected|_url, _route = ctx.endpoint_route(url)
Issue-Key: elbv2.listener-rule-priority-decides-which-rule-serves-the-request|run#8|PublishedUrlRejected|url = f"{ctx.endpoint_url('http://' + lb['DNSName'])}/pick/one"
Issue-Key: elbv2.listener-rule-priority-determines-match-winner|_wait_for_http_body#12|PublishedUrlRejected|_url, _route = ctx.endpoint_route(url)
Issue-Key: elbv2.listener-rule-priority-precedence-data-plane|_http#13|PublishedUrlRejected|_url, _route = ctx.endpoint_route(url)
Issue-Key: elbv2.listener-ssl-policy-bounds-the-negotiated-tls-version-and-cipher|_handshake#17|PublishedUrlRejected|_t = urllib.parse.urlsplit(ctx.endpoint_url(f"https://{host}"))
Issue-Key: elbv2.load-balancer-active-state-transition|_try_resolve#15|PublishedUrlRejected|return socket.gethostbyname(ctx.endpoint_host(hostname, 80)[0])
Issue-Key: elbv2.nlb-tcp-listener-preserves-client-source-ip-to-instance-target|_http#24|PublishedUrlRejected|_url, _route = ctx.endpoint_route(url)
Issue-Key: elbv2.redirect-action-placeholder-substitution|_http#18|PublishedUrlRejected|_url, _route = ctx.endpoint_route(url)
Issue-Key: elbv2.routing-http2-attribute-decides-the-alpn-protocol-the-alb-selects|_alpn#13|PublishedUrlRejected|_t = urllib.parse.urlsplit(ctx.endpoint_url(f"https://{host}"))
Issue-Key: elbv2.source-ip-condition-uses-the-layer3-address-not-x-forwarded-for|_http#20|PublishedUrlRejected|_url, _route = ctx.endpoint_route(url)
Issue-Key: elbv2.target-response-exceeding-idle-timeout-yields-504|_http#24|PublishedUrlRejected|_url, _route = ctx.endpoint_route(url)
Issue-Key: elbv2.unmatched-request-falls-through-to-the-default-action|_http#11|PublishedUrlRejected|_url, _route = ctx.endpoint_route(url)
Issue-Key: elbv2.xff-header-processing-mode-changes-what-the-target-receives|_http#18|PublishedUrlRejected|_url, _route = ctx.endpoint_route(url)
Issue-Key: elbv2.zero-weight-target-group-is-never-selected|_http#23|PublishedUrlRejected|_url, _route = ctx.endpoint_route(url)
Issue-Key: elbv2.elbv2-target-reaches-healthy|run#34|AssertionError|assert blocked == "unhealthy", (
Issue-Key: elbv2.health-check-transitions-to-unhealthy-with-reason|run#29|AssertionError|assert elapsed >= min_s - 5, (
Issue-Key: elbv2.server-side-periodic-health-check-executes-without-client-action|run#16|AssertionError|raise AssertionError(
Issue-Commit: 6bab9504f4457a639e729a9dc2b08c46f536304f

# elbv2: 4 net-new causes — the published DNSName is a real-AWS elb.amazonaws.com host the emulator does not serve (36 data-plane checks), and target health is reported without any probe ever running

> _Note: this issue was identified against the code base at commit `6bab9504f4457a639e729a9dc2b08c46f536304f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateLoadBalancer

## Description
Four net-new root causes in the elbv2 service (Cause 1 of 4 .. Cause 4 of 4). Related open filings that do NOT carry these replay keys: 20261109T132410Z-acm-elbv2-the-alb-s-published-dnsname-is.md and 20261109T124858Z-cognito-idp-elbv2-an-alb-s-published-dnsname.md describe the same published-address defect but are filed under the acm__elbv2 / cognito-idp__elbv2 headings and carry none of the check ids below; 20261105T050431Z-elbv2-describetargethealth-reports-a-constant-healthy-for-an.md and 20261104T094656Z-elbv2-seven-target-health-routing-and-data-plane-causes-still-open.md are adjacent to Causes 2-4 but the suite did not mark any of these checks OWNED, so they survive as live failures.

Cause 1 of 4: ELBv2 publishes LoadBalancer.DNSName in the real-AWS elb.amazonaws.com domain, i.e. an address other than the one the emulator is reached on, so no ALB/NLB data-plane behaviour is reachable at all.
AWS: Smithy (elasticloadbalancingv2) CreateLoadBalancer -> LoadBalancers[].DNSName, documented as "The public DNS name of the load balancer"; LoadBalancer.Scheme documents that an internet-facing load balancer's DNS name is publicly resolvable to the addresses of the load balancer nodes. On AWS the published DNSName is therefore the address that actually answers the listener ports: a client resolves it, connects, and the listener's rules/actions/attributes are applied to that request. Recorded behavior only for the wire details (the model covers the control plane).
Emulator: CreateLoadBalancer answers 200 with DNSName of the form <lb-name>-<id>.<region>.elb.amazonaws.com (the name and digits are per-run values; the defect is the domain), and DescribeLoadBalancers repeats it. That host is not one of the emulator's own hosts, so the harness refuses the published address before any connect: simparity.published.PublishedUrlRejected "endpoint_url was given http://<published-host>/<path> (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener" (and the endpoint_host variant for raw-socket probes).
Why: every client that does the one thing a load balancer exists for — send a request to the load balancer under the name the API handed it — is pointed at a hostname the emulator never answers on. It either escapes to real AWS or fails to resolve. Listener rules and priorities, weighted/zero-weight forwarding, stickiness, slow start, least-outstanding-requests, redirect placeholders, XFF/desync/drop-invalid-header attributes, idle-timeout 504s, TLS/SNI/ALPN negotiation, Lambda targets and the request/response contract are all unreachable, with a 200 control-plane rollout as the only signal.
Why (client impact, continued): a 200 from CreateLoadBalancer that hands back an unusable address is worse than a refusal, because the client has no way to tell.
Fix: the ELBv2 load-balancer state/serializer path that mints and reports DNSName (CreateLoadBalancer and DescribeLoadBalancers must publish the emulator's own reachable host[:port]), together with the listener socket path that must accept connections on that address and run the listener's rules and attributes.
Traces: all 36 ids listed in trace_ids except the three named under Causes 2-4 below; e.g. elbv2.e2e-alb-traffic-lifecycle-flow, elbv2.listener-rule-priority-precedence-data-plane, elbv2.zero-weight-target-group-is-never-selected.

Cause 2 of 4: DescribeTargetHealth reports State 'healthy' for a target whose health-check port has no security-group ingress, so nothing could have probed it.
AWS: Smithy (elasticloadbalancingv2) DescribeTargetHealth -> TargetHealthDescriptions[].TargetHealth.State; AWS only reports 'healthy' after HealthyThresholdCount consecutive successful probes from the load balancer nodes to the target's health-check port. With no ingress permitting the load balancer's security group on that port the probes cannot complete, so AWS reports 'unhealthy' with Reason Target.Timeout (Description "Request timed out"). Recorded behavior for the reachability dependency.
Emulator: the target is reported {'State': 'healthy'} with no Reason at all, with no listener on, and no ingress to, the health-check port (replay: health-check port 80 with no security-group ingress). The state is synthesized from registration, not from a probe.
Why: a client cannot distinguish a reachable target from an unreachable one; every security-group, listener and container misconfiguration reads as healthy, and health-driven routing, failover and ASG/ECS replacement logic built on DescribeTargetHealth silently does the wrong thing.
Fix: the ELBv2 target-health component — the periodic health-check probe and the target-health state machine behind DescribeTargetHealth — must derive State/Reason/Description from an actual probe of the target's health-check protocol/port/path and report Target.Timeout when the probe cannot complete.
Traces: elbv2.elbv2-target-reaches-healthy.

Cause 3 of 4: a target's transition to 'unhealthy' ignores UnhealthyThresholdCount x HealthCheckIntervalSeconds and happens almost immediately.
AWS: Smithy (elasticloadbalancingv2) TargetGroup.UnhealthyThresholdCount ("The number of consecutive health check failures required before considering the target unhealthy") and TargetGroup.HealthCheckIntervalSeconds ("The approximate amount of time, in seconds, between health checks of an individual target"). AWS therefore cannot declare a target unhealthy sooner than UnhealthyThresholdCount consecutive failed intervals; the floor is the product of those two modeled members.
Emulator: the target was declared unhealthy 1.0 s after the failure began, while the target group's UnhealthyThresholdCount=2 and HealthCheckIntervalSeconds=5 require at least 10 s. The replay message is "declared unhealthy after 1.0s but UnhealthyThresholdCount=2 x interval=5s requires >= 10s" — the 1.0 s, 2 and 5 are the run's values; the requirement is the product rule.
Why: clients tune threshold and interval precisely to avoid flapping on a single blip; an emulator that flips state on the first failure makes any test of deregistration delay, grace periods, or flap-resistant failover pass or fail for the wrong reason.
Fix: the ELBv2 target-health state machine must count consecutive probe outcomes and apply the target group's UnhealthyThresholdCount/HealthyThresholdCount against HealthCheckIntervalSeconds before changing State.
Traces: elbv2.health-check-transitions-to-unhealthy-with-reason.

Cause 4 of 4: the load balancer performs no autonomous, server-side health checks — a Lambda target registered in a target group is never invoked unless a client drives traffic.
AWS: recorded behavior only (the model does not describe probe scheduling). AWS ELBv2 probes each registered target on its own schedule, every HealthCheckIntervalSeconds, with no client action; for a Lambda target that probe is a lambda:Invoke, so AWS/Lambda Invocations accrues for the function while the check merely waits.
Emulator: no AWS/Lambda Invocations datapoint ever lands for the registered Lambda target within the check's 5-minute ingestion window ("AWS/Lambda Invocations metric never landed within 5 minutes of ingestion latency; cannot confirm autonomous health checks via telemetry"), i.e. the emulator never probes a registered target on its own.
Why: health in the emulator is a function of client requests rather than of the target's real state, so nothing a client observes about health is independent evidence; failover and replacement behaviour that depends on background probing never triggers.
Fix: the ELBv2 health-check scheduler must run per-target probes on the target group's interval independently of inbound data-plane traffic, and a Lambda-type target's probe must go through the Lambda invoke path so the invocation is recorded.
Traces: elbv2.server-side-periodic-health-check-executes-without-client-action.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the booted emulator: 253/598 passed; service heading `elbv2` 171/303 (56%), 132 failed (most OWNED/ALREADY FILED; the 39 ids in trace_ids are the survivors).

Cause 1, representative (identical frame in all 36):
  File "context.py", line 572, in endpoint_route -> return self._published_target(url, "endpoint_url"), {}
  File "context.py", line 586, in _published_target -> raise PublishedUrlRejected(...)
  simparity.published.PublishedUrlRejected: endpoint_url was given http://sim2real-behav-lb-9d148ceb7071-7011616140.us-west-2.elb.amazonaws.com/ (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:43329/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener
  [elbv2.alb-generated-responses-excluded-from-requestcount-and-target-codes]
  Same for HTTPS listeners (elbv2.listener-ssl-policy-bounds-the-negotiated-tls-version-and-cipher: https://sim2real-behav-lb-a6cd4b3f241a-8758047940.us-west-2.elb.amazonaws.com) and for the raw-socket form (elbv2.load-balancer-active-state-transition: endpoint_host was given sim2real-elbv2-lb-2a2a27f9d443-6816274750.us-west-2.elb.amazonaws.com:80 ... a customer of the emulator needs an own host[:port]).
  Expected: DNSName is an address the load balancer answers on and the check's HTTP/TLS probe reaches the listener. Actual: an AWS-owned elb.amazonaws.com host the emulator does not serve.

Cause 2: [mismatch] elbv2.elbv2-target-reaches-healthy
  AssertionError: DATAPLANE_UNIMPLEMENTED: target reported 'healthy' (reason=None) while its health-check port (80) had NO security-group ingress -- nothing could have probed it, so 'healthy' here is not a real health check
  Expected: TargetHealth.State 'unhealthy' with Reason 'Target.Timeout'. Actual: {'State': 'healthy'} with no Reason.

Cause 3: [mismatch] elbv2.health-check-transitions-to-unhealthy-with-reason
  AssertionError: declared unhealthy after 1.0s but UnhealthyThresholdCount=2 x interval=5s requires >= 10s

Cause 4: [mismatch] elbv2.server-side-periodic-health-check-executes-without-client-action
  AssertionError: AWS/Lambda Invocations metric never landed within 5 minutes of ingestion latency; cannot confirm autonomous health checks via telemetry

Smithy consulted (model services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10): CreateLoadBalancer output LoadBalancers[] (LoadBalancer.DNSName, LoadBalancer.Scheme); DescribeTargetHealth output TargetHealthDescriptions[].TargetHealth {State, Reason, Description}, errors TargetGroupNotFound/InvalidTarget/HealthUnavailable; TargetGroup.UnhealthyThresholdCount, TargetGroup.HealthCheckIntervalSeconds.

## Steps to Reproduce
1. Clone the repo and replay the behavioral suite; every id in trace_ids reproduces.
2. Cause 1: elbv2.create_load_balancer(Name=..., Subnets=[2 subnets], Type='application', Scheme='internet-facing'), then read LoadBalancers[0].DNSName. The emulator returns <name>-<id>.<region>.elb.amazonaws.com. Create an HTTP:80 listener with a fixed-response default action and send GET http://<DNSName>/ — on AWS the listener answers; here the published host is not an address the emulator is reached on, so the probe is refused before connect. Repeat with Protocol='HTTPS' and a raw TLS socket to <DNSName>:443 for the endpoint_host form.
3. Cause 2: create a target group (HealthCheckProtocol HTTP, HealthCheckPort traffic-port 80), register an in-VPC ip target whose security group permits NO ingress on port 80, and poll elbv2.describe_target_health. Expect State 'unhealthy' / Reason 'Target.Timeout'; the emulator reports State 'healthy' with no Reason.
4. Cause 3: create a target group with HealthCheckIntervalSeconds=5 and UnhealthyThresholdCount=2, register a target that answers, then make it fail, and time the transition to 'unhealthy'. Expect no earlier than 10 s; the emulator flips in about 1 s.
5. Cause 4: register a Lambda function as the only target of a lambda-type target group with health checks enabled, then wait without sending any client request and read AWS/Lambda Invocations for the function. Expect invocations from the load balancer's own probes; the emulator records none.
6. Re-replay the ids in trace_ids to confirm the fix.
