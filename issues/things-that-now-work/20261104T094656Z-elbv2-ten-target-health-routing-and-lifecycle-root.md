Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2.deregister-targets-draining-before-removal elbv2.target-draining-after-deregister elbv2.deregister-targets-transitions-state elbv2.cascade-delete-dependency
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elbv2.deregister-targets-draining-before-removal|run#15|AssertionError|assert state_immediately_after == "draining", (
Issue-Key: elbv2.target-draining-after-deregister|run#18|AssertionError|assert state == "draining", f"expected draining state right after deregister, got {state}"
Issue-Key: elbv2.deregister-targets-transitions-state|_poll_until#27|Timeout|time.sleep(interval_s)
Issue-Key: elbv2.health-check-transitions-to-unhealthy-with-reason|run#21|AssertionError|assert first.get("State") == "initial", (
Issue-Key: elbv2.cascade-delete-dependency|run#32|AssertionError|assert rule_delete_code is None, f"expected DeleteRule on the cascaded-away rule to succeed idempotently, got error {rule_delete_code}"
Issue-Commit: 7fd8acd3be042b6d00a1996e0a0d05df0b6f87ae

# elbv2: the 'draining' and 'initial' target-health states and the idempotent delete of a cascaded-away rule (3 of ten causes; the other seven are split out and still open)

## Resolution

Causes 1 and 5, the `initial` half of cause 2 and the message half of cause 6 are FIXED.
The other seven causes are NOT fixed, and they have been SPLIT OUT into their own open
report rather than counted as resolved here — every Issue-Key and trace id they own moved
with them, out of this record's header, into
`issues/things-that-do-not-work/20261104T094656Z-elbv2-seven-target-health-routing-and-data-plane-causes-still-open.md`.

* Cause 1 — FIXED. A target a load balancer is sending traffic to now enters `draining`
  (`Target.DeregistrationInProgress`) when `DeregisterTargets` names it, takes no new
  request while it is there, and leaves the target group when the group's
  `deregistration_delay.timeout_seconds` has elapsed; a registration nothing is sending
  traffic to has no connections to drain and leaves at once. The state machine is
  `services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/src/state/target_health.rs`,
  pinned by `tests/all/target_deregistration_and_health_spans.rs`.
* Cause 2 — the `initial` half is FIXED: health checking on a registration begins when it
  is registered or when the target group first becomes one a load balancer forwards to,
  whichever is later, so the first read after the attach reports
  `initial`/`Elb.RegistrationInProgress`. The PROBE half (a target whose health check
  cannot be answered must report `unhealthy`) is NOT fixed and is split out: deriving a
  health from a probe means connecting to the registered target, which this project's
  egress prohibition forbids, and reporting a constant `unhealthy` instead of a constant
  `healthy` would be the same defect with another literal.
* Cause 5 — FIXED. A rule released WITH the listener that held it (or with the load
  balancer that held the listener) is deleted idempotently: that deletion has been
  performed, so `DeleteRule` completes, while a rule the caller deleted itself and an
  identifier this store never held stay the modelled `RuleNotFound`
  (`src/state/cascaded_releases.rs`, pinned by `tests/all/cascaded_delete_idempotency.rs`).
  `DeleteListener` deliberately keeps `ListenerNotFound` for a listener a cascade removed,
  which is what the same recorded check asserts.
* Cause 6 — the MESSAGE half is FIXED: `SetRulePriorities` answers its `RuleNotFound` with
  `One or more priorities not found`, the collection-level wording the recorded refusal
  carries, rather than quoting one rule ARN. The RACE half is NOT fixed and is split out.
* Causes 3, 4, 7, 8, 9 and 10 — NOT FIXED, and split out so they stay filed. See the
  split-out report for each one's blocker.

> _Note: this issue was identified against the code base at commit `7fd8acd3be042b6d00a1996e0a0d05df0b6f87ae`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTargetHealth

## Description
Ten net-new root causes in ELBv2 target health, rule evaluation, lifecycle idempotency and the listener data plane. Each cause is independent and must be closed separately.

