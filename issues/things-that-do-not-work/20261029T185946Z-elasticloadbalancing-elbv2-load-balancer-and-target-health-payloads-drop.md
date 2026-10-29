Issue-Source: trace-sweep-divergence
Issue-Trace: elasticloadbalancing-alb-lifecycle-register-targets
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticloadbalancing-alb-lifecycle-register-targets elasticloadbalancing-set-security-groups-round-trip elbv2env-alb-create-provisioning-and-subnet-reconfig elbv2err-nlb-elastic-ip-rejections elbv2-capacity-reservation-default-state-on-new-load-balancer elbv2-describe-target-health-unregistered-target-unused
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elasticloadbalancing|DescribeLoadBalancers|2xx->2xx|$.DescribeLoadBalancersResponse.DescribeLoadBalancersResult.LoadBalancers.member.EnablePrefixForIpv6SourceNat
Issue-Key: elasticloadbalancing|DescribeLoadBalancers|2xx->2xx|$.DescribeLoadBalancersResponse.DescribeLoadBalancersResult.LoadBalancers.member.SecurityGroups
Issue-Key: elasticloadbalancing|CreateLoadBalancer|2xx->2xx|$.CreateLoadBalancerResponse.CreateLoadBalancerResult.LoadBalancers.member.DNSName
Issue-Key: elasticloadbalancing|CreateLoadBalancer|2xx->2xx|$.CreateLoadBalancerResponse.CreateLoadBalancerResult.LoadBalancers.member.SecurityGroups
Issue-Key: elasticloadbalancing|DescribeLoadBalancers|2xx->2xx|$.DescribeLoadBalancersResponse.DescribeLoadBalancersResult.LoadBalancers.member.DNSName
Issue-Key: elasticloadbalancing|CreateLoadBalancer|2xx->2xx|$.CreateLoadBalancerResponse.CreateLoadBalancerResult.LoadBalancers.member.EnablePrefixForIpv6SourceNat
Issue-Key: elasticloadbalancing|DescribeCapacityReservation|2xx->2xx|$.DescribeCapacityReservationResponse.DescribeCapacityReservationResult.DecreaseRequestsRemaining
Issue-Key: elasticloadbalancing|DescribeTargetHealth|2xx->2xx|$.DescribeTargetHealthResponse.DescribeTargetHealthResult.TargetHealthDescriptions.member.HealthCheckPort
Issue-Commit: 611eef386f1100e1b77f5a1515bab52b7a7a6aeb

# ELBv2 load balancer and target health payloads drop defaulted members and mis-form DNS names

