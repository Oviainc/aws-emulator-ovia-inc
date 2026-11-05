Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2.health-check-transitions-to-unhealthy-with-reason elbv2.target-health-check-state-transition-unhealthy-to-healthy elbv2.listener-rule-condition-type-matrix-routing elbv2.routing-http2-attribute-decides-the-alpn-protocol-the-alb-selects elbv2.concurrent-modify-rule-priority-optimistic-lock elbv2.op-contract-delete-listener elbv2.deploy-alb-cfn-then-target-health-behaves elbv2.deploy-alb-listener-target-group-cfn-behaves elbv2.deploy-alb-listener-target-group-cfn-routes-traffic elbv2.deploy-nlb-target-registration-cfn-health-check elbv2.deploy-duplicate-rule-priority-rejected elbv2.e2e-alb-target-routing-lifecycle elbv2.e2e-blue-green-weighted-target-group-shift elbv2.e2e-http-to-https-redirect-traffic-flow elbv2.imported-certificate-is-presented-and-sni-selects-per-host elbv2.listener-ssl-policy-bounds-the-negotiated-tls-version-and-cipher elbv2.elbv2attr-deep-elbv2-1-mtls-passthrough-clientcert-chain elbv2.elbv2attr-deep-elbv2-1-mtls-passthrough-forged-clientcert-header-stripped elbv2.elbv2attr-deep-elbv2-1-server-header-removed-and-hsts-injected elbv2.source-ip-condition-uses-the-layer3-address-not-x-forwarded-for elbv2.xff-header-processing-mode-changes-what-the-target-receives elbv2.capacity-reservation-modify-then-describe-transitions elbv2.server-side-capacity-reservation-provisioning-completes-without-client-polling
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elbv2.target-health-check-state-transition-unhealthy-to-healthy|run#14|AssertionError|assert state1 == "unhealthy", f"expected unhealthy before the code fix, got {state1}"
Issue-Key: elbv2.listener-rule-condition-type-matrix-routing|_assert_body#23|AssertionError|assert body.strip() == expected, (
Issue-Key: elbv2.routing-http2-attribute-decides-the-alpn-protocol-the-alb-selects|run#14|AssertionError|assert selected == "h2", (
Issue-Key: elbv2.concurrent-modify-rule-priority-optimistic-lock|run#16|AssertionError|assert loser_message == "One or more priorities not found", loser_message
Issue-Key: elbv2|DeleteListener|ListenerNotFound
Issue-Key: elbv2|DescribeLoadBalancers|LoadBalancerNotFound
Issue-Key: elbv2|RegisterTargets|TargetGroupNotFound
Issue-Key: elbv2.deploy-duplicate-rule-priority-rejected|run#7|AssertionError|assert code == "PriorityInUse", f"expected PriorityInUse, got CODE={code!r}"
Issue-Key: elbv2.e2e-alb-target-routing-lifecycle|_fetch_http#24|TimeoutError|raise TimeoutError(f"a 2xx answer from {url}: the run budget ended first; not "
Issue-Key: elbv2.e2e-blue-green-weighted-target-group-shift|_poll_http_get#23|Timeout|time.sleep(interval_s)
Issue-Key: elbv2.e2e-http-to-https-redirect-traffic-flow|_poll_http_get#21|Timeout|time.sleep(interval_s)
Issue-Key: elbv2.imported-certificate-is-presented-and-sni-selects-per-host|_poll_until#18|Timeout|time.sleep(interval_s)
Issue-Key: elbv2.listener-ssl-policy-bounds-the-negotiated-tls-version-and-cipher|_poll_until#24|Timeout|time.sleep(interval_s)
Issue-Key: elbv2.elbv2attr-deep-elbv2-1-mtls-passthrough-clientcert-chain|_poll_until#29|Timeout|time.sleep(interval_s)
Issue-Key: elbv2.elbv2attr-deep-elbv2-1-mtls-passthrough-forged-clientcert-header-stripped|_poll_until#28|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Key: elbv2.elbv2attr-deep-elbv2-1-server-header-removed-and-hsts-injected|_poll_until#27|Timeout|time.sleep(interval_s)
Issue-Key: elbv2.source-ip-condition-uses-the-layer3-address-not-x-forwarded-for|_poll_until#20|Timeout|time.sleep(interval_s)
Issue-Key: elbv2.xff-header-processing-mode-changes-what-the-target-receives|_poll_until#18|Timeout|time.sleep(interval_s)
Issue-Key: elbv2.capacity-reservation-modify-then-describe-transitions|_assert_reservation_withheld#8|Timeout|time.sleep(2.0)
Issue-Key: elbv2.server-side-capacity-reservation-provisioning-completes-without-client-polling|run#4|Timeout|time.sleep(2.0)
Issue-Commit: 7fd8acd3be042b6d00a1996e0a0d05df0b6f87ae