Cause 1 of 10: DeregisterTargets never reports the 'draining' state — the target jumps straight to 'unused'.
AWS: recorded behavior only for the timing; DescribeTargetHealth immediately after DeregisterTargets reports TargetHealth.State='draining' with Reason='Target.DeregistrationInProgress' for the duration of deregistration_delay.timeout_seconds, and only then the target leaves the group.
Emulator: the very next DescribeTargetHealth reports 'unused'; 'draining' is never observed.
Why: connection-draining is the contract clients wait on before terminating a target. A client that polls for 'draining' never sees it and cannot tell an in-flight drain from a target that was never registered, so it tears down capacity while requests are still in flight.
Fix: the target-group health state machine (DeregisterTargets handler + target-group state) must enter and hold 'draining' for the configured delay before removal, and DescribeTargetHealth must serialize it.
Traces: elbv2.deregister-targets-draining-before-removal, elbv2.target-draining-after-deregister, elbv2.deregister-targets-transitions-state.

Cause 2 of 10: a newly registered target's first health state is 'healthy', and health never moves with the probe result.
AWS: recorded behavior only: the first DescribeTargetHealth after RegisterTargets reports State='initial' with Reason='Elb.RegistrationInProgress'; the state then becomes 'healthy' or 'unhealthy' (Reason 'Target.FailedHealthChecks'/'Target.ResponseCodeMismatch') according to the configured health check.
Emulator: the first post-attach state is already 'healthy' with Reason None, and a target serving a failing health-check response stays 'healthy'.
Why: health is reported as a constant rather than derived from probes, so every negative health test passes falsely and a client cannot observe registration progress or a failing target.
Fix: the target-group health state machine and the health-check prober (target-group state + DescribeTargetHealth serializer).
Traces: elbv2.health-check-transitions-to-unhealthy-with-reason, elbv2.target-health-check-state-transition-unhealthy-to-healthy.

Cause 3 of 10: listener rule conditions are not evaluated — a host-header rule does not match and the request falls through to the default action.
AWS: recorded behavior only: an ALB evaluates rules in priority order; a request whose Host header matches a host-header condition is forwarded by that rule's action.
Emulator: a GET with `Host: <configured host>` is answered by the DEFAULT-RULE action instead of the BY-HOST-HEADER rule.
Why: path/host/header/query based routing is the core ALB function; clients get the default action for every request, so any multi-service ALB routes all traffic to one backend.
Fix: the ALB request router's rule-condition evaluation (listener rule matching in the data plane), not the CreateRule control-plane handler.
Traces: elbv2.listener-rule-condition-type-matrix-routing.

Cause 4 of 10: routing.http2.enabled is not honored in ALPN selection.
AWS: recorded behavior only: with the load-balancer attribute routing.http2.enabled=true an HTTPS client offering ALPN ['h2','http/1.1'] negotiates 'h2'.
Emulator: negotiates 'http/1.1'.
Why: a client that requires HTTP/2 (gRPC, multiplexed clients) silently gets HTTP/1.1, and the attribute it set reports as enabled while having no effect.
Fix: the HTTPS listener's TLS/ALPN negotiation in the data plane, reading the load-balancer attribute.
Traces: elbv2.routing-http2-attribute-decides-the-alpn-protocol-the-alb-selects.

Cause 5 of 10: DeleteRule on a rule already removed by a cascade answers RuleNotFound instead of succeeding.
AWS: recorded behavior only: after the owning listener/rule has been cascaded away, DeleteRule on that rule ARN completes idempotently (HTTP 200).
Emulator: answers RuleNotFound.
Why: cleanup and teardown paths (and CFN delete) that delete bottom-up fail on the second pass; a client must special-case an error AWS does not raise.
Fix: the DeleteRule handler's not-found branch (idempotent delete semantics).
Traces: elbv2.cascade-delete-dependency.

Cause 6 of 10: a concurrent rule-priority modification is refused with PriorityInUse where AWS completes it.
AWS: Smithy: PriorityInUseException (code `PriorityInUse`) applies when the requested priority is held by a rule outside the request. Under the optimistic-lock pattern in this check AWS lets exactly one reorder through and completes it.
Emulator: the call is refused with "Priority '1' is currently in use".
Why: a client reordering rules concurrently can never make progress; a legal reorder is reported as a permanent conflict.
Fix: the ModifyRule/SetRulePriorities priority bookkeeping — release the old priority within the same state transaction that claims the new one.
Traces: elbv2.concurrent-modify-rule-priority-optimistic-lock.

Cause 7 of 10: DeleteListener answers ListenerNotFound for a listener the same run created.
AWS: Smithy: DeleteListener errors are ListenerNotFoundException and ResourceInUseException; a listener that exists is deleted with HTTP 200 and an empty body.
Emulator: `ListenerNotFound` — "Listener '<listener-arn>' not found" — for a listener ARN returned by CreateListener in the same run.
Why: the listener ARN the control plane handed out does not resolve on delete, so no client can clean up a listener it created; this is a lookup-key mismatch, not a missing resource.
Fix: the DeleteListener handler's ARN resolution against listener state (the ARN form stored at create time vs. the one parsed at delete time).
Traces: elbv2.op-contract-delete-listener.

