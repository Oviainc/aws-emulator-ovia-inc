Issue-Source: trace-sweep-divergence
Issue-Trace: arc-region-switch-get-plan-in-region-not-found
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: arc-region-switch-get-plan-in-region-not-found arc-region-switch-list-route53-health-checks-in-region-not-found
Issue-Shape-Id: com.amazonaws.arcregionswitch#ArcRegionSwitch
Issue-Key: arc-region-switch|GetPlanInRegion|4xx->4xx|status
Issue-Key: arc-region-switch|ListRoute53HealthChecksInRegion|4xx->4xx|status
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# arc-region-switch InRegion reads answer 404 instead of 400 for an unresolvable plan ARN

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPlanInRegion

## Description
The Region-scoped (`*InRegion`) read operations of ARC Region switch share one defect: when the request's `arn` names a plan the service cannot resolve from the Region the call reached, the emulator maps that to the modeled `ResourceNotFoundException` (HTTP 404), while the recorded AWS responses for that same condition are HTTP 400 — the client-error class the service uses for an argument it cannot act on (`IllegalArgumentException`, "The request processing has an invalid argument", is the 400 shape this service models). The whole-corpus score is 0% because of these two clusters, and both are the same mapping decision made in two handlers. 2 causes.

Cause 1 of 2: GetPlanInRegion (fixture arc-region-switch-get-plan-in-region-not-found, step task#1).
AWS: recorded behavior only for the status class — Smithy (com.amazonaws.arcregionswitch#GetPlanInRegion) lists ONLY `AccessDeniedException` (@httpError 403) and `ResourceNotFoundException` (@httpError 404) for this operation and documents no 400, so the model does not account for the recording; the recorded response for a pattern-valid `arn` (PlanArn, pattern ^arn:aws[a-zA-Z-]*:arc-region-switch::[0-9]{12}:plan/([a-zA-Z0-9](?:[a-zA-Z0-9-]{0,30}[a-zA-Z0-9])?):([a-z0-9]{6})$) that resolves to no plan in the Region reached is HTTP 400. Hypothesis: the Region-scoped read rejects an ARN it cannot resolve in the credential-scoped Region as an invalid argument (400) rather than as a missing resource (404). The replay compared only the status key for this step (divergence key `status=400->404|keys=status`), so the error code and message the emulator serialized are not themselves reported as diverging; the required message text for the 400 class is AWS's fixed wording for that error with the plan ARN and Region taken from the request and from the service's state — never the literal ARN of any one recording.
Emulator: answers HTTP 404 with `ResourceNotFoundException`, built in the handler's `store.plans.get(&input.arn)` miss arm (and the following `is_available_in_region` arm).
Why: an SDK client branches on the modeled exception type the status selects. A caller that catches the service's 400 argument error to surface "this plan is not usable from this Region" never sees it, and a caller that retries or falls back on 404 `ResourceNotFoundException` takes a path AWS never puts it on; code that distinguishes "bad argument" from "resource absent" inverts.
Fix: the GetPlanInRegion handler's unresolved-ARN error mapping — the arm that selects which modeled error to return when the ARN resolves to no plan available in the request's Region; the modeled-error set for the operation has to admit the 400 shape it must now return (today the enum is widened only with `InternalServerException` via `extra_errors`).

Cause 2 of 2: ListRoute53HealthChecksInRegion (fixture arc-region-switch-list-route53-health-checks-in-region-not-found, step task#1).
AWS: Smithy (com.amazonaws.arcregionswitch#ListRoute53HealthChecksInRegion) models `IllegalArgumentException` @httpError 400 ("The request processing has an invalid argument") alongside `AccessDeniedException` 403, `ResourceNotFoundException` 404 and `InternalServerException` 500; the recording shows HTTP 400 for an `arn` that resolves to no plan from the Region reached, i.e. the modeled 400 argument error, not the 404. Message requirement: AWS's fixed text for that error class, with the variable parts (the plan ARN, the Region) taken from the request and the emulator's state.
Emulator: answers HTTP 404 `ResourceNotFoundException` from the handler's `store.plans.get(&input.arn)` miss arm; it only reaches `IllegalArgumentException` (400) for pagination-token problems.
Why: same client break as cause 1, and here it is worse-grounded: the operation already models the 400 error the recording uses, so a client written against the model's own error set is handed the wrong one of two shapes it explicitly handles.
Fix: the ListRoute53HealthChecksInRegion handler's unresolved-ARN arm — return the modeled 400 `IllegalArgumentException` for an ARN that names no plan resolvable in the request's Region, reserving 404 for whatever condition AWS actually records 404 for.

No open issue covers either cause (no existing arc-region-switch issue is filed); neither divergence carries a `stub_path` or an `owned_by` owner.

Traces: arc-region-switch-get-plan-in-region-not-found#task#1; arc-region-switch-list-route53-health-checks-in-region-not-found#task#1. Component citations: services/service-com-amazonaws-arcregionswitch-arcregionswitch/src/ops/get_plan_in_region.rs lines 163-182; services/service-com-amazonaws-arcregionswitch-arcregionswitch/src/ops/list_route53_health_checks_in_region.rs lines 205-214.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200): steps_total 2, steps_passed 0, steps_mismatched 2, score_pct 0.0.

Cause 1 — fixture arc-region-switch-get-plan-in-region-not-found, step task#1, operation GetPlanInRegion, verdict mismatch:
  expected_status 400, actual_status 404
  detail: "status: expected 400, got 404"
  divergence_key: service=arc-region-switch|GetPlanInRegion|status=400->404|keys=status
  affected: [arc-region-switch-get-plan-in-region-not-found#task#1]
Expected: HTTP 400 (client argument error) for an `arn` that resolves to no plan from the Region reached.
Actual: HTTP 404 with the modeled `ResourceNotFoundException`.

Cause 2 — fixture arc-region-switch-list-route53-health-checks-in-region-not-found, step task#1, operation ListRoute53HealthChecksInRegion, verdict mismatch:
  expected_status 400, actual_status 404
  detail: "status: expected 400, got 404"
  divergence_key: service=arc-region-switch|ListRoute53HealthChecksInRegion|status=400->404|keys=status
  affected: [arc-region-switch-list-route53-health-checks-in-region-not-found#task#1]
Expected: HTTP 400 — the operation's own modeled `IllegalArgumentException`.
Actual: HTTP 404 with the modeled `ResourceNotFoundException`.

Model evidence (services/service-com-amazonaws-arcregionswitch-arcregionswitch/smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json, service ArcRegionSwitch 2022-07-26, awsJson1_0/rpcv2Cbor, endpointPrefix arc-region-switch):
  GetPlanInRegion errors = AccessDeniedException(403), ResourceNotFoundException(404) — no modeled 400.
  ListRoute53HealthChecksInRegion errors = AccessDeniedException(403), IllegalArgumentException(400), InternalServerException(500), ResourceNotFoundException(404).

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it (admin health answers 200; data listener accepts).
2. Replay the wire corpus simparity-wire-aws-v1 (replay_wire_corpus).
3. Cause 1: replay fixture arc-region-switch-get-plan-in-region-not-found, step task#1 — a GetPlanInRegion call whose `arn` is a well-formed PlanArn naming a plan not resolvable from the Region the request's SigV4 scope names. The recording expects HTTP 400; the emulator returns HTTP 404 (ResourceNotFoundException).
4. Cause 2: replay fixture arc-region-switch-list-route53-health-checks-in-region-not-found, step task#1 — a ListRoute53HealthChecksInRegion call with the same kind of unresolvable `arn`. The recording expects HTTP 400 (the operation's modeled IllegalArgumentException); the emulator returns HTTP 404 (ResourceNotFoundException).
5. Both steps must pass (status, error code and message) before this issue is closed; re-replay both fixtures, not just the representative.
