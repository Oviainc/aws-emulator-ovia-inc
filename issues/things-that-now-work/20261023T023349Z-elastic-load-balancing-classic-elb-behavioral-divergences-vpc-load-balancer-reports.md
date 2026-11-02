Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elb.enable-availability-zones-on-a-vpc-load-balancer-rejected elb.health-check-timeout-not-below-interval-rejected elb.duplicate-listener-port-with-different-backend-rejected elb.duplicate-load-balancer-name-rejected
Issue-Shape-Id: com.amazonaws.elasticloadbalancing#ElasticLoadBalancing_v7
Issue-Key: elb.enable-availability-zones-on-a-vpc-load-balancer-rejected|run#6|AssertionError|assert before_azs == [azs[0]] and before_subnets == [subnet_ids[0]], (
Issue-Key: elb.health-check-timeout-not-below-interval-rejected|run#10|AssertionError|assert equal_code == "ValidationError", (
Issue-Key: elb.duplicate-listener-port-with-different-backend-rejected|run#14|AssertionError|assert message == expected_message, (
Issue-Key: elb.duplicate-load-balancer-name-rejected|run#11|AssertionError|assert message == expected, (
Issue-Commit: 1b15cfcf83a9fafcaa69f938ed17066b612a8ba0

# Classic ELB behavioral divergences: VPC load balancer reports no AvailabilityZones, cross-field health-check refusal uses ValidationException, and two conflict messages use non-AWS templates (4 causes)

