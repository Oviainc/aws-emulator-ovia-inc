Issue-Source: trace-sweep-divergence
Issue-Trace: elbclassic-create-load-balancer-validation-and-duplicate-name
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elbclassic-create-load-balancer-validation-and-duplicate-name elbclassic-health-check-configuration-round-trip-and-validation elbclassic-listener-lifecycle-duplicate-and-idempotent-delete elbclassic-load-balancer-lifecycle-create-describe-delete elbclassic-subnet-attach-detach-and-security-group-reassignment elbclassic-load-balancer-attributes-round-trip-and-range-validation
Issue-Shape-Id: com.amazonaws.elasticloadbalancing#ElasticLoadBalancing_v7
Issue-Key: elasticloadbalancing|DescribeLoadBalancers|2xx->5xx|$.DescribeLoadBalancersResponse
Issue-Key: elasticloadbalancing|DescribeLoadBalancers|2xx->5xx|$headers.content-type
Issue-Key: elasticloadbalancing|DescribeLoadBalancers|2xx->5xx|status
Issue-Key: elasticloadbalancing|DeleteLoadBalancer|2xx->5xx|status
Issue-Key: elasticloadbalancing|ConfigureHealthCheck|2xx->5xx|$.ConfigureHealthCheckResponse
Issue-Key: elasticloadbalancing|ConfigureHealthCheck|2xx->5xx|$headers.content-type
Issue-Key: elasticloadbalancing|ConfigureHealthCheck|2xx->5xx|status
Issue-Key: elasticloadbalancing|CreateLoadBalancerListeners|2xx->5xx|$.CreateLoadBalancerListenersResponse
Issue-Key: elasticloadbalancing|CreateLoadBalancerListeners|2xx->5xx|$headers.content-type
Issue-Key: elasticloadbalancing|CreateLoadBalancerListeners|2xx->5xx|status
Issue-Key: elasticloadbalancing|DeleteLoadBalancerListeners|2xx->5xx|$.DeleteLoadBalancerListenersResponse
Issue-Key: elasticloadbalancing|DeleteLoadBalancerListeners|2xx->5xx|$headers.content-type
Issue-Key: elasticloadbalancing|DeleteLoadBalancerListeners|2xx->5xx|status
Issue-Key: elasticloadbalancing|DescribeLoadBalancerAttributes|2xx->5xx|$.DescribeLoadBalancerAttributesResponse
Issue-Key: elasticloadbalancing|DescribeLoadBalancerAttributes|2xx->5xx|$headers.content-type
Issue-Key: elasticloadbalancing|DescribeLoadBalancerAttributes|2xx->5xx|status
Issue-Key: elasticloadbalancing|ModifyLoadBalancerAttributes|2xx->5xx|$.ModifyLoadBalancerAttributesResponse
Issue-Key: elasticloadbalancing|ModifyLoadBalancerAttributes|2xx->5xx|$headers.content-type
Issue-Key: elasticloadbalancing|ModifyLoadBalancerAttributes|2xx->5xx|status
Issue-Key: elasticloadbalancing|DeleteLoadBalancer|2xx->5xx|$.DeleteLoadBalancerResponse
Issue-Key: elasticloadbalancing|DeleteLoadBalancer|2xx->5xx|$headers.content-type
Issue-Commit: 17f16131651e3a76635c083c093a8581359ec045

# Classic ELB success responses answer 500 JSON instead of awsQuery XML (8 causes)

> _Note: this issue was identified against the code base at commit `17f16131651e3a76635c083c093a8581359ec045`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeLoadBalancers

## Description
EIGHT causes, all of them success-path Classic Load Balancer operations that answer HTTP 500 with `Content-Type: application/x-amz-json-1.1` and NO response body of the modeled shape. Each cause below must be closed before this issue is submitted.

SHARED ROOT CAUSE (applies to all eight, and to the sibling issue covering the error paths): `Elastic Load Balancing` (Classic, model version 2012-06-01) and `Elastic Load Balancing v2` (2015-12-01) both declare `sigv4Name: elasticloadbalancing` / `endpointPrefix: elasticloadbalancing` (smithy_query service_info on both models), and BOTH crates still carry the generated default `can_handle`, which claims a request whose resolved route key equals the service's own signing name:
  services/service-com-amazonaws-elasticloadbalancing-elasticloadbalancing-v7/src/service.rs:76-84
  services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/src/service.rs:76-84
