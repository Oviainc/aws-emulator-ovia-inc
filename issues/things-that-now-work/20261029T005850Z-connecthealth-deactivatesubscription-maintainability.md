Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: fe42665d4d71982cabac275c755b64c4cdb40af6

# The guarded subscription status transition is implemented twice, once per handler, while every other Subscription rule is single-homed in state.rs

_Filed: 2026-10-29T00:58:50Z_
_Operation: DeactivateSubscription | Class: maintainability_

> _Note: this issue was identified against the code base at commit `fe42665d4d71982cabac275c755b64c4cdb40af6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The subscription status machine's one step — "if the record is not already in the target status, move it there, stamp the instant of that transition, and move lastUpdatedAt" — is written out twice, once inside `ActivateSubscription::handle` and once inside `DeactivateSubscription::handle`. Only the target status and which stamp field is written differ; the guard, the two writes and the `last_updated_at` assignment are the same computation.

This is the one `Subscription` rule that is NOT single-homed. `state.rs` states explicitly that helpers several operations share live in that module rather than being copied into each `src/ops/*.rs`, and both of the other per-record `Subscription` rules already obey it: liveness is `Subscription::is_live()` and the response projection is `Subscription::description()` — which these same two handlers call. The transition is the exception, and the two copies can (and already do) disagree about the rule they implement: the `ActivateSubscription` copy justifies its guard with an `@idempotent` trait the model declares only for `DeactivateSubscription`, so the two blocks look symmetric only because one of them asserts a model fact the other's model actually carries.

A single parameterised step on the record — e.g. `Subscription::transition_to(status, stamp, now)` beside `is_live()` and `description()` in `state.rs` — removes the second copy. Every response byte and every existing assertion stays identical: the guard, the target statuses, the stamped fields and the projection are unchanged; only the number of places the rule is written drops from two to one.

## Evidence

Copy 1 — `services/service-com-amazonaws-connecthealth-connecthealth/src/ops/activate_subscription.rs:181-191`:
```
        // `@idempotent`: activating an already-active subscription is a no-op, so the
        // stamps it already carries stay put rather than being rewritten per call.
        //
        // `deactivated_at` is deliberately NOT cleared here: each stamp records that its
        // transition happened, so re-activating keeps the previous deactivation boundary
        // readable and a caller can still reconstruct the inactive interval.
        if subscription.status != crate::state::SubscriptionStatus::Active {
            subscription.status = crate::state::SubscriptionStatus::Active;
            subscription.activated_at = Some(now);
            subscription.last_updated_at = now;
        }
```

Copy 2 — `services/service-com-amazonaws-connecthealth-connecthealth/src/ops/deactivate_subscription.rs:180-190`:
```
        // `@idempotent`: deactivating an already-inactive subscription is a no-op, so
        // the stamps it already carries stay put rather than being rewritten per call.
        //
        // `activated_at` is deliberately NOT cleared here, mirroring
        // `ActivateSubscription`: each stamp records that its transition happened, so
        // both boundaries of the billing interval stay readable.
        if subscription.status != crate::state::SubscriptionStatus::Inactive {
            subscription.status = crate::state::SubscriptionStatus::Inactive;
            subscription.deactivated_at = Some(now);
            subscription.last_updated_at = now;
        }
```

The crate's own statement of where a shared rule belongs — `services/service-com-amazonaws-connecthealth-connecthealth/src/state.rs:10-15`:
```
//! Helpers several operations share (id minting, ARN building, the opaque pagination
//! cursor, and every response projection — `SubscriptionDescription`,
//! `WebAppConfiguration`, `EncryptionContext`, and the recorded `accountId` /
//! `onboardingGeneration` pair) live here rather than being copied
//! into each `src/ops/*.rs`: `src/lib.rs` is generated, so this module is the crate's
//! one shared, editable home.
```

The two sibling `Subscription` rules that DO obey it, in the same `impl` the transition would join — `services/service-com-amazonaws-connecthealth-connecthealth/src/state.rs:744-763`:
```
impl Subscription {
    /// Whether this record is still live: one `DeleteDomain` already marked `DELETED`
    /// is gone as far as every reader and every transition is concerned.
    pub fn is_live(&self) -> bool {
        self.status != SubscriptionStatus::Deleted
    }

    /// This record as the modelled `SubscriptionDescription` four operations return.
    pub fn description(&self) -> SubscriptionDescription {
```
Both handlers above already route through `subscription.description()` (activate_subscription.rs:194, deactivate_subscription.rs:193), so the projection half of the pair is shared and only the transition half is duplicated.

Proof the copies have already drifted: the `ActivateSubscription` copy's comment cites `@idempotent`, which `OPERATIONS.md:51` prints only for the sibling — `- **Traits:** @idempotent` under `## DeactivateSubscription` — while `OPERATIONS.md:26-31` (`## ActivateSubscription`) prints no Traits line at all.

Test coverage that must stay unchanged: `services/service-com-amazonaws-connecthealth-connecthealth/tests/all/domain_subscription_lifecycle.rs` (end-to-end create -> deactivate -> activate over the router's own dispatch, asserting the stored record each read path reports) and `services/service-com-amazonaws-connecthealth-connecthealth/tests/all/domain_liveness_agreement.rs` (the same transitions against a domain's liveness window).

## Steps to Reproduce

1. Open services/service-com-amazonaws-connecthealth-connecthealth/src/ops/activate_subscription.rs and read lines 181-191.
2. Open services/service-com-amazonaws-connecthealth-connecthealth/src/ops/deactivate_subscription.rs and read lines 180-190; note the identical guard/assign/stamp/lastUpdatedAt shape with only the target status and the stamp field substituted.
3. Open services/service-com-amazonaws-connecthealth-connecthealth/src/state.rs lines 744-763 and observe that `Subscription::is_live()` and `Subscription::description()` — the other two per-record Subscription rules, both called by those same handlers — are single-homed there, and that state.rs:10-15 names that module as the crate's one shared home for a rule several operations share.
4. Compare the two comments: the activate copy justifies its guard with `@idempotent`, a trait OPERATIONS.md:51 prints only under `## DeactivateSubscription` and OPERATIONS.md:26-31 prints nowhere under `## ActivateSubscription` — the copies already state different rules.
5. Run the crate's test target (tests/all/domain_subscription_lifecycle.rs, tests/all/domain_liveness_agreement.rs) before and after collapsing the two copies into one `Subscription` method to confirm every response and every assertion is unchanged.