# elbv2: seven target-health, routing, lifecycle and data-plane causes still open (split out of the ten-cause root-cause report)

> _Split out of `issues/things-that-now-work/20261104T094656Z-elbv2-ten-target-health-routing-and-lifecycle-root.md`, whose causes 1 and 5, the `initial` half of cause 2 and the message half of cause 6 are fixed. The causes below are NOT fixed; they kept the original report's numbering, trace ids and Issue-Keys so they stay filed and replayable. They were identified against the code base at commit `7fd8acd3be042b6d00a1996e0a0d05df0b6f87ae`, so trace each one back to the current code base._

## Operation
DescribeTargetHealth

## Description
Seven of the ten net-new ELBv2 root causes remain open. Each is independent and must be closed separately; each entry states what was learned while the other three were fixed, so the next fixer does not start from zero.

Cause 2 of 10 (the PROBE half; its `initial` half is fixed): a target whose health check cannot be answered still reports 'healthy'.
AWS: recorded behavior only: after the `initial` state the target becomes 'healthy' or 'unhealthy' (Reason 'Target.FailedHealthChecks'/'Target.ResponseCodeMismatch') according to the configured health check; the recorded divergence is `an in-VPC IP with nothing listening reported {'State': 'healthy'}; expected 'unhealthy'`.
Emulator: every registration that is past its settling span reports 'healthy', whatever is or is not behind the address.
Why: health is reported as a constant rather than derived from a check, so every negative health test passes falsely.
What is known: the `initial` span and the `draining` state are now a real state machine (`src/state/target_health.rs`), so only the CHECK ITSELF is missing. A faithful check has to be answered by something in this process: a connection to an arbitrary registered address is egress, which this project forbids and the merge gate's pattern scan refuses, and a constant 'unhealthy' is the same defect as the constant 'healthy' with another literal. The one target type this emulator can really answer for is `lambda`, whose function the Lambda service holds and can be invoked in-process.
Traces: elbv2.health-check-transitions-to-unhealthy-with-reason, elbv2.target-health-check-state-transition-unhealthy-to-healthy.

Cause 3 of 10: a host-header rule does not match and the request falls through to the default action.
AWS: recorded behavior only: an ALB evaluates rules in priority order; a request whose Host header matches a host-header condition is forwarded by that rule's action.
Emulator: a GET with `Host: <configured host>` is answered by the DEFAULT-RULE action instead of the BY-HOST-HEADER rule.
What is known: the condition half is already implemented — `src/service/data_plane.rs`'s `condition_met` matches `host-header` against the request's host case-insensitively, over both `Values` and `HostHeaderConfig.Values` — and the plane DID answer the probe (with the default rule), so the request was claimed. The defect is in what the serving seam reads as the request's HOST (the ADDRESSING half of that module: `Addressed::of_request` / `DataPlaneRequest::of`, and which of the four addressings claimed the probe), not in the matcher.
Traces: elbv2.listener-rule-condition-type-matrix-routing.

