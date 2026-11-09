Issue-Source: trace-sweep-divergence
Issue-Trace: elbclassic-create-load-balancer-validation-and-duplicate-name
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elbclassic-create-load-balancer-validation-and-duplicate-name elbclassic-health-check-configuration-round-trip-and-validation elbclassic-listener-lifecycle-duplicate-and-idempotent-delete elbclassic-load-balancer-lifecycle-create-describe-delete elbclassic-policy-lifecycle-cookie-stickiness-and-generic-policy elbclassic-subnet-attach-detach-and-security-group-reassignment
Issue-Shape-Id: com.amazonaws.elasticloadbalancing#ElasticLoadBalancing_v7
Issue-Key: elasticloadbalancing|DescribeLoadBalancers|2xx->2xx|$.DescribeLoadBalancersResponse.DescribeLoadBalancersResult.LoadBalancerDescriptions.member.CanonicalHostedZoneName
Issue-Key: elasticloadbalancing|DescribeLoadBalancers|2xx->2xx|$.DescribeLoadBalancersResponse.DescribeLoadBalancersResult.LoadBalancerDescriptions.member.DNSName
Issue-Key: elasticloadbalancing|CreateLoadBalancer|2xx->2xx|$.CreateLoadBalancerResponse.CreateLoadBalancerResult.DNSName
Issue-Commit: 01897137b22b96058352710896276b71185e2aeb

# ELB Classic publishes an unreachable *.elb.amazonaws.com DNSName from CreateLoadBalancer and DescribeLoadBalancers

