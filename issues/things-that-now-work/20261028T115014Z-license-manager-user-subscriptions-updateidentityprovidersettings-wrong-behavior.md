Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 0c9cda5ef496fef3a9fe3e02c7d9493d80d1134a

# UpdateIdentityProviderSettings writes settings back after the EC2 hop without re-resolving the registration, so it mutates a registration that is being torn down and loses a concurrent update

_Filed: 2026-10-28T11:50:14Z_
_Operation: UpdateIdentityProviderSettings | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `0c9cda5ef496fef3a9fe3e02c7d9493d80d1134a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateIdentityProviderSettings` is the only handler in this crate that releases the service-state lock across an `await` and then re-takes it to write. The pre-hop read goes through `state.resolve_identity_provider(..)`, which is the crate's single liveness-filtered resolution (it funnels through `live_identity_provider`, which excludes a registration whose deregistration was accepted). The post-hop WRITE does not: it reaches straight into the map with `state.identity_providers.get_mut(&arn)`, which is not filtered at all, and it also does not call `materialise_due` on the second lock (only the first lock does).

Two consequences, both of the same class:

1. Liveness is not re-checked. A `DeregisterIdentityProvider` that lands while `resolve_settings_network` is in flight leaves the record present in `identity_providers` in the non-terminal `DEREGISTERING` status (the deregistration deliberately does NOT drop the record). `get_mut` therefore succeeds, `record.settings = updated` is applied to a registration that is being torn down, and the handler answers 200 with that registration's summary — the exact opposite of the rule the crate states and tests for every other operation that acts on a registration. The inline comment above the `get_mut` even claims this case is refused ("a reference that no longer resolves is the same refusal ... for one that never did"), which `get_mut` cannot deliver because it applies no filter.

2. The read-modify-write loses a concurrent update. `settings` is cloned under the first lock, `apply_update_settings` folds `AddSubnets`/`RemoveSubnets` into that stale snapshot, and the result is written under the second lock without re-reading. Two overlapping `UpdateIdentityProviderSettings` calls on one registration both start from the same base, so the later write silently discards the subnets the earlier one added — stored state is lost, not merely reordered.

The fix is to resolve and fold under the second lock (i.e. re-run the crate's own `live_identity_provider`/`resolve_identity_provider` plus `materialise_due` and `apply_update_settings` against the value actually held at write time), so the write obeys the same liveness rule the read did and no update is based on a snapshot the store has moved past.

## Evidence

The unfiltered post-hop write and the comment it contradicts — `src/ops/update_identity_provider_settings.rs:217-224`:
```
        let mut state = state.lock();
        let record = state.identity_providers.get_mut(&arn).ok_or_else(|| {
            // The registration can be deregistered while the resolution above is in
            // flight: a reference that no longer resolves is the same refusal the
            // resolution before the hop gives for one that never did, not a server fault.
            identity_provider_not_valid()
        })?;
        record.settings = updated;
```

The liveness filter the same handler used BEFORE the hop — `src/ops/update_identity_provider_settings.rs:180-197`:
```
        let (arn, settings) = {
            let mut state = state.lock();
            state.materialise_due(now);
            state
                .resolve_identity_provider(
                    input.identity_provider_arn.as_deref(),
                    ...
                .map(|record| (record.arn.clone(), record.settings.clone()))
```

…which funnels through the crate's one liveness helper — `src/state.rs:1068-1075`:
```
    /// The registration `arn` names, when it names one this account can still act on: a
    /// registration whose deregistration was accepted resolves for nothing, exactly as one
    /// that is gone.
    pub fn live_identity_provider(&self, arn: &str) -> Option<&IdentityProviderRecord> {
        self.identity_providers
            .get(arn)
            .filter(|record| !deregistration_started(&record.status))
    }
```
and `src/state.rs:1882-1885`:
```
        if let Some(arn) = arn {
            return self
                .live_identity_provider(arn)
                .ok_or(StateError::ProviderNotRegistered);
```

The record is deliberately still in the map while `DEREGISTERING`, which is why the unfiltered `get_mut` finds it — `src/state.rs:2046-2050`:
```
    /// Deregistration is ASYNCHRONOUS — License Manager has to tear down the VPC endpoints
    /// the registration provisioned, which is why the operation answers the non-terminal
    /// `DEREGISTERING` — so the record is NOT dropped here: a caller polling
    /// `ListIdentityProviders` keeps seeing the registration in `DEREGISTERING` until the
    /// teardown completes, instead of it vanishing between two reads.
```

The rule the crate states and pins for exactly this operation — `tests/all/identity_provider_deregistration.rs:362-377`:
```
/// RULE: a resource on its way out is REPORTED so a client can watch it go, not so work
/// can be built on it — every operation that acts on a registration refuses one that is
/// being torn down, exactly as it refuses one that is gone, and it is no longer taggable.
#[tokio::test]
async fn a_registration_being_deregistered_is_not_a_live_registration() {
    ...
            (
                "UpdateIdentityProviderSettings",
                update_settings(
```
That test only exercises a deregistration that completed BEFORE the call, so the pre-hop check covers it; nothing covers a deregistration (or a second update) that lands DURING the `resolve_settings_network` hop, which is the window the unfiltered `get_mut` is reached through.

No other handler in the crate takes the lock twice: `grep "state.lock()|\.await" src/ops` shows a second `state.lock()` only in `update_identity_provider_settings.rs` (181 and 217); `associate_user.rs`, `disassociate_user.rs`, `list_user_associations.rs` and `register_identity_provider.rs` all await first and lock exactly once afterwards.

## Steps to Reproduce

1. `RegisterIdentityProvider` for an Active Directory provider with `Settings{Subnets:["subnet-A"], SecurityGroupId:"sg-0123abcd"}` and note the minted `IdentityProviderArn`.
2. Symptom 1 (liveness): issue `UpdateIdentityProviderSettings{IdentityProviderArn, UpdateSettings:{AddSubnets:["subnet-B"], RemoveSubnets:[]}}` and, while its `resolve_settings_network` EC2 hop is in flight (src/ops/update_identity_provider_settings.rs:213-215, a cross-service await), issue `DeregisterIdentityProvider{IdentityProviderArn}`.
3. Observe the update answers 200 with an `IdentityProviderSummary` whose `Status` is `DEREGISTERING` and whose `Settings.Subnets` now contains `subnet-B` — the registration being torn down was mutated. The crate's own rule (tests/all/identity_provider_deregistration.rs:362-364) and the handler's own comment (src/ops/update_identity_provider_settings.rs:219-221) both say such a request is refused with `ValidationException`/`Identity provider is not valid`.
4. Symptom 2 (lost update): with the registration live, issue two overlapping `UpdateIdentityProviderSettings` calls for the same `IdentityProviderArn`, one with `AddSubnets:["subnet-B"]` and one with `AddSubnets:["subnet-C"]`, both entering the handler before either reaches line 217.
5. `ListIdentityProviders` and read the registration's `Settings.Subnets`: only the later writer's subnet is present (e.g. `["subnet-A","subnet-C"]`). Both calls cloned `settings` at line 189 before the hop and wrote the whole value back at line 224 without re-reading, so the first accepted update's stored subnet is gone.
