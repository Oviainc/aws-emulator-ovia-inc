Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 15aaa5471fc3f7278352228b97d2216e65bea584

# service.rs documents the unrouted-operation default as awsJson -> 404, but this crate's own test pins 501

_Filed: 2026-10-28T14:45:07Z_
_Operation: ListRecommendedActions | Class: maintainability_

> _Note: this issue was identified against the code base at commit `15aaa5471fc3f7278352228b97d2216e65bea584`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`src/service.rs` is this crate's hand-owned seam file ("EDIT THIS FILE. It is emitted once with working defaults and is never regenerated", service.rs:6-8). Two of its comments state, as the rule a future author will trust when deciding whether to override the seam, that a request that reaches this service but names no modeled operation is answered 404 because the service's protocol is awsJson. That is not what the code does: the crate's own end-to-end test asserts 501 for exactly that request, and the framework path the comment points at documents awsJson/restJson1 unrouted-operation as 501 `NotImplemented`.

So the only two places in this crate that state the status of the routing-error default disagree with the only place that exercises it. An author reading `override_routing_error` to decide whether this service needs its own envelope is told the default is 404 and will reason about a status the service never emits; whoever later notices the 501 cannot tell from the crate whether the status or the comment is the mistake.

The code and the response are correct and must not be touched: the framework states the 501 is deliberate ("the 501 elsewhere is the emulator's deliberate 'modeled but NOT REGISTERED' signal", vendor/smithy-server-runtime/src/pre_dispatch.rs:38-39). The defect is the stale claim in the two comments. Fixing it changes no response and no existing test.

## Evidence

Claim side, in this crate:
- services/.../src/service.rs:29-32 — "//! - `override_routing_error` — how this service words a request that reached it / //!   but named no modeled operation. The default returns `None`, so the entry / //!   renders the protocol-correct default (query -> 400 InvalidAction, awsJson -> / //!   404, …)."
- services/.../src/service.rs:265-268 — "// naming no operation ... Left to the entry's protocol-correct default (awsJson -> / // 404 in the media type the model's protocol declares)"

Behaviour side, in this crate's own test:
- services/.../tests/all/list_recommended_actions.rs:1202-1209 —
  "        (
            \"an unroutable target\",
            json_1_0,
            \"AWSBillingAndCostManagementRecommendedActions.NoSuchOperation\",
            json!({}),
            501,
            json_1_0,
        ),"
  asserted at list_recommended_actions.rs:1216-1219 ("expected HTTP {expected_status}").

Framework path the comment names:
- vendor/smithy-server-runtime/src/pre_dispatch.rs:30 — "//! | awsJson / restJson1 | **501** `NotImplemented` | no (falls to `InternalServerError`) | caller-supplied |" (the 404 row is rpcv2Cbor, pre_dispatch.rs:28).
- The same stale "awsJson -> 404" wording the service file copied lives in the generator at vendor/smithy-macros/src/lib.rs:4321-4322 ("(query -> 400 InvalidAction, awsJson -> 404, / rpcv2Cbor -> 404, rest -> 501)").

Test coverage of the behaviour that must stay unchanged: `every_response_announces_the_services_media_type` (tests/all/list_recommended_actions.rs:1128) covers the unroutable target (501, media type application/x-amz-json-1.0); the other suites in tests/all/ cover success, pagination and refusals and are untouched by a comment correction.

## Steps to Reproduce

1. Read services/service-com-amazonaws-bcmrecommendedactions-awsbillingandcostmanagementrecommendedactions/src/service.rs lines 29-32 and 260-271: both say the unrouted-operation default for this awsJson service is 404.
2. Read tests/all/list_recommended_actions.rs lines 1202-1219: the case "an unroutable target" (X-Amz-Target: AWSBillingAndCostManagementRecommendedActions.NoSuchOperation) expects HTTP 501.
3. Read vendor/smithy-server-runtime/src/pre_dispatch.rs line 30: awsJson/restJson1 unrouted operation is 501 NotImplemented; the 404 row (line 28) is rpcv2Cbor.
4. Conclude the crate's own comment and its own test state different statuses for the same failure mode; only the comment is wrong.