Cause 4 of 10: routing.http2.enabled is not honored in ALPN selection.
AWS: recorded behavior only: with the load-balancer attribute routing.http2.enabled=true an HTTPS client offering ALPN ['h2','http/1.1'] negotiates 'h2'.
Emulator: negotiates 'http/1.1'.
What is known: the attribute is stored and reported by the control plane; the negotiation happens in the core listener's TLS handshake, which this service does not own (`vendor/`), so closing this needs a core seam that lets a service decide the ALPN protocol for the name a load balancer publishes.
Traces: elbv2.routing-http2-attribute-decides-the-alpn-protocol-the-alb-selects.

Cause 6 of 10 (the RACE half; its message half is fixed): a concurrent rule-priority modification is refused PriorityInUse where AWS lets one through.
AWS: under the optimistic-lock pattern in this check AWS lets exactly one reorder through and refuses the loser with `One or more priorities not found` — the rule its pairs named was gone by then.
Emulator: the loser is refused `Priority '1' is currently in use`.
What is known: `SetRulePriorities` already applies the whole request as ONE transaction (the conflict is decided over the order the request LEAVES BEHIND, so a swap or a cycle in one call is accepted — `tests/all/rule_priorities.rs`), and its `RuleNotFound` now carries AWS's list wording. What is missing is which call of the recorded race fails: the emulator still holds a rule the recorded run had already removed at that point, which the divergence does not show. The check's step sequence is needed to close it.
Traces: elbv2.concurrent-modify-rule-priority-optimistic-lock.

Cause 7 of 10: DeleteListener answers ListenerNotFound for a listener the same run created.
AWS: Smithy: DeleteListener errors are ListenerNotFoundException and ResourceInUseException; a listener that exists is deleted with HTTP 200 and an empty body.
Emulator: `ListenerNotFound` — "Listener '<listener-arn>' not found" — for a listener ARN returned by CreateListener in the same run.
What is known: it is NOT the cascade (the recorded `cascade-delete-dependency` check asserts that a listener removed with its load balancer IS refused `ListenerNotFound`, and that is what the emulator answers), and it is NOT an ARN-form mismatch: a CreateListener/DeleteListener round trip resolves, pinned by `tests/all/listener_lifecycle_certificates_and_rules.rs` and `tests/all/cascaded_delete_idempotency.rs`. Something else in that run removed the listener first; the check's own call sequence is needed.
Traces: elbv2.op-contract-delete-listener.

Cause 8 of 10: ELBv2 resources reported as created by a CloudFormation stack do not exist in ELBv2.
AWS: recorded behavior only: when a stack reaches CREATE_COMPLETE, every AWS::ElasticLoadBalancingV2::LoadBalancer / TargetGroup / Listener / ListenerRule in it is resolvable through the ELBv2 API, and a bad stack rolls back instead of reporting CREATE_COMPLETE.
Emulator: after CREATE_COMPLETE, DescribeLoadBalancers answers `LoadBalancerNotFound`, RegisterTargets answers `TargetGroupNotFound` for the stack's target group, and DescribeRules/CreateRule answer `ListenerNotFound`; a stack with a nonexistent subnet reaches CREATE_COMPLETE instead of rolling back.
Fix: the CloudFormation resource providers for the ELBv2 resource types — create the resource in ELBv2 state (and fail/roll back the stack when it cannot be created) rather than only recording stack status. That code is in the CloudFormation service crate, not this one. Related open issues 20261023T205437Z-cloudformation-reports-create-complete-for-resources-it-never.md and 20261024T235905Z-elbv2-describerules-answers-listenernotfound-for-the-listener-arn.md describe adjacent symptoms.
Traces: elbv2.deploy-alb-cfn-then-target-health-behaves, elbv2.deploy-alb-listener-target-group-cfn-behaves, elbv2.deploy-alb-listener-target-group-cfn-routes-traffic, elbv2.deploy-nlb-target-registration-cfn-health-check, elbv2.deploy-duplicate-rule-priority-rejected.