Cause 8 of 10: ELBv2 resources reported as created by a CloudFormation stack do not exist in ELBv2.
AWS: recorded behavior only: when a stack reaches CREATE_COMPLETE, every AWS::ElasticLoadBalancingV2::LoadBalancer / TargetGroup / Listener / ListenerRule in it is resolvable through the ELBv2 API, and a bad stack rolls back instead of reporting CREATE_COMPLETE.
Emulator: after CREATE_COMPLETE, DescribeLoadBalancers answers `LoadBalancerNotFound` ("One or more load balancers not found"), RegisterTargets answers `TargetGroupNotFound` ("Target groups '<arn>' not found") for the stack's target group, and DescribeRules/CreateRule answer `ListenerNotFound`; a stack with a nonexistent subnet reaches CREATE_COMPLETE instead of rolling back.
Why: an IaC client believes it has a load balancer, target group and listener; every subsequent ELBv2 call against the stack's outputs fails, and a template AWS rejects is reported as deployed.
Fix: the CloudFormation resource providers for the ELBv2 resource types — create the resource in ELBv2 state (and fail/roll back the stack when it cannot be created) rather than only recording stack status. Related open issues 20261023T205437Z-cloudformation-reports-create-complete-for-resources-it-never.md and 20261024T235905Z-elbv2-describerules-answers-listenernotfound-for-the-listener-arn.md describe adjacent symptoms but the suite did not mark these checks as owned by them.
Traces: elbv2.deploy-alb-cfn-then-target-health-behaves, elbv2.deploy-alb-listener-target-group-cfn-behaves, elbv2.deploy-alb-listener-target-group-cfn-routes-traffic, elbv2.deploy-nlb-target-registration-cfn-health-check, elbv2.deploy-duplicate-rule-priority-rejected.

Cause 9 of 10: the ALB/NLB listener data plane never produces an answer.
AWS: recorded behavior only: a listener on a load balancer whose target group has a registered, healthy target answers a client request (2xx/3xx, or the configured fixed-response/redirect) — over HTTP, over HTTPS with the listener's certificate and SNI selection, with mTLS passthrough headers, with X-Forwarded-For processing and with weighted/sticky forwarding.
Emulator: every one of these checks ends with the probe never answered — `TimeoutError: a 2xx answer from <listener endpoint>: the run budget ended first` / `TimeoutError: the passthrough HTTPS listener serving the multi-value lambda echo: the run budget ended first` — or the whole check exhausting its 60s budget without the listener ever responding.
Why: the data plane is the load balancer's entire purpose. A client that deploys a working configuration gets no response at all, so no end-to-end behavior (redirects, SNI, header rewriting, stickiness, weighting, source-IP conditions) can be exercised.
Fix: the ELBv2 data-plane listener/forwarder (listener socket + target forwarding), including TLS termination for HTTPS listeners.
Traces: elbv2.e2e-alb-target-routing-lifecycle, elbv2.e2e-blue-green-weighted-target-group-shift, elbv2.e2e-http-to-https-redirect-traffic-flow, elbv2.imported-certificate-is-presented-and-sni-selects-per-host, elbv2.listener-ssl-policy-bounds-the-negotiated-tls-version-and-cipher, elbv2.elbv2attr-deep-elbv2-1-mtls-passthrough-clientcert-chain, elbv2.elbv2attr-deep-elbv2-1-mtls-passthrough-forged-clientcert-header-stripped, elbv2.elbv2attr-deep-elbv2-1-server-header-removed-and-hsts-injected, elbv2.source-ip-condition-uses-the-layer3-address-not-x-forwarded-for, elbv2.xff-header-processing-mode-changes-what-the-target-receives.

Cause 10 of 10: capacity-reservation state never transitions.
AWS: recorded behavior only: after ModifyCapacityReservation, DescribeCapacityReservation reports the reservation moving through its provisioning states and reaching a terminal provisioned state server-side, without the client having to drive it.
Emulator: both checks exhaust the 60s budget without the reservation ever leaving its initial state.
Why: a client polling for provisioning completion never terminates, so capacity reservation is unusable and indistinguishable from a hung control plane.
Fix: the capacity-reservation state machine behind ModifyCapacityReservation/DescribeCapacityReservation (server-side state progression).
Traces: elbv2.capacity-reservation-modify-then-describe-transitions, elbv2.server-side-capacity-reservation-provisioning-completes-without-client-polling.