So every request to the shared `elasticloadbalancing` signing name is claimed by two services at once. Hypothesis: that ambiguous claim is what the router turns into a generic 500 rendered in the framework's default awsJson envelope — which is exactly what the replay observes, since an awsQuery service can never legitimately answer `application/x-amz-json-1.1`, and the Classic crate's own scaffold would have answered 501 text/xml had any handler been reached. The service's state layer is fully written (services/service-com-amazonaws-elasticloadbalancing-elasticloadbalancing-v7/src/state.rs), so the handlers are unreachable rather than absent.

Cause 1 of 8: DescribeLoadBalancers (fixture elbclassic-create-load-balancer-validation-and-duplicate-name, step task#5).
AWS: Smithy awsQuery, ElasticLoadBalancing 2012-06-01. DescribeLoadBalancers -> DescribeAccessPointsOutput, HTTP 200, `Content-Type: text/xml`, body `DescribeLoadBalancersResponse/DescribeLoadBalancersResult/LoadBalancerDescriptions` with one `member` per load balancer (LoadBalancerName, DNSName, CanonicalHostedZoneNameID, Scheme, VPCId, Subnets, SecurityGroups, SourceSecurityGroup, ListenerDescriptions, HealthCheck, Policies, CreatedTime) plus `ResponseMetadata/RequestId`.
Emulator: HTTP 500, `Content-Type: application/x-amz-json-1.1`, no `DescribeLoadBalancersResponse` element at all.
Why: a client can never read back a load balancer it just created; the 500 is retried as a server fault rather than surfaced, and every describe-then-assert flow (and every CloudFormation/ASG consumer reading ELB state) stalls.
Fix: the service's request-claiming seam (`can_handle` in the Classic crate's service.rs, so only Classic actions are claimed there) and then the DescribeLoadBalancers handler/serializer.