Cause 9 of 10: the ALB/NLB listener data plane never produces an answer.
AWS: recorded behavior only: a listener on a load balancer whose target group has a registered, healthy target answers a client request (2xx/3xx, or the configured fixed-response/redirect) — over HTTP, over HTTPS with the listener's certificate and SNI selection, with mTLS passthrough headers, with X-Forwarded-For processing and with weighted/sticky forwarding.
Emulator: every one of these checks ends with the probe never answered.
What is known: the HTTP half of the plane exists and answers a `fixed-response` and a `redirect` out of stored listener records (`src/service/data_plane.rs`); what is missing is (a) TLS termination, the certificate and SNI selection, and the mTLS handshake, which the core listener in read-only `vendor/` owns, and (b) a `forward` that reaches something: every registered `instance`/`ip` target is an address with no server of this emulator behind it, so a forward can only be answered by a target type this process can serve (a `lambda` target, invoked in-process). Until one of those exists the plane answers 503/502 rather than inventing a body for a backend that never ran.
Traces: elbv2.e2e-alb-target-routing-lifecycle, elbv2.e2e-blue-green-weighted-target-group-shift, elbv2.e2e-http-to-https-redirect-traffic-flow, elbv2.imported-certificate-is-presented-and-sni-selects-per-host, elbv2.listener-ssl-policy-bounds-the-negotiated-tls-version-and-cipher, elbv2.elbv2attr-deep-elbv2-1-mtls-passthrough-clientcert-chain, elbv2.elbv2attr-deep-elbv2-1-mtls-passthrough-forged-clientcert-header-stripped, elbv2.elbv2attr-deep-elbv2-1-server-header-removed-and-hsts-injected, elbv2.source-ip-condition-uses-the-layer3-address-not-x-forwarded-for, elbv2.xff-header-processing-mode-changes-what-the-target-receives.

Cause 10 of 10: capacity-reservation state never transitions.
AWS: recorded behavior only: after ModifyCapacityReservation, DescribeCapacityReservation reports the reservation moving through its provisioning states and reaching a terminal provisioned state server-side, without the client having to drive it.
Emulator: both checks exhaust the 60s budget without the reservation ever leaving its initial state.
What is known: this account is NOT entitled to configure minimum load balancer capacity — the recorded `ModifyCapacityReservation` naming `MinimumLoadBalancerCapacity` answers the modelled `OperationNotPermitted` with the account-level sentence, which `tests/all/capacity_reservation.rs` pins over every figure — so there is no reservation to move through provisioning states, and the first check's own `_assert_reservation_withheld` polls for something that refusal does not produce. Closing this needs the two checks' step sequences: what they poll for after a withheld reservation, and which of `LastModifiedTime`, `DecreaseRequestsRemaining` or `CapacityReservationState` the recorded read answers with.
Traces: elbv2.capacity-reservation-modify-then-describe-transitions, elbv2.server-side-capacity-reservation-provisioning-completes-without-client-polling.

## Steps to Reproduce
Replay the behavioral suite and re-run exactly the checks in trace_ids.
1. Cause 2 (probe half) — RegisterTargets with an in-VPC address nothing listens on, wait past the settling span and poll DescribeTargetHealth; expect 'unhealthy' with a Reason.
2. Cause 3 — create an ALB listener with a default fixed-response rule plus a higher-priority host-header rule, then GET with that Host header; expect the host-header rule's action.
3. Cause 4 — set routing.http2.enabled=true, connect to the HTTPS listener offering ALPN ['h2','http/1.1']; expect 'h2'.
4. Cause 6 — issue concurrent rule-priority modifications under the check's optimistic-lock pattern; expect one to complete and the loser to be refused `One or more priorities not found`.
5. Cause 7 — CreateListener then DeleteListener with the returned ARN, in the op-contract check's own call sequence; expect HTTP 200.
6. Cause 8 — deploy the four ELBv2 CloudFormation stacks and call DescribeLoadBalancers / RegisterTargets / DescribeRules against the stack outputs; expect them to resolve. Deploy the nonexistent-subnet stack; expect rollback, not CREATE_COMPLETE.
7. Cause 9 — for each listed check, bring up the listener and target as the check does and send the client request; expect a response within the check budget.
8. Cause 10 — ModifyCapacityReservation then poll DescribeCapacityReservation; expect what the recording answers.
