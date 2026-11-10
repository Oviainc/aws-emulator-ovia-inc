Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elb.enable-availability-zones-on-a-vpc-load-balancer-rejected elb.health-check-timeout-not-below-interval-rejected
Issue-Shape-Id: com.amazonaws.elasticloadbalancing#ElasticLoadBalancing_v7
Issue-Key: elb.enable-availability-zones-on-a-vpc-load-balancer-rejected|run#8|AssertionError|assert code == "ValidationError", (
Issue-Key: elb.health-check-timeout-not-below-interval-rejected|run#11|AssertionError|assert equal_message == "HealthCheck timeout must be less than interval. ", (
Issue-Commit: e5bbef2d2760924537f10016f55a30b12c0b7775

# ELB Classic: EnableAvailabilityZonesForLoadBalancer answers 501 NotImplemented instead of the ValidationError refusal, and the ConfigureHealthCheck timeout/interval message drops AWS's trailing space

> _Note: this issue was identified against the code base at commit `e5bbef2d2760924537f10016f55a30b12c0b7775`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
EnableAvailabilityZonesForLoadBalancer

## Description
Two net-new root causes in the ELB Classic (elasticloadbalancing, 2012-06-01) control plane, both grounded in behavioral checks that reached the emulator and got a wrong refusal. 2 causes.