Cause 2 of 8: DeleteLoadBalancer, teardown path (fixture elbclassic-create-load-balancer-validation-and-duplicate-name, step teardown#11).
AWS: DeleteLoadBalancer -> DeleteAccessPointOutput, HTTP 200. The model declares NO errors for it, and its documentation makes it idempotent: deleting an already-deleted or never-created load balancer still succeeds.
Emulator: HTTP 500.
Why: teardown cannot complete, so resources leak and a client that treats delete as idempotent sees a hard server failure instead.
Fix: `can_handle`, then the DeleteLoadBalancer handler.

Cause 3 of 8: ConfigureHealthCheck (fixture elbclassic-health-check-configuration-round-trip-and-validation, step task#5).
AWS: ConfigureHealthCheck -> ConfigureHealthCheckOutput, HTTP 200 text/xml, body `ConfigureHealthCheckResponse/ConfigureHealthCheckResult/HealthCheck` echoing the accepted configuration (Target, Interval, Timeout, UnhealthyThreshold, HealthyThreshold). Smithy: HealthCheck members all @required, Interval @range 5..300, Timeout @range 2..60, UnhealthyThreshold/HealthyThreshold @range 2..10.
Emulator: HTTP 500, `application/x-amz-json-1.1`, no ConfigureHealthCheckResponse.
Why: health-check configuration can never be applied or read back, so no client can drive the documented round trip.
Fix: `can_handle`, then the ConfigureHealthCheck handler (services/service-com-amazonaws-elasticloadbalancing-elasticloadbalancing-v7/src/ops/configure_health_check.rs is still the SCAFFOLDED stub) and the HealthCheckConfig state it writes.

Cause 4 of 8: CreateLoadBalancerListeners (fixture elbclassic-listener-lifecycle-duplicate-and-idempotent-delete, step task#5).
AWS: CreateLoadBalancerListeners -> CreateLoadBalancerListenerOutput, HTTP 200 text/xml, an empty `CreateLoadBalancerListenersResult` plus `ResponseMetadata/RequestId`.
Emulator: HTTP 500, `application/x-amz-json-1.1`.
Why: listeners can never be added, so nothing downstream of listener creation (duplicate detection, SSL certificate assignment, policy binding) is reachable.
Fix: `can_handle`, then the CreateLoadBalancerListeners handler and the listener set in state.

Cause 5 of 8: DeleteLoadBalancerListeners (fixture elbclassic-listener-lifecycle-duplicate-and-idempotent-delete, step task#8).
AWS: DeleteLoadBalancerListeners -> DeleteLoadBalancerListenerOutput, HTTP 200 text/xml, an empty `DeleteLoadBalancerListenersResult` plus ResponseMetadata. Deleting a listener port that is not configured still returns 200 (the recorded fixture asserts the same 200 twice in a row).
Emulator: HTTP 500 on both the first and the repeated delete.
Why: listener removal is impossible and the documented idempotency of a repeat delete cannot be relied on.
Fix: `can_handle`, then the DeleteLoadBalancerListeners handler.

Cause 6 of 8: DescribeLoadBalancerAttributes (fixture elbclassic-load-balancer-attributes-round-trip-and-range-validation, step task#5).
AWS: DescribeLoadBalancerAttributes -> DescribeLoadBalancerAttributesOutput, HTTP 200 text/xml, body `DescribeLoadBalancerAttributesResult/LoadBalancerAttributes` carrying CrossZoneLoadBalancing, ConnectionSettings.IdleTimeout, AccessLog, ConnectionDraining and AdditionalAttributes. Recorded behavior shows a freshly created load balancer already reports the service defaults (cross-zone disabled, idle timeout 60, access log disabled, connection draining disabled with timeout 300, and the `elb.http.desyncmitigationmode` additional attribute).
Emulator: HTTP 500, `application/x-amz-json-1.1`.
Why: attribute state is unreadable, so neither the defaults nor any modification can be verified.
Fix: `can_handle`, then the DescribeLoadBalancerAttributes handler plus an attributes record in state (ComAmazonawsElasticloadbalancingElasticloadbalancingV7State currently models no load-balancer attributes at all).

Cause 7 of 8: ModifyLoadBalancerAttributes (fixture elbclassic-load-balancer-attributes-round-trip-and-range-validation, step task#6).
AWS: ModifyLoadBalancerAttributes -> ModifyLoadBalancerAttributesOutput, HTTP 200 text/xml, body `ModifyLoadBalancerAttributesResult` echoing LoadBalancerName and only the attributes the request supplied.
Emulator: HTTP 500, `application/x-amz-json-1.1`.
Why: attributes can never be changed, so the whole attribute round trip is unusable.
Fix: `can_handle`, then the ModifyLoadBalancerAttributes handler and the attributes state it persists.

Cause 8 of 8: DeleteLoadBalancer, lifecycle path (fixture elbclassic-load-balancer-lifecycle-create-describe-delete, step task#6).
AWS: DeleteLoadBalancer -> HTTP 200 with an empty `DeleteLoadBalancerResult` and ResponseMetadata, both for the first delete and for a repeat delete of the same name (the model declares no errors for this operation).
Emulator: HTTP 500, `application/x-amz-json-1.1`, no DeleteLoadBalancerResponse, on both the first and the repeat delete.
Why: the create/describe/delete lifecycle never terminates and the documented idempotent repeat delete is reported as a server fault.
Fix: `can_handle`, then the DeleteLoadBalancer handler and removal from the per-region store.

Traces: see Issue-Trace/Issue-Step for cause 1 and `trace_ids` for causes 2-8. Note: two further clusters on the same root cause arrive under the service token `elb` (DescribeLoadBalancerPolicies, DescribeLoadBalancerPolicyTypes) and are deferred by the per-service bootstrap gate, not filed here. Not covered by any existing open issue (no open issue matches 'elasticloadbalancing' or 'load balancer').

## Evidence
Replay summary: trace_set_id=simparity-wire-aws-v1, emulator_up=true, steps_total=95, steps_passed=7, steps_mismatched=61, state_reset_failures=0.

Cause 1 — DescribeLoadBalancers (elbclassic-create-load-balancer-validation-and-duplicate-name#task#5, 8 affected steps):
  expected status 200, got 500
  $.DescribeLoadBalancersResponse: missing (expected DescribeLoadBalancersResult/LoadBalancerDescriptions/member{...}, got None)
  $headers.content-type: expected 'text/xml', got 'application/x-amz-json-1.1'

Cause 2 — DeleteLoadBalancer teardown (elbclassic-create-load-balancer-validation-and-duplicate-name#teardown#11, 5 affected steps): expected status 200, got 500.

Cause 3 — ConfigureHealthCheck (elbclassic-health-check-configuration-round-trip-and-validation#task#5):
  expected 200, got 500; $.ConfigureHealthCheckResponse missing (expected ConfigureHealthCheckResult/HealthCheck); content-type expected 'text/xml', got 'application/x-amz-json-1.1'.

Cause 4 — CreateLoadBalancerListeners (elbclassic-listener-lifecycle-duplicate-and-idempotent-delete#task#5):
  expected 200, got 500; $.CreateLoadBalancerListenersResponse missing; content-type 'text/xml' -> 'application/x-amz-json-1.1'.

Cause 5 — DeleteLoadBalancerListeners (elbclassic-listener-lifecycle-duplicate-and-idempotent-delete#task#8, 2 affected steps):
  expected 200, got 500; $.DeleteLoadBalancerListenersResponse missing; content-type 'text/xml' -> 'application/x-amz-json-1.1'.

Cause 6 — DescribeLoadBalancerAttributes (elbclassic-load-balancer-attributes-round-trip-and-range-validation#task#5, 3 affected steps):
  expected 200, got 500; $.DescribeLoadBalancerAttributesResponse missing (expected LoadBalancerAttributes with CrossZoneLoadBalancing/ConnectionSettings/AccessLog/ConnectionDraining/AdditionalAttributes); content-type 'text/xml' -> 'application/x-amz-json-1.1'.

Cause 7 — ModifyLoadBalancerAttributes (elbclassic-load-balancer-attributes-round-trip-and-range-validation#task#6):
  expected 200, got 500; $.ModifyLoadBalancerAttributesResponse missing; content-type 'text/xml' -> 'application/x-amz-json-1.1'.

Cause 8 — DeleteLoadBalancer lifecycle (elbclassic-load-balancer-lifecycle-create-describe-delete#task#6, 2 affected steps):
  expected 200, got 500; $.DeleteLoadBalancerResponse missing; content-type 'text/xml' -> 'application/x-amz-json-1.1'.

Model evidence: smithy_query service_info on com-amazonaws-elasticloadbalancing-elasticloadbalancing-v7 -> protocols ['awsQuery'], sigv4Name 'elasticloadbalancing', xmlNamespace http://elasticloadbalancing.amazonaws.com/doc/2012-06-01/. Same query on com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10 -> protocols ['awsQuery'], sigv4Name 'elasticloadbalancing'. An awsQuery service must never answer application/x-amz-json-1.1.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from the checkout.
2. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus).
3. Representative cause: fixture `elbclassic-create-load-balancer-validation-and-duplicate-name`, step `task#5` — a DescribeLoadBalancers query-protocol POST signed for the `elasticloadbalancing` signing name after the load balancer was created. Observe HTTP 500 with Content-Type application/x-amz-json-1.1 instead of 200 text/xml with a DescribeLoadBalancersResponse body.
4. Repeat for each bundled cause using its fixture#step from `trace_ids`:
   - elbclassic-create-load-balancer-validation-and-duplicate-name#teardown#11 (DeleteLoadBalancer)
   - elbclassic-health-check-configuration-round-trip-and-validation#task#5 (ConfigureHealthCheck)
   - elbclassic-listener-lifecycle-duplicate-and-idempotent-delete#task#5 (CreateLoadBalancerListeners)
   - elbclassic-listener-lifecycle-duplicate-and-idempotent-delete#task#8 (DeleteLoadBalancerListeners)
   - elbclassic-load-balancer-attributes-round-trip-and-range-validation#task#5 (DescribeLoadBalancerAttributes)
   - elbclassic-load-balancer-attributes-round-trip-and-range-validation#task#6 (ModifyLoadBalancerAttributes)
   - elbclassic-load-balancer-lifecycle-create-describe-delete#task#6 (DeleteLoadBalancer)
5. Inspect services/service-com-amazonaws-elasticloadbalancing-elasticloadbalancing-v7/src/service.rs:76-84 and services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/src/service.rs:76-84 — both claim the same `elasticloadbalancing` route key.
6. All eight causes must answer the modeled 200 text/xml response before this issue is closed.
