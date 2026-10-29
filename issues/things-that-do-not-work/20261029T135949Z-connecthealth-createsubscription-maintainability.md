Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 3c3c87ac9166712e1ad44c6d25ef542a1ff1b247

# CreateSubscription hand-builds the stored-subscription projection that state.rs single-homes in Subscription::description(), so the create response and the four other projections can drift

_Filed: 2026-10-29T13:59:49Z_
_Operation: CreateSubscription | Class: maintainability_

> _Note: this issue was identified against the code base at commit `3c3c87ac9166712e1ad44c6d25ef542a1ff1b247`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`state.rs` names itself the one home for "every response projection", `SubscriptionDescription` first among them, and `Subscription::description()` is that projection: it reads the eight members of a stored subscription out of the record, converting the status through `as_wire()` and each instant through `to_timestamp`. `ActivateSubscription`, `DeactivateSubscription`, `GetSubscription` and `ListSubscriptions` all call it.

`CreateSubscription::handle` writes the same eight-member projection out by hand instead. `CreateSubscriptionOutput` is a distinct Rust type from `SubscriptionDescription`, but it is member-for-member the same shape (`domainId`, `subscriptionId`, `arn`, `status`, `createdAt`, `lastUpdatedAt`, `activatedAt`, `deactivatedAt`), and the handler's block applies the same three conversions to the same fields of the same record. So the rule "this is how a stored subscription is reported" exists twice in this crate, and the copies can disagree about any one of the eight members — which is exactly the drift this crate has already been filed and fixed for three times over, on the `EncryptionContext` projection (connecthealth-getdomain-maintainability), the recorded `accountId`/`onboardingGeneration` projection (connecthealth-createdomain-maintainability--r2) and the subscription status step (connecthealth-deactivatesubscription-maintainability).

The fix is local and response-preserving: build the create's output from `stored.description()` (or move that mapping next to `description()` in `state.rs`), so the create response and the four other projections are one computation. No response byte and no existing assertion changes.

## Evidence

The crate's statement of where this projection belongs — `services/service-com-amazonaws-connecthealth-connecthealth/src/state.rs:10-16`:
```
//! Helpers several operations share (id minting, ARN building, the opaque pagination
//! cursor, the one status step a `Subscription` transition takes, and every response
//! projection — `SubscriptionDescription`,
//! `WebAppConfiguration`, `EncryptionContext`, and the recorded `accountId` /
//! `onboardingGeneration` pair) live here rather than being copied
//! into each `src/ops/*.rs`: `src/lib.rs` is generated, so this module is the crate's
//! one shared, editable home.
```

Copy 1, the single-homed projection — `services/service-com-amazonaws-connecthealth-connecthealth/src/state.rs:780-792`:
```
    /// This record as the modelled `SubscriptionDescription` four operations return.
    pub fn description(&self) -> SubscriptionDescription {
        SubscriptionDescription {
            domain_id: self.domain_id.clone(),
            subscription_id: self.subscription_id.clone(),
            arn: self.arn.clone(),
            status: self.status.as_wire().to_string(),
            created_at: to_timestamp(self.created_at),
            last_updated_at: to_timestamp(self.last_updated_at),
            activated_at: self.activated_at.map(to_timestamp),
            deactivated_at: self.deactivated_at.map(to_timestamp),
        }
    }
```

Copy 2, the same computation written out in the handler — `services/service-com-amazonaws-connecthealth-connecthealth/src/ops/create_subscription.rs:229-238`:
```
        Ok(CreateSubscriptionOutput {
            domain_id: stored.domain_id.clone(),
            subscription_id: stored.subscription_id.clone(),
            arn: stored.arn.clone(),
            status: stored.status.as_wire().to_string(),
            created_at: crate::state::to_timestamp(stored.created_at),
            last_updated_at: crate::state::to_timestamp(stored.last_updated_at),
            activated_at: stored.activated_at.map(crate::state::to_timestamp),
            deactivated_at: stored.deactivated_at.map(crate::state::to_timestamp),
        })
```

The two output types hold the identical member set, so the two copies project one record — `src/ops/types/create_subscription_types.rs:20-31`:
```
#[smithy_macros::smithy(shape = "com.amazonaws.connecthealth#CreateSubscriptionOutput")]
#[derive(Debug, Clone)]
pub struct CreateSubscriptionOutput {
    pub domain_id: String,
    pub subscription_id: String,
    pub arn: String,
    pub status: String,
    pub created_at: ::aws_smithy_types::DateTime,
    pub last_updated_at: ::aws_smithy_types::DateTime,
    pub activated_at: Option<::aws_smithy_types::DateTime>,
    pub deactivated_at: Option<::aws_smithy_types::DateTime>,
}
```
and `smithy_query(query="shape", shape="SubscriptionDescription")` lists exactly `domainId`, `subscriptionId`, `arn`, `status`, `createdAt`, `lastUpdatedAt`, `activatedAt`, `deactivatedAt`.

The sibling handlers that do route through the shared projection: `src/ops/activate_subscription.rs:191`, `src/ops/deactivate_subscription.rs:188`, `src/ops/get_subscription.rs:175` (`subscription: Some(subscription.description())`) and `src/ops/list_subscriptions.rs:204` (`.map(crate::state::Subscription::description)`).

Test coverage that must stay unchanged — `tests/all/domain_subscription_lifecycle.rs:1219-1300`, whose own comment states the rule the two copies have to keep agreeing on:
```
    //   * a subscription is created in the enabled status `CreateSubscription` puts it
    //     in ("Creates a new Subscription within a Domain for billing and user
    //     management"), and EVERY projection of the stored record — the create response,
    //     `GetSubscription`, `ListSubscriptions`, and the transition responses — reports
    //     that one persisted status;
```
plus `tests/all/subscription_activation_stamp.rs`, `tests/all/subscription_transition_step.rs` and `tests/all/shared_pagination_policy.rs` (ListSubscriptions page contents).

## Steps to Reproduce

1. Read services/service-com-amazonaws-connecthealth-connecthealth/src/state.rs:10-16 — the crate declares `state.rs` the one home for every response projection, naming `SubscriptionDescription` — and state.rs:780-792, where that projection lives.
2. Read src/ops/create_subscription.rs:229-238: the same eight members of the same stored record, with the same `as_wire()` / `to_timestamp` conversions, written out a second time in the handler.
3. Compare src/ops/activate_subscription.rs:191, deactivate_subscription.rs:188, get_subscription.rs:175 and list_subscriptions.rs:204, which all project the record through `Subscription::description()` — CreateSubscription is the only subscription responder that does not.
4. Confirm with smithy_query(query="shape", shape="SubscriptionDescription") and src/ops/types/create_subscription_types.rs:20-31 that the two output shapes carry the identical member set, so a change to one projection has to be mirrored in the other by hand.
5. Run the crate's test target (tests/all/domain_subscription_lifecycle.rs, tests/all/subscription_activation_stamp.rs, tests/all/shared_pagination_policy.rs) before and after collapsing the handler's block onto `stored.description()` to confirm every response and every assertion is unchanged.