Cause 1 of 2: EnableAvailabilityZonesForLoadBalancer is an unimplemented scaffold stub, so the EC2-Classic-era zone-enable call on a VPC load balancer is answered with the scaffold's 501 NotImplemented rather than the service's refusal. Failing check: elastic-load-balancing.enable-availability-zones-on-a-vpc-load-balancer-rejected.
AWS: Smithy: EnableAvailabilityZonesForLoadBalancer (input AddAvailabilityZonesInput: LoadBalancerName @required AccessPointName, AvailabilityZones @required; output AvailabilityZones) models exactly one error, AccessPointNotFoundException (awsQueryError code LoadBalancerNotFound, httpError 400); the operation doc states "Adds the specified Availability Zones to the set of Availability Zones for the specified load balancer in EC2-Classic or a default VPC. For load balancers in a non-default VPC, use AttachLoadBalancerToSubnets." The refusal for a load balancer in a NON-default VPC is a service rule the model does not declare: recorded behavior (us-west-2, measured by the check) is HTTP 400 with error code `ValidationError` and the message, whose text is entirely fixed with no variable part, `You cannot enable Availability Zones for ELBs in a non default VPC`. The refusal must have no effect: the load balancer's AvailabilityZones and Subnets, as DescribeLoadBalancers reports them, are unchanged afterwards, and the same zone is reachable through AttachLoadBalancerToSubnets, which returns the full subnet set and makes the zone appear in DescribeLoadBalancers.
Emulator: the call fails with HTTP 501, error code `NotImplemented`, message `operation EnableAvailabilityZonesForLoadBalancer is scaffolded but not implemented in this emulator` — the handler body is still the generated stub that returns `Scaffolded::not_implemented(...)` and the file still carries the `SCAFFOLDED, NOT IMPLEMENTED` marker.
Why: a client that calls the EC2-Classic zone API against a VPC load balancer expects a 400 ValidationError it can recognize and then fall back to AttachLoadBalancerToSubnets. Against the emulator it sees a 501 instead: botocore and the SDKs classify that as a server-side/transient failure, so such a client retries a call that can never succeed rather than taking its VPC fallback path, and a client that asserts on the ValidationError code/message cannot distinguish "this API does not apply to VPC load balancers" from "the service is broken". For a load balancer in EC2-Classic or a DEFAULT VPC the operation is a legitimate success path and is unavailable entirely.
Fix: the operation's handler — implement zone membership there: resolve the load balancer in the request's region (AccessPointNotFoundException / LoadBalancerNotFound, 400, when no record holds the name), and for a record that belongs to a non-default VPC refuse with the 400 ValidationError and the fixed message above WITHOUT mutating stored zones or subnets; delete the SCAFFOLDED marker when done. The non-VPC path returns the updated AvailabilityZones from the stored record so a following DescribeLoadBalancers agrees.
Traces: elastic-load-balancing.enable-availability-zones-on-a-vpc-load-balancer-rejected (the check's placement assertions before the refusal passed, so only the refusal itself diverged). File/line citation: services/service-com-amazonaws-elasticloadbalancing-elasticloadbalancing-v7/src/ops/enable_availability_zones_for_load_balancer.rs:156.

Cause 2 of 2: ConfigureHealthCheck's timeout-not-below-interval refusal carries the right status and code but a message one character short of AWS's: the trailing space is missing. Failing check: elastic-load-balancing.health-check-timeout-not-below-interval-rejected.
AWS: Smithy: ConfigureHealthCheck (HealthCheck @required; Interval and Timeout are constrained only individually) models only AccessPointNotFoundException; the cross-field rule is documented on HealthCheck$Timeout ("This value must be less than the Interval value") and is enforced as a service rule. Recorded behavior (us-west-2): a Timeout equal to OR greater than Interval is refused with HTTP 400, error code `ValidationError`, and message `HealthCheck timeout must be less than interval. ` — fixed text with a TRAILING SPACE after the period and no variable part (it states the rule, not the submitted values). The boundary is strict inequality: Timeout 9 / Interval 10 is accepted.
Emulator: same status and code, but Message = `HealthCheck timeout must be less than interval.` (no trailing space). The refusal itself and the no-partial-apply behavior are correct; only the message text diverges.
Why: the error message is part of the wire contract. A client, test fixture or recorded-response comparison that matches this refusal by exact message text — the only way to tell this rule apart from the other ValidationError refusals this operation emits, since the code is shared — fails to match, and any consumer diffing against a real AWS recording sees a message mismatch on every rejected health check.
Fix: the ConfigureHealthCheck handler's cross-field validation — the refusal string constant must reproduce AWS's text exactly, including the trailing space.
Traces: elastic-load-balancing.health-check-timeout-not-below-interval-rejected (both the Timeout == Interval and the Timeout > Interval refusals). File/line citation: services/service-com-amazonaws-elasticloadbalancing-elasticloadbalancing-v7/src/ops/configure_health_check.rs:224 (HEALTH_CHECK_TIMEOUT_RELATION), raised at lines 150-154.

Neither cause is covered by an open issue. The two open ELB Classic issues (20261029T172824Z-elasticloadbalancing-elb-classic-describe-responses-serve-static-defaults-instead.md and ...-elb-classic-error-replies-carry-the-generic-error.md) concern DescribeLoadBalancers state and the Detach/Attach/ApplySecurityGroups message bodies, not these two operations; the open ELBv2 issues concern a different API surface.

## Evidence
Behavioral suite run: elastic-load-balancing 3/16 (19%), 13 failed (11 of them blocked by already-filed unimplemented stubs CreateInternetGateway and DescribeLoadBalancerPolicies and skipped here).

Cause 1 — [mismatch] elastic-load-balancing.enable-availability-zones-on-a-vpc-load-balancer-rejected:
  AssertionError: enabling availability zone 'us-west-2b' on a VPC load balancer failed with 'NotImplemented', expected ValidationError
  (check line 176: `assert code == "ValidationError"`; the next assertion expects message 'You cannot enable Availability Zones for ELBs in a non default VPC'). The underlying response is HTTP 501 'operation EnableAvailabilityZonesForLoadBalancer is scaffolded but not implemented in this emulator'. Source: enable_availability_zones_for_load_balancer.rs:156 returns `Scaffolded::not_implemented("EnableAvailabilityZonesForLoadBalancer")`, file header line 4 still reads 'SCAFFOLDED, NOT IMPLEMENTED'. Expected fixed text: the whole sentence; no variable parts. Note the check's earlier steps succeeded — CreateVpc/CreateSubnet/CreateSecurityGroup/CreateLoadBalancer and the DescribeLoadBalancers placement assertion all passed — so the emulator is serving this surface.

Cause 2 — [mismatch] elastic-load-balancing.health-check-timeout-not-below-interval-rejected:
  AssertionError: a health check with Timeout == Interval failed with message 'HealthCheck timeout must be less than interval.', expected 'HealthCheck timeout must be less than interval. ' (with its trailing space)
  Code matched (ValidationError), status matched (400); only the trailing space differs. Fixed text = the whole sentence including the trailing space; no values from the request appear in it. Source: configure_health_check.rs:224 defines HEALTH_CHECK_TIMEOUT_RELATION = "HealthCheck timeout must be less than interval." and lines 150-154 raise it for `timeout >= interval`. The check's positive control (Target HTTP:80/ok, Interval 10, Timeout 4), the default TCP:80 health check, the no-partial-apply read-back and the accepted Timeout 9 / Interval 10 boundary all behaved correctly.

## Steps to Reproduce
Cause 1:
1. Create a non-default VPC with two subnets in two available AZs and a security group (EC2).
2. CreateLoadBalancer (ELB Classic) with an HTTP:80 listener, Scheme=internal, placed in the FIRST subnet only.
3. Call EnableAvailabilityZonesForLoadBalancer with LoadBalancerName=<that name> and AvailabilityZones=[<the second AZ>].
   Expected: HTTP 400, Error.Code=ValidationError, Error.Message='You cannot enable Availability Zones for ELBs in a non default VPC', and DescribeLoadBalancers still reports only the original zone/subnet.
   Actual: HTTP 501, Error.Code=NotImplemented, Error.Message='operation EnableAvailabilityZonesForLoadBalancer is scaffolded but not implemented in this emulator'.
4. Confirm the positive control still works: AttachLoadBalancerToSubnets with the second subnet returns both subnets and DescribeLoadBalancers then reports both zones.

Cause 2:
1. Create a VPC classic load balancer as above (any subnets).
2. ConfigureHealthCheck with HealthCheck={Target:'HTTP:80/ok', Interval:10, Timeout:4, UnhealthyThreshold:2, HealthyThreshold:2} — accepted.
3. ConfigureHealthCheck with the same body but Timeout:10 (equal to Interval), then again with Timeout:11.
   Expected both times: HTTP 400, Error.Code=ValidationError, Error.Message='HealthCheck timeout must be less than interval. ' (note the trailing space).
   Actual both times: the same status and code with Error.Message='HealthCheck timeout must be less than interval.' (trailing space absent).
4. Re-replay both failing checks to confirm: elastic-load-balancing.enable-availability-zones-on-a-vpc-load-balancer-rejected and elastic-load-balancing.health-check-timeout-not-below-interval-rejected.
