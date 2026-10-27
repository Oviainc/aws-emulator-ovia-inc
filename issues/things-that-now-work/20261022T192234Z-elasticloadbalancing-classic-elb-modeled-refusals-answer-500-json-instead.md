Issue-Source: trace-sweep-divergence
Issue-Trace: elbclassic-create-load-balancer-validation-and-duplicate-name
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elbclassic-create-load-balancer-validation-and-duplicate-name elbclassic-load-balancer-lifecycle-create-describe-delete elbclassic-health-check-configuration-round-trip-and-validation elbclassic-listener-lifecycle-duplicate-and-idempotent-delete elbclassic-load-balancer-attributes-round-trip-and-range-validation elbclassic-subnet-attach-detach-and-security-group-reassignment
Issue-Shape-Id: com.amazonaws.elasticloadbalancing#ElasticLoadBalancing_v7
Issue-Key: elasticloadbalancing|CreateLoadBalancer|4xx->5xx|$.ErrorResponse
Issue-Key: elasticloadbalancing|CreateLoadBalancer|4xx->5xx|$headers.content-type
Issue-Key: elasticloadbalancing|CreateLoadBalancer|4xx->5xx|status
Issue-Key: elasticloadbalancing|DescribeLoadBalancers|4xx->5xx|$.ErrorResponse
Issue-Key: elasticloadbalancing|DescribeLoadBalancers|4xx->5xx|$headers.content-type
Issue-Key: elasticloadbalancing|DescribeLoadBalancers|4xx->5xx|status
Issue-Key: elasticloadbalancing|ConfigureHealthCheck|4xx->5xx|$.ErrorResponse
Issue-Key: elasticloadbalancing|ConfigureHealthCheck|4xx->5xx|$headers.content-type
Issue-Key: elasticloadbalancing|ConfigureHealthCheck|4xx->5xx|status
Issue-Key: elasticloadbalancing|CreateLoadBalancerListeners|4xx->5xx|$.ErrorResponse
Issue-Key: elasticloadbalancing|CreateLoadBalancerListeners|4xx->5xx|$headers.content-type
Issue-Key: elasticloadbalancing|CreateLoadBalancerListeners|4xx->5xx|status
Issue-Key: elasticloadbalancing|ModifyLoadBalancerAttributes|4xx->5xx|$.ErrorResponse
Issue-Key: elasticloadbalancing|ModifyLoadBalancerAttributes|4xx->5xx|$headers.content-type
Issue-Key: elasticloadbalancing|ModifyLoadBalancerAttributes|4xx->5xx|status
Issue-Key: elasticloadbalancing|AttachLoadBalancerToSubnets|4xx->5xx|$.ErrorResponse
Issue-Key: elasticloadbalancing|AttachLoadBalancerToSubnets|4xx->5xx|$headers.content-type
Issue-Key: elasticloadbalancing|AttachLoadBalancerToSubnets|4xx->5xx|status
Issue-Key: elasticloadbalancing|ApplySecurityGroupsToLoadBalancer|4xx->5xx|$.ErrorResponse
Issue-Key: elasticloadbalancing|ApplySecurityGroupsToLoadBalancer|4xx->5xx|$headers.content-type
Issue-Key: elasticloadbalancing|ApplySecurityGroupsToLoadBalancer|4xx->5xx|status
Issue-Commit: 17f16131651e3a76635c083c093a8581359ec045

# Classic ELB modeled refusals answer 500 JSON instead of 400 awsQuery errors (7 causes)

> _Note: this issue was identified against the code base at commit `17f16131651e3a76635c083c093a8581359ec045`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateLoadBalancer

## Description
SEVEN causes, all of them Classic Load Balancer refusal paths that must answer a 4xx awsQuery `ErrorResponse` in text/xml but instead answer HTTP 500 with `Content-Type: application/x-amz-json-1.1` and no ErrorResponse element. Each cause below must be closed before this issue is submitted.

SHARED ROOT CAUSE (the same one as the companion issue on the success paths): `Elastic Load Balancing` (Classic, 2012-06-01) and `Elastic Load Balancing v2` (2015-12-01) both declare `sigv4Name: elasticloadbalancing` / `endpointPrefix: elasticloadbalancing` (smithy_query service_info on both models), and both crates still carry the generated default `can_handle`, which claims any request whose resolved route key equals that signing name:
  services/service-com-amazonaws-elasticloadbalancing-elasticloadbalancing-v7/src/service.rs:76-84
  services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/src/service.rs:76-84
Two services therefore claim the same request. Hypothesis: that ambiguous claim is rendered as a generic 500 in the framework's default awsJson envelope, which is what the replay observes — an awsQuery service can never legitimately answer `application/x-amz-json-1.1`, and the Classic crate's own scaffolded handlers would have answered 501 text/xml had any been reached.