> _Note: this issue was identified against the code base at commit `01897137b22b96058352710896276b71185e2aeb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeLoadBalancers

## Description
2 causes. Both are the same fault seen on the two operations that publish a Classic load balancer's address: the emulator mints a real-AWS Elastic Load Balancing hostname instead of an address it actually serves, so every value it hands a client points off the emulator.

Cause 1 of 2 — DescribeLoadBalancers reports an unreachable DNSName / CanonicalHostedZoneName (fixture elbclassic-create-load-balancer-validation-and-duplicate-name, step task#5; the same cluster covers 8 further steps across elbclassic-health-check-configuration-round-trip-and-validation, elbclassic-listener-lifecycle-duplicate-and-idempotent-delete, elbclassic-load-balancer-lifecycle-create-describe-delete, elbclassic-policy-lifecycle-cookie-stickiness-and-generic-policy and elbclassic-subnet-attach-detach-and-security-group-reassignment).
AWS: Smithy: DescribeLoadBalancers output DescribeAccessPointsOutput$LoadBalancerDescriptions (list of LoadBalancerDescription); LoadBalancerDescription$DNSName (target DNSName) documented 'The DNS name of the load balancer'; LoadBalancerDescription$CanonicalHostedZoneName (target DNSName) documented 'The DNS name of the load balancer'; LoadBalancerDescription$Scheme documented 'If Scheme is internet-facing, the load balancer has a public DNS name that resolves to a public IP address. If Scheme is internal, the load balancer has a public DNS name that resolves to a private IP address.' The model states the member's MEANING, not its spelling: the requirement is that the reported value is an address at which the load balancer's configured listeners actually accept connections. The textual form <name>-<10 digits>.<region>.elb.amazonaws.com is a real-AWS artifact, never a modeled constraint. Recorded behavior only for the exact literal: the recording does not assert the DNS string ('<shaped-not-asserted>'), it asserts that the published host is one the serving endpoint owns, and for the load balancer in this step the recorded response carries NO CanonicalHostedZoneName member at all.
Emulator: HTTP 200, but LoadBalancerDescriptions.member.DNSName is a synthesized real-AWS host of the form internal-<name>-<10 digits>.<region>.elb.amazonaws.com (verdict host_not_own), and CanonicalHostedZoneName is populated with that same unreachable host where the recording has the member absent.
Why: a client that does the one thing a load balancer exists for — resolve the returned DNSName and open a connection to it, or point a Route 53 alias at CanonicalHostedZoneName — is sent into real AWS DNS space (or to nothing). No listener, health-check, stickiness or cross-zone behaviour of the emulated load balancer can ever be observed, and the spurious CanonicalHostedZoneName makes clients believe an alias target exists.
Fix: the DNS-name minting in the ELB Classic crate's state (the load-balancer record's dns_name/dns_suffix) together with the DescribeLoadBalancers serializer that projects it: the stored value must be an address the emulator serves (its own host/port, or a per-resource listener it stands up for the load balancer's Listener$LoadBalancerPort entries), and CanonicalHostedZoneName must be populated only where AWS populates it.
Traces: elbclassic-create-load-balancer-validation-and-duplicate-name#task#5 (representative), plus elbclassic-health-check-configuration-round-trip-and-validation#task#6 and #task#10, elbclassic-listener-lifecycle-duplicate-and-idempotent-delete#task#6 and #task#10, elbclassic-load-balancer-lifecycle-create-describe-delete#task#5, elbclassic-policy-lifecycle-cookie-stickiness-and-generic-policy#task#12, elbclassic-subnet-attach-detach-and-security-group-reassignment#task#9 and #task#13. Code: services/service-com-amazonaws-elasticloadbalancing-elasticloadbalancing-v7/src/state.rs (dns_name/dns_suffix on the load-balancer record) and src/ops/describe_load_balancers.rs.

Cause 2 of 2 — CreateLoadBalancer answers an unreachable DNSName (fixture elbclassic-load-balancer-lifecycle-create-describe-delete, step task#4).
AWS: Smithy: CreateLoadBalancer output CreateAccessPointOutput$DNSName (target DNSName) — the operation's only output member, documented as the DNS name of the load balancer; the same reachability requirement as Cause 1. Recorded behavior only for the literal value ('<shaped-not-asserted>'): the recording asserts the returned host is one the serving endpoint owns.
Emulator: HTTP 200 with DNSName = internal-<name>-<10 digits>.<region>.elb.amazonaws.com (verdict host_not_own) — the create handler stores the minted real-AWS host and returns it verbatim, and stands up no listener for it.
Why: the create call is where a client learns the only address it will ever use for the load balancer; handing back a host the emulator does not serve makes every subsequent data-plane step impossible, and the failure surfaces far from the call that caused it.
Fix: the CreateLoadBalancer handler and the state it writes — mint and store a reachable address for the new load balancer and return that, so Cause 1's describe path reports the same reachable value.
Traces: elbclassic-load-balancer-lifecycle-create-describe-delete#task#4. Code: services/service-com-amazonaws-elasticloadbalancing-elasticloadbalancing-v7/src/ops/create_load_balancer.rs.

Related (not covering these): open behavioral issue 20261109T141728Z-elb-classic-the-published-dnsname-is-a-real.md describes the same defect from the behavioral suite; it carries behavioral check ids only and no wire replay key, so neither of these fixture steps is gated by it. The ELBv2 DNSName issues (e.g. 20261109T132410Z-acm-elbv2-the-alb-s-published-dnsname-is.md) are a different crate and code path.

## Evidence
Cause 1 — DescribeLoadBalancers, fixture elbclassic-create-load-balancer-validation-and-duplicate-name step task#5 (expected status 200, actual 200; cluster of 9 steps).
Expected (AWS): LoadBalancerDescriptions.member.DNSName is an address the caller can resolve and connect to — one of the serving endpoint's own hosts; the recorded response carries no CanonicalHostedZoneName member for this load balancer.
Actual (emulator), verbatim from replay:
  $.DescribeLoadBalancersResponse.DescribeLoadBalancersResult.LoadBalancerDescriptions.member.DNSName: host_not_own (expected '<shaped-not-asserted>', got 'internal-sim2real-probe-clb-8bd717b0-5258351275.us-west-2.elb.amazonaws.com')
  $.DescribeLoadBalancersResponse.DescribeLoadBalancersResult.LoadBalancerDescriptions.member.CanonicalHostedZoneName: host_not_own (expected None, got 'internal-sim2real-probe-clb-8bd717b0-5258351275.us-west-2.elb.amazonaws.com')
The requirement is the host's reachability and the member's presence; the load-balancer name, the 10-digit suffix and the region in the quoted value are per-run/environment values and are not part of it.

Cause 2 — CreateLoadBalancer, fixture elbclassic-load-balancer-lifecycle-create-describe-delete step task#4 (expected status 200, actual 200).
Expected (AWS): CreateLoadBalancerResult.DNSName is a reachable address of the new load balancer.
Actual (emulator), verbatim from replay:
  $.CreateLoadBalancerResponse.CreateLoadBalancerResult.DNSName: host_not_own (expected '<shaped-not-asserted>', got 'internal-sim2real-probe-clb-a451cb52-9120689121.us-west-2.elb.amazonaws.com')
Again the generated name/suffix are per-run values; the defect is that the host is not one the emulator serves.

Model evidence: CreateAccessPointOutput$DNSName, LoadBalancerDescription$DNSName and LoadBalancerDescription$CanonicalHostedZoneName all target DNSName and are documented 'The DNS name of the load balancer'; LoadBalancerDescription$Scheme documents that the name 'resolves to' a public or private IP address — i.e. an address, not a label.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire fixture corpus (trace set simparity-wire-aws-v1).
2. Cause 2: replay fixture elbclassic-load-balancer-lifecycle-create-describe-delete and stop at step task#4 (CreateLoadBalancer, internal scheme, HTTP:80 listener). Read CreateLoadBalancerResult.DNSName and observe it is <name>-<10 digits>.<region>.elb.amazonaws.com (internal- prefixed for the internal scheme) — a real-AWS host, not one the emulator serves. The step is reported host_not_own.
3. Cause 1: continue the same fixture to step task#5, or replay elbclassic-create-load-balancer-validation-and-duplicate-name to step task#5, and call DescribeLoadBalancers for that load balancer. Observe LoadBalancerDescriptions.member.DNSName carries the same unreachable host and CanonicalHostedZoneName is populated with it although the recorded response omits the member.
4. Attempt an unsigned HTTP GET to http://<DNSName>/ — the host is not an address the emulator serves or resolves.
5. Expected: both operations report a DNSName the emulator serves (its own host/port or a per-resource listener for the load balancer's configured listener ports), the listener answers there, and CanonicalHostedZoneName is present only where AWS reports it.
6. The same DescribeLoadBalancers cluster reproduces on elbclassic-health-check-configuration-round-trip-and-validation#task#6/#task#10, elbclassic-listener-lifecycle-duplicate-and-idempotent-delete#task#6/#task#10, elbclassic-load-balancer-lifecycle-create-describe-delete#task#5, elbclassic-policy-lifecycle-cookie-stickiness-and-generic-policy#task#12 and elbclassic-subnet-attach-detach-and-security-group-reassignment#task#9/#task#13.