> _Note: this issue was identified against the code base at commit `1b15cfcf83a9fafcaa69f938ed17066b612a8ba0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeLoadBalancers

## Description
FOUR net-new root causes in the Classic Elastic Load Balancing (2012-06-01) emulator, all observed by the behavioral suite simparity-behavioral-aws-v1 against a Classic ELB created with Subnets/SecurityGroups/Scheme=internal in a purpose-built VPC. Each cause below must be closed before this issue is submitted.

Context on what is NOT this issue: the open issue 20261022T192234Z-elasticloadbalancing-classic-elb-modeled-refusals-answer-500-json-instead.md ("Classic ELB modeled refusals answer 500 JSON instead of 400 awsQuery errors") covers the earlier state in which these refusal paths answered HTTP 500 application/x-amz-json-1.1. The refusals now DO arrive as parseable awsQuery client errors (botocore surfaced DuplicateListener / DuplicateLoadBalancerName / ValidationException), so what is reported here is what those now-reachable paths get WRONG, plus a state-representation defect on the success path that that issue does not mention. None of the checks below were marked OWNED or ALREADY FILED by the replay.

Cause 1 of 4: DescribeLoadBalancers reports an empty AvailabilityZones list for a load balancer created from Subnets.
AWS: Smithy (com-amazonaws-elasticloadbalancing-elasticloadbalancing-v7) CreateLoadBalancer input takes either AvailabilityZones or Subnets; DescribeLoadBalancers output is LoadBalancerDescriptions, whose LoadBalancerDescription carries BOTH Subnets and AvailabilityZones. Recorded behavior (behavioral check, us-west-2): a VPC load balancer created with exactly one subnet is described with that subnet AND with the availability zone that subnet lives in, i.e. zone membership is derived from the attached subnets; after AttachLoadBalancerToSubnets adds a second subnet the description lists both subnets and both zones.
Emulator: the description carries the correct Subnets (['subnet-...020']) but AvailabilityZones is [] — the zone of the attached subnet is never derived or stored.
Why: a client that places targets, pins DNS, or asserts multi-AZ coverage by reading LoadBalancerDescription.AvailabilityZones sees a load balancer in no zone at all; cross-zone/zone-count logic and any "is this LB multi-AZ" guard silently takes the wrong branch, and an attach/detach flow cannot observe its own effect on zone membership.
Fix: CreateLoadBalancer's state write and AttachLoadBalancerToSubnets/DetachLoadBalancerFromSubnets in the Classic ELB crate must resolve each attached subnet to its EC2 availability zone and keep the load balancer's zone set in sync; DescribeLoadBalancers' serializer must emit it.
Traces: elastic-load-balancing.enable-availability-zones-on-a-vpc-load-balancer-rejected (fails at the pre-condition, before the EnableAvailabilityZonesForLoadBalancer refusal is even probed).

Cause 2 of 4: ConfigureHealthCheck's timeout-vs-interval refusal uses error code ValidationException instead of ValidationError.
AWS: Smithy ConfigureHealthCheck models only AccessPointNotFoundException (awsQueryError LoadBalancerNotFound, 400); the HealthCheck shape constrains Interval and Timeout only individually, and Timeout's documentation states the value must be less than Interval. The cross-member rule is therefore a service rule, refused with HTTP 400 and awsQuery error code `ValidationError` (recorded behavior, us-west-2; the recorded message 'HealthCheck timeout must be less than interval. ' is request/rule-derived, so the requirement is a ValidationError naming the timeout-vs-interval constraint — the exact text and its trailing space are an example).
Emulator: the same request (Interval 10 / Timeout 10, and Interval 10 / Timeout 11) is refused with error code `ValidationException`.
Why: Classic ELB is an awsQuery service whose client-side validation code is `ValidationError`; `ValidationException` is the JSON-protocol spelling and matches no code a Classic ELB client branches on, so error-code dispatch (retry-vs-fix, or mapping to a user-facing validation message) falls through to the generic unknown-error path.
Fix: ConfigureHealthCheck's cross-field validation in the Classic ELB crate must raise the awsQuery ValidationError code (400) rather than the generic ValidationException.
Traces: elastic-load-balancing.health-check-timeout-not-below-interval-rejected.

Cause 3 of 4: the DuplicateListener message does not follow AWS's template.
AWS: Smithy CreateLoadBalancerListeners models DuplicateListenerException, errorKind client, httpError 400, awsQueryError code `DuplicateListener`, documented as "A listener already exists for the specified load balancer name and port, but with a different instance port, protocol, or SSL certificate." The recorded message is built from the request and that documented rule: 'A listener already exists for <LoadBalancerName> with LoadBalancerPort <port>, but with a different InstancePort, Protocol, or SSLCertificateId'. The requirement is that rule — name the load balancer and the conflicting LoadBalancerPort and state that the conflict is in InstancePort/Protocol/SSLCertificateId; the literal string is an example.
Emulator: the code and status are right, but the message echoes the EXISTING listener's settings instead of naming the differing fields: 'A listener already exists for <lb> on LoadBalancerPort 80, but with Protocol HTTP, InstanceProtocol HTTP and InstancePort 80'.
Why: the message is the only place AWS reports WHICH attribute conflicts; a client (or operator log) reading it is told the stored configuration rather than the reason for the refusal, so a conflicting listener definition cannot be localised from the response.
Fix: CreateLoadBalancerListeners' conflict-detection error construction in the Classic ELB crate.
Traces: elastic-load-balancing.duplicate-listener-port-with-different-backend-rejected.

Cause 4 of 4: the DuplicateLoadBalancerName message omits the documented 'configured with different parameters' rule.
AWS: Smithy CreateLoadBalancer models DuplicateAccessPointNameException, errorKind client, httpError 400, awsQueryError code `DuplicateLoadBalancerName`. Recorded behavior (us-west-2): the message is request-derived and names the rule that produced it — 'Load Balancer named <LoadBalancerName> already exists and it is configured with different parameters.' (the per-account/per-region name namespace plus the differing-configuration test that distinguishes it from an idempotent re-create). The requirement is that rule; the literal text is an example.
Emulator: code and status are right, but the message is the bare 'Load balancer named <lb> already exists' — it drops the differing-parameters clause (and lowercases the resource name).
Why: Classic ELB's CreateLoadBalancer is idempotent for a byte-identical re-create and a conflict only for a differing one; a client that distinguishes "already mine, carry on" from "name taken by a different configuration" has nothing in the response to distinguish them once the clause is gone.
Fix: CreateLoadBalancer's duplicate-name detection error construction in the Classic ELB crate.
Traces: elastic-load-balancing.duplicate-load-balancer-name-rejected.

## Evidence
Behavioral suite simparity-behavioral-aws-v1: 1/17 passed, 4 mismatched, 12 errored (11 of the errors blocked by UNIMPLEMENTED EC2 CreateInternetGateway and 1 by UNIMPLEMENTED CreateLBCookieStickinessPolicy — both ALREADY FILED and excluded here). All four mismatches are the causes above.

Cause 1 — elastic-load-balancing.enable-availability-zones-on-a-vpc-load-balancer-rejected:
  AssertionError: the load balancer was placed in zones [] / subnets ['subnet-00000000000000020'], expected exactly ['us-west-2a'] / ['subnet-00000000000000020']
  (the check creates the LB with Subnets=[subnet in us-west-2a], SecurityGroups=[sg], Scheme='internal', then reads DescribeLoadBalancers.LoadBalancerDescriptions[0].AvailabilityZones/Subnets)

Cause 2 — elastic-load-balancing.health-check-timeout-not-below-interval-rejected:
  AssertionError: a health check with Timeout == Interval failed with 'ValidationException', expected ValidationError
  (request: ConfigureHealthCheck HealthCheck={Target 'HTTP:80/ok', Interval 10, Timeout 10, UnhealthyThreshold 2, HealthyThreshold 2}; the preceding Timeout 4 / Interval 10 call was accepted and echoed correctly)

Cause 3 — elastic-load-balancing.duplicate-listener-port-with-different-backend-rejected:
  AssertionError: DuplicateListener carried the message 'A listener already exists for <lb> on LoadBalancerPort 80, but with Protocol HTTP, InstanceProtocol HTTP and InstancePort 80', expected 'A listener already exists for <lb> with LoadBalancerPort 80, but with a different InstancePort, Protocol, or SSLCertificateId'
  (error code DuplicateListener asserted and matched first; request re-declares LoadBalancerPort 80 with InstancePort 8080)

Cause 4 — elastic-load-balancing.duplicate-load-balancer-name-rejected:
  AssertionError: creating a second load balancer named '<lb>' failed with message 'Load balancer named <lb> already exists', expected 'Load Balancer named <lb> already exists and it is configured with different parameters.'
  (error code DuplicateLoadBalancerName asserted and matched first; request reuses the name with Listeners=[TCP 81 -> TCP 8080])

Model evidence (smithy_query query=operation, model services/service-com-amazonaws-elasticloadbalancing-elasticloadbalancing-v7/smithy-model/com-amazonaws-elasticloadbalancing-elasticloadbalancing-v7.json):
  CreateLoadBalancer: input AvailabilityZones (list) | Subnets (list); errors include DuplicateAccessPointNameException (client, httpError 400, awsQueryError code DuplicateLoadBalancerName/400)
  CreateLoadBalancerListeners: errors include DuplicateListenerException (client, httpError 400, awsQueryError code DuplicateListener/400)
  ConfigureHealthCheck: input LoadBalancerName + HealthCheck (both required); only modeled error AccessPointNotFoundException (awsQueryError LoadBalancerNotFound/400) — no cross-field error, so the timeout<interval rule is a service rule
  DescribeLoadBalancers: output LoadBalancerDescriptions (LoadBalancerDescription carries both Subnets and AvailabilityZones)
  EnableAvailabilityZonesForLoadBalancer: output AvailabilityZones; only modeled error AccessPointNotFoundException

## Steps to Reproduce
1. git_clone the repo; build and boot the emulator from the checkout.
2. Run the behavioral suite (simparity-behavioral-aws-v1).
3. Cause 1: create an EC2 VPC + one subnet in an available AZ + a security group, then elb.create_load_balancer(LoadBalancerName=<n>, Listeners=[HTTP 80 -> HTTP 80], Subnets=[<subnet>], SecurityGroups=[<sg>], Scheme='internal') and elb.describe_load_balancers(LoadBalancerNames=[<n>]). Observe AvailabilityZones == [] instead of the subnet's zone; repeat after attach_load_balancer_to_subnets with a second subnet and observe the zone set still does not follow the subnets. Re-replay elastic-load-balancing.enable-availability-zones-on-a-vpc-load-balancer-rejected.
4. Cause 2: on that load balancer call elb.configure_health_check(HealthCheck={'Target':'HTTP:80/ok','Interval':10,'Timeout':10,'UnhealthyThreshold':2,'HealthyThreshold':2}) (and again with Timeout 11). Observe Error.Code 'ValidationException'; expected 400 with awsQuery Code 'ValidationError' naming the timeout-vs-interval rule. Re-replay elastic-load-balancing.health-check-timeout-not-below-interval-rejected.
5. Cause 3: with an HTTP 80 -> 80 listener in place, call elb.create_load_balancer_listeners(Listeners=[{'Protocol':'HTTP','LoadBalancerPort':80,'InstanceProtocol':'HTTP','InstancePort':8080}]). Code DuplicateListener is correct; observe the message reports the stored listener instead of naming the differing InstancePort/Protocol/SSLCertificateId. Re-replay elastic-load-balancing.duplicate-listener-port-with-different-backend-rejected.
6. Cause 4: call elb.create_load_balancer again with the SAME LoadBalancerName and a different listener set (TCP 81 -> TCP 8080). Code DuplicateLoadBalancerName is correct; observe the message omits the 'already exists and it is configured with different parameters' rule. Re-replay elastic-load-balancing.duplicate-load-balancer-name-rejected.
7. All four causes must be closed (and their four checks pass) before this issue is submitted.