> _Note: this issue was identified against the code base at commit `611eef386f1100e1b77f5a1515bab52b7a7a6aeb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeLoadBalancers

## Description
The LoadBalancer, CapacityReservation and TargetHealthDescription serializers omit members AWS always returns at their default value, serialize SecurityGroups as an empty scalar instead of a member list, and emit a DNS name whose form the recorded shape rejects. 8 causes.

Cause 1 of 8: DescribeLoadBalancers — LoadBalancers.member.EnablePrefixForIpv6SourceNat absent and SecurityGroups serialized as the empty string.
AWS: Smithy LoadBalancer.EnablePrefixForIpv6SourceNat -> EnablePrefixForIpv6SourceNatEnum, documented 'The default value is off', and AWS returns 'off' on every load balancer. Smithy LoadBalancer.SecurityGroups -> SecurityGroups (list), serialized in the awsQuery form as SecurityGroups.member with one entry per group id.
Emulator: the EnablePrefixForIpv6SourceNat element is missing and SecurityGroups is '' even though the load balancer has groups attached.
Why: a client reading enable_prefix_for_ipv6_source_nat() gets None instead of Off, and a client reading security_groups() gets an empty list for a load balancer that has them — breaking any security-group reconciliation.
Fix: the LoadBalancer serializer/state (default EnablePrefixForIpv6SourceNat, SecurityGroups list serialization).
Traces: elasticloadbalancing-alb-lifecycle-register-targets#task#6 (also elasticloadbalancing-set-security-groups-round-trip#task#3).

Cause 2 of 8: DescribeLoadBalancers after SetSecurityGroups — EnablePrefixForIpv6SourceNat absent. Same AWS rule and fix as cause 1. Traces: elasticloadbalancing-set-security-groups-round-trip#task#5.

Cause 3 of 8: CreateLoadBalancer (ALB) — SecurityGroups serialized as '' instead of a member list, and DNSName fails the recorded shape.
AWS: Smithy LoadBalancer.DNSName; the recording shape-checks the DNS name rather than asserting a literal, so the form is the requirement, not the generated id. Hypothesis: the emulator's host form diverges from the per-type AWS form (Application Load Balancer '<[internal-]name>-<id>.<region>.elb.amazonaws.com'), which is what the shape encodes.
Emulator: DNSName 'internal-<name>-<id>.<region>.elb.amazonaws.com' rejected by the shape, SecurityGroups ''.
Fix: the CreateLoadBalancer response serializer and the DNS-name composition in load balancer state.
Traces: elbv2env-alb-create-provisioning-and-subnet-reconfig#task#5.

Cause 4 of 8: DescribeLoadBalancers for the same ALB — DNSName shape failure, EnablePrefixForIpv6SourceNat absent, SecurityGroups ''. Same causes as 1 and 3. Traces: elbv2env-alb-create-provisioning-and-subnet-reconfig#task#6 (also #task#15).

Cause 5 of 8: CreateLoadBalancer (Network Load Balancer) — DNSName shape failure and EnablePrefixForIpv6SourceNat absent.
AWS: a Network Load Balancer's DNS name uses the 'elb.<region>.amazonaws.com' suffix ('<[internal-]name>-<id>.elb.<region>.amazonaws.com'), unlike the ALB form; the recording shape-checks that form. EnablePrefixForIpv6SourceNat is 'off' by default — it is specifically an NLB member, so its absence matters most here.
Emulator: 'internal-<name>-<id>.<region>.elb.amazonaws.com' (the ALB suffix order) and no EnablePrefixForIpv6SourceNat element.
Fix: load balancer DNS-name composition keyed on LoadBalancerTypeEnum, plus the default member.
Traces: elbv2err-nlb-elastic-ip-rejections#task#6.

Cause 6 of 8: DescribeLoadBalancers for the same NLB — same two divergences. Traces: elbv2err-nlb-elastic-ip-rejections#task#7.

Cause 7 of 8: DescribeCapacityReservation on a freshly created load balancer — DescribeCapacityReservationResult.DecreaseRequestsRemaining absent.
AWS: returns the member with the account's default allowance for the day (recorded value 2) on a load balancer that has no capacity reservation.
Emulator: the element is missing.
Fix: the DescribeCapacityReservation handler/state default.
Traces: elbv2-capacity-reservation-default-state-on-new-load-balancer#task#2.

Cause 8 of 8: DescribeTargetHealth — TargetHealthDescriptions.member.HealthCheckPort absent.
AWS: Smithy TargetHealthDescription.HealthCheckPort ('The port to use to connect with the target'); AWS fills it from the target group's effective health check port (the recorded value is the target group's configured port) for every described target, including an unregistered/unused one.
Emulator: the element is missing.
Fix: the DescribeTargetHealth serializer — resolve the effective health check port per target.
Traces: elbv2-describe-target-health-unregistered-target-unused#task#3.

## Evidence
All causes: 200 expected and 200 returned; only members diverge.
EnablePrefixForIpv6SourceNat: expected 'off', got None (causes 1, 2, 4, 5, 6).
SecurityGroups: expected {'member': <shaped>}, got '' (causes 1, 3, 4).
DNSName: expected <shaped>, got 'internal-sim2real-...-<id>.us-west-2.elb.amazonaws.com' (causes 3, 4, 5, 6).
DecreaseRequestsRemaining: expected '2', got None (cause 7).
HealthCheckPort: expected '8080' (the target group's port), got None (cause 8).

## Steps to Reproduce
Replay the six fixtures named under Traces and diff the listed steps against their recorded responses (replay_wire_corpus, trace set simparity-wire-aws-v1).
