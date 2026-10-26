Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: fa5ed22240e89dcfd43a22a078f364ee68f69599

# Resolve the (provider key, product) registration through the existing state helper in ListProductSubscriptions

_Filed: 2026-10-25T09:18:02Z_
_Operation: ListProductSubscriptions | Class: maintainability_

> _Note: this issue was identified against the code base at commit `fa5ed22240e89dcfd43a22a078f364ee68f69599`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule "a registration is addressed by the `(provider key, Product)` pair and only a LIVE one resolves" is implemented twice in this crate: once in `crate::state::ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState::resolve_identity_provider`, which `StartProductSubscription`, `StopProductSubscription`, `DeregisterIdentityProvider` and `UpdateIdentityProviderSettings` all call, and once inline in `ListProductSubscriptions`'s `Product`-supplied branch, which repeats the same index lookup plus the same `live_identity_provider` filter and then raises the same `StateError::ProviderNotRegistered`. The two copies can drift: a change to what "resolves" means (today: not `DEREGISTERING`) has to be made in both places or this list will report subscriptions through a registration no other operation will act on.

Only the `Product`-supplied branch duplicates the helper; the `Product`-omitted branch legitimately scans the index for every product of the key, which no helper covers, so it stays as it is. Routing the `Some(product)` branch through `resolve_identity_provider(None, Some(&input.identity_provider), Some(product))` yields the identical outcome — the same `StateError::Validation` from `provider_key` and the same `StateError::ProviderNotRegistered` mapped to `ValidationException` carrying `IDENTITY_PROVIDER_NOT_VALID` — so every response and every existing test stays unchanged.

## Evidence

The helper, documented as the one resolution — services/service-com-amazonaws-licensemanagerusersubscriptions-licensemanagerusersubscriptions/src/state.rs:1631-1665:
```
    /// Resolve a registration from the two ways every operation can address one: its
    /// ARN, or the `(IdentityProvider, Product)` pair it was registered with.
    ...
    /// Only a LIVE registration resolves: one whose deregistration was accepted is still
    /// REPORTED in its `DEREGISTERING` status, but it is on its way out, ...
    pub fn resolve_identity_provider(
        ...
        let key = provider_key(identity_provider)?;
        let arn = self
            .identity_provider_index
            .get(&(key, product.to_string()))
            .ok_or(StateError::ProviderNotRegistered)?;
        self.live_identity_provider(arn)
            .ok_or(StateError::ProviderNotRegistered)
    }
```
The second copy — src/ops/list_product_subscriptions.rs:169-200:
```
        let key = crate::state::provider_key(&input.identity_provider).map_err(into_error)?;
        let provider_arns: Vec<String> = match input.product.as_deref() {
            Some(product) => state
                .identity_provider_index
                .get(&(key.clone(), product.to_string()))
                .filter(|arn| state.live_identity_provider(arn).is_some())
                .cloned()
                .into_iter()
                .collect(),
            ...
        if provider_arns.is_empty() {
            return Err(into_error(crate::state::StateError::ProviderNotRegistered));
        }
```
Existing tests that cover the behaviour and must stay green unchanged: tests/all/identity_provider_subscriptions.rs:592 `product_subscriptions_round_trip_and_stop_removes_them`, tests/all/identity_provider_subscriptions.rs:731 `subscriptions_require_a_registration_and_are_unique_per_user`, tests/all/identity_provider_deregistration.rs (its `list_subscriptions` helper, line 57, after a deregistration), and tests/all/filter_operations.rs:162 `product_subscriptions_list`.

## Steps to Reproduce

1. Read src/state.rs:1641-1665 (`resolve_identity_provider`) — index lookup on `(provider key, product)` then `live_identity_provider`, else `StateError::ProviderNotRegistered`.
2. Read src/ops/list_product_subscriptions.rs:170-200 — the same lookup, the same live filter and the same `ProviderNotRegistered` written out again.
3. Change the definition of a resolvable registration in the helper (e.g. also exclude `REGISTERING`) and observe that `ListProductSubscriptions` keeps the old rule, because it never calls the helper.