Cause 1 of 7: CreateLoadBalancer with an unknown subnet (fixture elbclassic-create-load-balancer-validation-and-duplicate-name, step task#9).
AWS: Smithy awsQuery. CreateLoadBalancer models SubnetNotFoundException, errorKind client, httpError 400, awsQueryError code `SubnetNotFound`, httpResponseCode 400, doc "One or more of the specified subnets do not exist." Wire shape: HTTP 400, `Content-Type: text/xml`, body `ErrorResponse/Error` with Type `Sender`, Code `SubnetNotFound`, plus `RequestId`. The recorded message is `InvalidSubnetID.NotFound` — an example, since neither the model nor AWS documents the text.
Emulator: HTTP 500, `application/x-amz-json-1.1`, no ErrorResponse element.
Why: a client cannot distinguish "you named a subnet that does not exist" (its own, non-retryable mistake) from a server fault, so it retries a request that can never succeed instead of correcting the subnet id.
Fix: the service's request-claiming seam (`can_handle` in the Classic crate's service.rs) and then CreateLoadBalancer's validation, which must emit SubnetNotFoundException.

Cause 2 of 7: DescribeLoadBalancers naming a load balancer that does not exist (fixture elbclassic-create-load-balancer-validation-and-duplicate-name, step task#10).
AWS: DescribeLoadBalancers models AccessPointNotFoundException, errorKind client, httpError 400, awsQueryError code `LoadBalancerNotFound`, httpResponseCode 400, doc "The specified load balancer does not exist." Wire shape: HTTP 400 text/xml, `ErrorResponse/Error` with Type `Sender`, Code `LoadBalancerNotFound`. The recorded message ("There is no ACTIVE Load Balancer named '<name>'") is built from the requested name, so the requirement is that the message names the load balancer asked for — not the recorded literal.
Emulator: HTTP 500, `application/x-amz-json-1.1`, no ErrorResponse.
Why: existence checks are impossible — a describe of a deleted or mistyped name looks like an outage, so a client cannot implement a wait-until-gone or a not-found branch.
Fix: `can_handle`, then DescribeLoadBalancers' lookup path, which must emit AccessPointNotFoundException for any requested name absent from the region's store.

Cause 3 of 7: ConfigureHealthCheck with timeout >= interval and other out-of-policy health checks (fixture elbclassic-health-check-configuration-round-trip-and-validation, step task#7; 3 affected steps).
AWS: Smithy HealthCheck declares Interval @range 5..300, Timeout @range 2..60, HealthyThreshold/UnhealthyThreshold @range 2..10, and Timeout's own documentation states "This value must be less than the Interval value." That cross-member rule is not expressible as a @range, and ConfigureHealthCheck models no specific error for it, so AWS refuses it with HTTP 400 text/xml, `ErrorResponse/Error` Type `Sender`, Code `ValidationError`; the recorded message "HealthCheck timeout must be less than interval." is built from the violated rule, so the requirement is a ValidationError naming that timeout-vs-interval constraint.
Emulator: HTTP 500, `application/x-amz-json-1.1`, no ErrorResponse, for all three refusal probes in the fixture.
Why: an invalid health-check configuration is reported as a server fault, so a client retries it rather than fixing the values, and a configuration AWS would reject could not be detected even if it were accepted.
Fix: `can_handle`, then ConfigureHealthCheck's validation (the cross-member timeout < interval check, in addition to the modeled @range refusals the framework already applies).

Cause 4 of 7: CreateLoadBalancerListeners re-declaring an existing port with different settings (fixture elbclassic-listener-lifecycle-duplicate-and-idempotent-delete, step task#7).
AWS: CreateLoadBalancerListeners models DuplicateListenerException, errorKind client, httpError 400, awsQueryError code `DuplicateListener`, httpResponseCode 400, doc "A listener already exists for the specified load balancer name and port, but with a different instance port, protocol, or SSL certificate." Wire shape: HTTP 400 text/xml, `ErrorResponse/Error` Type `Sender`, Code `DuplicateListener`. The recorded message is built from the request (it names the load balancer and the LoadBalancerPort, and lists InstancePort/Protocol/SSLCertificateId as the differing fields), so the requirement is a message identifying the conflicting load balancer and port.
Emulator: HTTP 500, `application/x-amz-json-1.1`, no ErrorResponse.
Why: the documented distinction between an idempotent re-create of an identical listener and a conflicting re-create is lost, so a client cannot tell which of its listener definitions conflicts.
Fix: `can_handle`, then CreateLoadBalancerListeners' conflict detection against the stored listener set.

Cause 5 of 7: ModifyLoadBalancerAttributes with an out-of-range connection-draining/idle timeout (fixture elbclassic-load-balancer-attributes-round-trip-and-range-validation, step task#8).
AWS: ModifyLoadBalancerAttributes models InvalidConfigurationRequestException (httpError 409, awsQueryError `InvalidConfigurationRequest`) and LoadBalancerAttributeNotFoundException, but the recorded refusal of an out-of-bounds timeout is HTTP 400 text/xml with `ErrorResponse/Error` Type `Sender`, Code `ValidationError` and a message built from the violated bound ("Time out must be at least 1 second and no more than 3600 seconds"). The requirement is a 400 ValidationError naming the timeout bound; the exact text is an example.
Emulator: HTTP 500, `application/x-amz-json-1.1`, no ErrorResponse.
Why: an out-of-range attribute value is indistinguishable from a server fault, so a client retries instead of clamping the value, and the attribute-range contract is unverifiable.
Fix: `can_handle`, then ModifyLoadBalancerAttributes' validation of the attribute value ranges.

Cause 6 of 7: AttachLoadBalancerToSubnets with an unknown subnet (fixture elbclassic-subnet-attach-detach-and-security-group-reassignment, step task#14).
AWS: AttachLoadBalancerToSubnets models SubnetNotFoundException, errorKind client, httpError 400, awsQueryError code `SubnetNotFound`, httpResponseCode 400, doc "One or more of the specified subnets do not exist." Wire shape: HTTP 400 text/xml, `ErrorResponse/Error` Type `Sender`, Code `SubnetNotFound` (the fixture does not assert the message text).
Emulator: HTTP 500, `application/x-amz-json-1.1`, no ErrorResponse.
Why: attaching a bad subnet is reported as a server fault, so a client cannot correct the subnet id and the attach/detach flow cannot validate its inputs.
Fix: `can_handle`, then AttachLoadBalancerToSubnets' subnet-existence validation.

Cause 7 of 7: ApplySecurityGroupsToLoadBalancer with an unknown security group (fixture elbclassic-subnet-attach-detach-and-security-group-reassignment, step task#15).
AWS: ApplySecurityGroupsToLoadBalancer models InvalidSecurityGroupException, errorKind client, httpError 400, awsQueryError code `InvalidSecurityGroup`, httpResponseCode 400, doc "One or more of the specified security groups do not exist." Wire shape: HTTP 400 text/xml, `ErrorResponse/Error` Type `Sender`, Code `InvalidSecurityGroup` (the fixture does not assert the message text).
Emulator: HTTP 500, `application/x-amz-json-1.1`, no ErrorResponse.
Why: security-group reassignment cannot be validated, so a client treats its own bad group id as a transient server failure.
Fix: `can_handle`, then ApplySecurityGroupsToLoadBalancer's security-group validation.

Traces: see Issue-Trace/Issue-Step for cause 1 and `trace_ids` for causes 2-7. Note: two further clusters on the same root cause arrive under the service token `elb` (DescribeLoadBalancerPolicies, DescribeLoadBalancerPolicyTypes) and are deferred by the per-service bootstrap gate, not filed here. Not covered by any existing open issue (no open issue matches 'elasticloadbalancing' or 'load balancer').

## Evidence
Replay summary: trace_set_id=simparity-wire-aws-v1, emulator_up=true, steps_total=95, steps_passed=7, steps_mismatched=61, state_reset_failures=0.

Cause 1 — CreateLoadBalancer / unknown subnet (elbclassic-create-load-balancer-validation-and-duplicate-name#task#9):
  expected status 400, got 500
  $.ErrorResponse: missing (expected {'Error': {'Type': 'Sender', 'Code': 'SubnetNotFound', 'Message': 'InvalidSubnetID.NotFound'}, 'RequestId': <shaped>}, got None)
  $headers.content-type: expected 'text/xml', got 'application/x-amz-json-1.1'

Cause 2 — DescribeLoadBalancers / unknown name (elbclassic-create-load-balancer-validation-and-duplicate-name#task#10, 2 affected steps):
  expected 400, got 500; $.ErrorResponse missing (expected Error{Type 'Sender', Code 'LoadBalancerNotFound', Message "There is no ACTIVE Load Balancer named '<name>'"}); content-type 'text/xml' -> 'application/x-amz-json-1.1'.

Cause 3 — ConfigureHealthCheck / invalid health check (elbclassic-health-check-configuration-round-trip-and-validation#task#7, 3 affected steps: task#7, task#8, task#9):
  expected 400, got 500; $.ErrorResponse missing (expected Error{Type 'Sender', Code 'ValidationError', Message 'HealthCheck timeout must be less than interval.'}); content-type 'text/xml' -> 'application/x-amz-json-1.1'.

Cause 4 — CreateLoadBalancerListeners / duplicate port (elbclassic-listener-lifecycle-duplicate-and-idempotent-delete#task#7):
  expected 400, got 500; $.ErrorResponse missing (expected Error{Type 'Sender', Code 'DuplicateListener', Message 'A listener already exists for <lb> with LoadBalancerPort 80, but with a different InstancePort, Protocol, or SSLCertificateId'}); content-type 'text/xml' -> 'application/x-amz-json-1.1'.

Cause 5 — ModifyLoadBalancerAttributes / out-of-range timeout (elbclassic-load-balancer-attributes-round-trip-and-range-validation#task#8):
  expected 400, got 500; $.ErrorResponse missing (expected Error{Type 'Sender', Code 'ValidationError', Message 'Time out must be at least 1 second and no more than 3600 seconds'}); content-type 'text/xml' -> 'application/x-amz-json-1.1'.

Cause 6 — AttachLoadBalancerToSubnets / unknown subnet (elbclassic-subnet-attach-detach-and-security-group-reassignment#task#14):
  expected 400, got 500; $.ErrorResponse missing (expected Error{Type 'Sender', Code 'SubnetNotFound', Message <shaped-not-asserted>}); content-type 'text/xml' -> 'application/x-amz-json-1.1'.

Cause 7 — ApplySecurityGroupsToLoadBalancer / unknown security group (elbclassic-subnet-attach-detach-and-security-group-reassignment#task#15):
  expected 400, got 500; $.ErrorResponse missing (expected Error{Type 'Sender', Code 'InvalidSecurityGroup', Message <shaped-not-asserted>}); content-type 'text/xml' -> 'application/x-amz-json-1.1'.

Model evidence (smithy_query shape, com-amazonaws-elasticloadbalancing-elasticloadbalancing-v7):
  SubnetNotFoundException: errorKind client, httpError 400, awsQueryError {code 'SubnetNotFound', httpResponseCode 400}
  AccessPointNotFoundException: errorKind client, httpError 400, awsQueryError {code 'LoadBalancerNotFound', httpResponseCode 400}
  DuplicateListenerException: errorKind client, httpError 400, awsQueryError {code 'DuplicateListener', httpResponseCode 400}
  InvalidSecurityGroupException: errorKind client, httpError 400, awsQueryError {code 'InvalidSecurityGroup', httpResponseCode 400}
  InvalidConfigurationRequestException: errorKind client, httpError 409, awsQueryError {code 'InvalidConfigurationRequest', httpResponseCode 409}
  HealthCheck: Interval @range 5..300, Timeout @range 2..60 ("This value must be less than the Interval value."), HealthyThreshold/UnhealthyThreshold @range 2..10, all @required
  service_info: protocols ['awsQuery'], sigv4Name 'elasticloadbalancing' — and the same sigv4Name on com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from the checkout.
2. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus).
3. Representative cause: fixture `elbclassic-create-load-balancer-validation-and-duplicate-name`, step `task#9` — a CreateLoadBalancer query POST naming a subnet id that does not exist. Observe HTTP 500 with Content-Type application/x-amz-json-1.1 instead of 400 text/xml with ErrorResponse/Error/Code = SubnetNotFound.
4. Repeat for each bundled cause using its fixture#step from `trace_ids`:
   - elbclassic-create-load-balancer-validation-and-duplicate-name#task#10 (DescribeLoadBalancers -> LoadBalancerNotFound)
   - elbclassic-health-check-configuration-round-trip-and-validation#task#7 (ConfigureHealthCheck -> ValidationError, timeout >= interval)
   - elbclassic-listener-lifecycle-duplicate-and-idempotent-delete#task#7 (CreateLoadBalancerListeners -> DuplicateListener)
   - elbclassic-load-balancer-attributes-round-trip-and-range-validation#task#8 (ModifyLoadBalancerAttributes -> ValidationError, timeout bound)
   - elbclassic-subnet-attach-detach-and-security-group-reassignment#task#14 (AttachLoadBalancerToSubnets -> SubnetNotFound)
   - elbclassic-subnet-attach-detach-and-security-group-reassignment#task#15 (ApplySecurityGroupsToLoadBalancer -> InvalidSecurityGroup)
5. Inspect services/service-com-amazonaws-elasticloadbalancing-elasticloadbalancing-v7/src/service.rs:76-84 and services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/src/service.rs:76-84 — both claim the same `elasticloadbalancing` route key.
6. All seven causes must answer their modeled 4xx text/xml awsQuery ErrorResponse (with the Code listed above) before this issue is closed.
