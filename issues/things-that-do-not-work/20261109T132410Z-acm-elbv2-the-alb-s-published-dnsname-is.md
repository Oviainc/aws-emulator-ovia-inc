Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm__elbv2.acm-alb-https-listener-serves-the-imported-certificate acm__elbv2.live-listener-certificate-rotation-swaps-the-presented-leaf-with-no-dropped-request
Issue-Key: acm__elbv2.acm-alb-https-listener-serves-the-imported-certificate|_https_get#30|PublishedUrlRejected|with socket.create_connection(ctx.endpoint_host(dns_name, 443), timeout=15) as raw:
Issue-Key: acm__elbv2.live-listener-certificate-rotation-swaps-the-presented-leaf-with-no-dropped-request|run#21|PublishedUrlRejected|lb_address = ctx.endpoint_host(lb_dns, 443)
Issue-Commit: cda81fc610349487f102bcc6f8ad570fe265d90b

# acm__elbv2: the ALB's published DNSName is a real-AWS elb.amazonaws.com host the emulator does not serve, so no ACM certificate can ever be presented or rotated on the wire

> _Note: this issue was identified against the code base at commit `cda81fc610349487f102bcc6f8ad570fe265d90b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateLoadBalancer

## Description
One net-new root cause (Cause 1 of 1).

Related open filings that do NOT cover this replay: 20261107T183213Z-acm-elbv2-an-alb-https-443-listener-holding.md and 20261104T074116Z-acm-elbv2-3-net-new-root-causes-an.md both carry acm__elbv2.acm-alb-https-listener-serves-the-imported-certificate, but their recorded replay keys are a connect-time ConnectionRefusedError / a 60 s timeout inside the check's _https_get. This replay never reaches a connect at all: it is now refused earlier, in the harness's published-address gate (context.py endpoint_host -> simparity.published.PublishedUrlRejected), so the suite does not mark either check OWNED and both survive as live failures. 20261109T124858Z-cognito-idp-elbv2-an-alb-s-published-dnsname.md describes the same emulator behaviour but is filed under a different service heading (cognito-idp__elbv2) and so does not carry these two check ids.

Cause 1 of 1: ELBv2 publishes a load balancer's DNSName as a real-AWS hostname in the elb.amazonaws.com domain, i.e. an address other than the one the emulator is reached on.
AWS: Smithy (elasticloadbalancingv2) LoadBalancer.DNSName — "The public DNS name of the load balancer"; CreateLoadBalancer returns LoadBalancers[].DNSName and LoadBalancer.Scheme documents that "The DNS name of an Internet-facing load balancer is publicly resolvable to the public IP addresses of the nodes". On AWS the published DNSName is therefore the address that actually answers TCP/443 for the load balancer's listeners: a client connects to it, completes the TLS handshake, and is served the leaf ACM holds for the listener's Certificates[].CertificateArn. Recorded behavior only for the data plane (the model describes the control plane only), but the contract the recording shows is that DNSName is a connectable address.
Emulator: CreateLoadBalancer answers 200 with DNSName of the form <lb-name>-<id>.<region>.elb.amazonaws.com (replay: 'sim2real-xsvc-lb-1737cdf596e0-2280834542.us-west-2.elb.amazonaws.com', 'sim2real-xsvc-rl-9409a17fb227-3256253902.us-west-2.elb.amazonaws.com' — the names and ids are per-run values; the defect is the domain). That host is not one of the emulator's own hosts, so the published address is not the one the emulator is reached on: the harness refuses it with PublishedUrlRejected "endpoint_host was given <host>:443 (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs an own host[:port]: the endpoint host, a documented own host or, for a loopback endpoint, a loopback address".
Why: a client that does the one thing an ACM certificate on an ALB exists for — open TLS to the load balancer under its own name — is sent to a hostname the emulator never answers on. Either it resolves to real AWS (egress) or it fails to resolve; either way certificate presentation, SNI selection, hostname verification against a private anchor and live certificate rotation are all unreachable, with a successful 200 control-plane rollout as the only signal.
Fix: the ELBv2 service's load-balancer state/serializer path that mints and reports DNSName (CreateLoadBalancer and DescribeLoadBalancers must publish the emulator's own reachable host[:port] for the load balancer), together with the listener-socket path that must accept TLS on that address and present the ACM leaf for each Certificates[].CertificateArn.
Traces: acm__elbv2.acm-alb-https-listener-serves-the-imported-certificate; acm__elbv2.live-listener-certificate-rotation-swaps-the-presented-leaf-with-no-dropped-request.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the booted emulator: 238/270 passed; acm__elbv2 3/7 (43%), 4 failed — two are marked OWNED by 20261104T074116Z-acm-elbv2-3-net-new-root-causes-an.md, these two are not.

[mismatch] acm__elbv2.acm-alb-https-listener-serves-the-imported-certificate:
  File "context.py", line 619, in endpoint_host -> self._published_target(f"{published_host}:{port}", "endpoint_host")
  simparity.published.PublishedUrlRejected: endpoint_host was given sim2real-xsvc-lb-1737cdf596e0-2280834542.us-west-2.elb.amazonaws.com:443 (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs an own host[:port]: the endpoint host, a documented own host or, for a loopback endpoint, a loopback address

[mismatch] acm__elbv2.live-listener-certificate-rotation-swaps-the-presented-leaf-with-no-dropped-request:
  same frame, endpoint_host given sim2real-xsvc-rl-9409a17fb227-3256253902.us-west-2.elb.amazonaws.com:443

Both checks read the host straight from the control plane: lb_arn, lb_dns = lb["LoadBalancerArn"], lb["DNSName"] from elb.create_load_balancer(Scheme='internet-facing', Type='application', ...), then socket.create_connection(ctx.endpoint_host(dns_name, 443)). Expected: DNSName is an address the load balancer answers on, the TLS handshake completes and the presented leaf is byte-identical to the ACM certificate named by the HTTPS listener. Actual: the published DNSName is an AWS-owned elb.amazonaws.com name the emulator does not serve, so nothing the checks assert about the certificate is reachable.

Smithy consulted: elasticloadbalancingv2 LoadBalancer.DNSName ("The public DNS name of the load balancer"), LoadBalancer.Scheme (internet-facing DNS name resolves to the nodes' public addresses).

## Steps to Reproduce
1. Clone the repo and replay the behavioral suite; both check ids in trace_ids reproduce.
2. acm.import_certificate a leaf chained to a self-signed root; poll acm.describe_certificate until Status == ISSUED.
3. Create a VPC with two subnets and a security group allowing 443; elbv2.create_load_balancer(Name=..., Subnets=[...], SecurityGroups=[...], Scheme='internet-facing', Type='application').
4. Read LoadBalancers[0].DNSName from the response (or from DescribeLoadBalancers). The emulator returns <name>-<id>.<region>.elb.amazonaws.com — a real-AWS host, not an address the emulator is reached on.
5. elbv2.create_listener(Protocol='HTTPS', Port=443, Certificates=[{'CertificateArn': <arn>}], DefaultActions=[fixed-response]) and open a TLS connection to DNSName:443 with SNI = the certificate's SAN. On AWS the handshake completes and the imported leaf is presented; here the published address is not the emulator's own host, so the probe is refused before any connect.
6. Re-replay the two check ids in trace_ids to confirm the fix.
