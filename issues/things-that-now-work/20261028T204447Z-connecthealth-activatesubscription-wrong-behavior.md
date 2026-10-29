Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 0e1d5242aac04ba2a44c3cb18c53433536c8022f

# ActivateSubscription never stamps activatedAt/lastUpdatedAt on an already-ACTIVE subscription — which is every freshly created one — and justifies the skip with an @idempotent trait the model does not declare for it

_Filed: 2026-10-28T20:44:47Z_
_Operation: ActivateSubscription | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `0e1d5242aac04ba2a44c3cb18c53433536c8022f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler's own `IMPLEMENTED:` header states that `ActivateSubscription` "moves the PERSISTED subscription to `ACTIVE`, stamping `activatedAt` and `lastUpdatedAt`". The code does neither whenever the record is already `ACTIVE`: both writes sit inside `if subscription.status != SubscriptionStatus::Active { .. }`.

That branch is skipped on the ordinary path, because `CreateSubscription` deliberately creates the subscription in `ACTIVE` with `activated_at: None`. So the first — and for most clients the only — `ActivateSubscription` call a subscription ever receives answers 200 while writing nothing: `activatedAt` stays absent and `lastUpdatedAt` does not move. `activatedAt` is reachable only if the caller first calls `DeactivateSubscription`, which is the opposite of what the state module says the member means: `activated_at` / `deactivated_at` are documented there as "a HISTORY of the explicit `ActivateSubscription` / `DeactivateSubscription` transitions, not a view of `status`", yet the only writer is gated on `status` changing.

The comment that justifies the skip cites a trait this operation does not have. `smithy_query(query="operation", operation="ActivateSubscription,DeactivateSubscription")` reports `"idempotent": true` for `DeactivateSubscription` only; `ActivateSubscription` carries no `idempotent` trait at all (OPERATIONS.md likewise prints a `**Traits:** @idempotent` line for DeactivateSubscription and none for ActivateSubscription). The sibling `DeactivateSubscription` handler makes the same no-op argument and there it IS backed by the model — so the two handlers look symmetric only because one of them asserts a model fact that is false.

## Evidence

Handler doc vs. handler code, same file:

`services/service-com-amazonaws-connecthealth-connecthealth/src/ops/activate_subscription.rs:4-9`:
```
//! IMPLEMENTED: moves the PERSISTED subscription to `ACTIVE`, stamping `activatedAt`
//! and `lastUpdatedAt`, and returns the stored `SubscriptionDescription`. Any
//! `deactivatedAt` the record already carries is RETAINED: the stamps are a history of
//! the transitions that happened, not a view of the current status. `@idempotent`:
//! repeating the call on an already-active subscription changes nothing and still
//! answers 200. An unknown domain or subscription is a 404.
```
`services/service-com-amazonaws-connecthealth-connecthealth/src/ops/activate_subscription.rs:180-190`:
```
        // `@idempotent`: activating an already-active subscription is a no-op, so the
        // stamps it already carries stay put rather than being rewritten per call.
        ...
        if subscription.status != crate::state::SubscriptionStatus::Active {
            subscription.status = crate::state::SubscriptionStatus::Active;
            subscription.activated_at = Some(now);
            subscription.last_updated_at = now;
        }
```

The state the branch is never entered from, written by the sibling create — `services/service-com-amazonaws-connecthealth-connecthealth/src/ops/create_subscription.rs:214-220`:
```
            status: crate::state::SubscriptionStatus::Active,
            created_at: now,
            last_updated_at: now,
            // `activatedAt` / `deactivatedAt` record the explicit transition calls, so a
            // subscription that has never been activated or deactivated carries neither;
            // the initial status is not one of those transitions.
            activated_at: None,
```

The member's documented meaning in this crate — `services/service-com-amazonaws-connecthealth-connecthealth/src/state.rs:648-651`:
```
/// `activated_at` / `deactivated_at` are a HISTORY of the explicit
/// `ActivateSubscription` / `DeactivateSubscription` transitions, not a view of
/// `status`: each is stamped when its transition happens and is never cleared, so both
/// boundaries of a billing interval remain readable after the opposite transition.
```

The model, contradicting the `@idempotent` justification — `smithy_query(query="operation", operation="ActivateSubscription,DeactivateSubscription")` returns for `ActivateSubscription`: `"http": {"method": "POST", "uri": "/domains/{domainId}/subscriptions/{subscriptionId}/activate", "code": 200}` with NO `idempotent` key, while `DeactivateSubscription` ends with `"idempotent": true`. `services/service-com-amazonaws-connecthealth-connecthealth/OPERATIONS.md:48-52` prints `- **Traits:** @idempotent` under `## DeactivateSubscription` and prints no Traits line under `## ActivateSubscription` (lines 26-31).

Test coverage: `tests/all/domain_subscription_lifecycle.rs` and `tests/all/domain_liveness_agreement.rs` exercise ActivateSubscription only after a DeactivateSubscription (the branch where the status does change) — no test calls activate on a freshly created, never-deactivated subscription, so this path is unpinned.

## Steps to Reproduce

1. POST /domain with {"name":"clinic"} and capture domainId.
2. POST /domains/{domainId}/subscriptions and observe the 201: status=ACTIVE, activatedAt absent.
3. POST /domains/{domainId}/subscriptions/{subscriptionId}/activate.
4. Observe the 200 body: subscription.activatedAt is still absent and subscription.lastUpdatedAt is unchanged from the create, although the handler's own header says the call stamps both.
5. GET /domains/{domainId}/subscriptions/{subscriptionId} and confirm activatedAt is still absent — the member is unreachable unless the caller first deactivates.