## Evidence
All quotes are from the behavioral replay output; ARNs, ids, hostnames, ports, request ids and dates are per-run values, not requirements.

Cause 1: `AssertionError: expected 'draining' immediately after DeregisterTargets, got 'unused'` and `AssertionError: expected draining state right after deregister, got unused`; elbv2.deregister-targets-transitions-state additionally exhausts its budget (`Error: Check timed out after 60.0s`) waiting for the transition.
Cause 2: `AssertionError: first post-attach health state was 'healthy' (reason None); real ELBv2 starts at 'initial'` and `AssertionError: expected unhealthy before the code fix, got healthy`.
Cause 3: `AssertionError: GET http://<alb-dns>/ headers={'Host': '<configured-host>'}: routed to 'DEFAULT-RULE', expected 'BY-HOST-HEADER'`.
Cause 4: `AssertionError: with routing.http2.enabled=true a client offering ['h2', 'http/1.1'] negotiated 'http/1.1', expected 'h2'`.
Cause 5: `AssertionError: expected DeleteRule on the cascaded-away rule to succeed idempotently, got error RuleNotFound`.
Cause 6: `AssertionError: Priority '1' is currently in use` (the emulator refused the reorder the check expected to complete).
Cause 7: `botocore.errorfactory.ListenerNotFoundException: An error occurred (ListenerNotFound) when calling the DeleteListener operation: Listener 'arn:...:listener/app/<lb>/<id>/<lid>' not found`.
Cause 8: `botocore.errorfactory.LoadBalancerNotFoundException: ... when calling the DescribeLoadBalancers operation: One or more load balancers not found`; `botocore.errorfactory.TargetGroupNotFoundException: ... when calling the RegisterTargets operation: Target groups '<stack tg arn>' not found` (three separate stacks: ALB listener+TG, ALB routes-traffic, NLB target registration); `AssertionError: expected PriorityInUse, got CODE='ListenerNotFound'`; and for the bad-subnet stack `AssertionError: expected the bad-subnet stack to roll back, but it reached CREATE_COMPLETE`.
Cause 9: `TimeoutError: a 2xx answer from https://<host>:<port>/probe/<id>: the run budget ended first; not graded`; `TimeoutError: the passthrough HTTPS listener serving the multi-value lambda echo: the run budget ended first; not graded`; and `Error: Check timed out after 60.0s` for the blue/green weighted shift, the HTTP→HTTPS redirect flow, the SNI/imported-certificate check, the SSL-policy negotiation check, the server-header/HSTS check, the source-IP condition check and the XFF processing-mode check — in every case no listener response was ever produced.
Cause 10: `Error: Check timed out after 60.0s` for both capacity-reservation checks.

## Steps to Reproduce
Replay the behavioral suite and re-run exactly the checks in trace_ids:
1. Cause 1 — create a target group, RegisterTargets, DeregisterTargets, then DescribeTargetHealth immediately; expect State='draining' (Reason Target.DeregistrationInProgress) for deregistration_delay.timeout_seconds.
2. Cause 2 — RegisterTargets and DescribeTargetHealth at once (expect 'initial'/Elb.RegistrationInProgress), then point the health check at a failing path and poll (expect 'unhealthy' with a Reason).
3. Cause 3 — create an ALB listener with a default fixed-response rule plus a higher-priority host-header rule, then GET with that Host header; expect the host-header rule's action.
4. Cause 4 — set routing.http2.enabled=true, connect to the HTTPS listener offering ALPN ['h2','http/1.1']; expect 'h2'.
5. Cause 5 — delete the owning listener so its rules cascade away, then DeleteRule on a cascaded rule ARN; expect HTTP 200.
6. Cause 6 — issue concurrent rule-priority modifications under the check's optimistic-lock pattern; expect one to complete.
7. Cause 7 — CreateListener then DeleteListener with the returned ARN; expect HTTP 200.
8. Cause 8 — deploy the four ELBv2 CloudFormation stacks and call DescribeLoadBalancers / RegisterTargets / DescribeRules against the stack outputs; expect them to resolve. Deploy the nonexistent-subnet stack; expect rollback, not CREATE_COMPLETE.
9. Cause 9 — for each listed check, bring up the listener and target as the check does and send the client request; expect a response within the check budget.
10. Cause 10 — ModifyCapacityReservation then poll DescribeCapacityReservation; expect the state to reach a terminal provisioned value server-side.
