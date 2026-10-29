Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: fed81ce974dc846a43fcb0778ceaef82f64eb9ba

# AssociateUser states the provider-not-registered refusal twice, and the two copies have drifted to different sentences

_Filed: 2026-10-29T02:54:13Z_
_Operation: AssociateUser | Class: maintainability_

> _Note: this issue was identified against the code base at commit `fed81ce974dc846a43fcb0778ceaef82f64eb9ba`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

AssociateUser produces the user-association plane's "identity provider is not registered" refusal in two places: the inline branch after `provider_key_is_registered`, which carries `IDENTITY_PROVIDER_NOT_REGISTERED`, and the `StateError::ProviderNotRegistered` arm of its `into_error`, which carries the OTHER plane's sentence `IDENTITY_PROVIDER_NOT_VALID`. The comment above that arm says the arm exists precisely so "the two paths cannot word one refusal two ways" — but they do word it two ways. The sibling handler for the same resolution, `ListUserAssociations`, keeps both of its copies on `IDENTITY_PROVIDER_NOT_REGISTERED`, so the drift is in AssociateUser alone.

There is no behavioural impact today: the arm is unreachable, as its own comment states, because the only state-layer calls `into_error` is applied to in this handler (`provider_key`, `validate_tags`, `merge_tags`) can only yield `StateError::Validation`. Routing the inline branch through `into_error(StateError::ProviderNotRegistered)` and giving that arm the plane's own sentence leaves the refusal a caller sees — and every existing test — unchanged, while making the rule exist once.

## Evidence

Copy 1 — src/ops/associate_user.rs:210-222:
```
        if !state.provider_key_is_registered(&key) {
            return Err(AssociateUserErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    // The one sentence this service's user-association plane states this
                    // refusal with, so this handler and `ListUserAssociations` — which
                    // resolves the same directory the same way — cannot word one refusal
                    // two ways.
                    message: Some(crate::state::IDENTITY_PROVIDER_NOT_REGISTERED.to_string()),
```
Copy 2 — src/ops/associate_user.rs:341-352, same handler:
```
        // ... The
        // lookup therefore cannot report this here; the arm keeps that same class so the
        // two paths cannot word one refusal two ways.
        crate::state::StateError::ProviderNotRegistered => {
            AssociateUserErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
                message: Some(crate::state::IDENTITY_PROVIDER_NOT_VALID.to_string()),
            })
```
The sibling keeps one sentence — src/ops/list_user_associations.rs:289-295:
```
        crate::state::StateError::ProviderNotRegistered => {
            ListUserAssociationsErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: Some(crate::state::IDENTITY_PROVIDER_NOT_REGISTERED.to_string()),
```
And the constants' own docs say which plane owns which sentence — src/state.rs:353-362: "What the USER-ASSOCIATION plane says about an identity provider that no LIVE registration of this account is for. One sentence in one place, because it is one resolution: `AssociateUser` creates an association through the registration of the directory it names and `ListUserAssociations` reports the associations of that same directory ... and both answer their modelled `ResourceNotFoundException` carrying this." versus src/state.rs:345-351 for `IDENTITY_PROVIDER_NOT_VALID`: "What the operations that RESOLVE an identity-provider reference say ... `CreateLicenseServerEndpoint` and `UpdateIdentityProviderSettings` both refuse an unresolvable reference with it".

Tests covering the reachable copy (unchanged by the fix): tests/all/user_associations.rs:1036-1041 asserts `ResourceNotFoundException` with `IDENTITY_PROVIDER_NOT_REGISTERED`, and tests/all/associations_follow_their_registration.rs:307-312 asserts the same pair.

## Steps to Reproduce

1. Read src/ops/associate_user.rs:210-222 — the inline refusal carries `crate::state::IDENTITY_PROVIDER_NOT_REGISTERED` under a comment claiming one sentence for this refusal.
2. Read src/ops/associate_user.rs:341-352 — the `StateError::ProviderNotRegistered` arm of the same file's `into_error` carries `crate::state::IDENTITY_PROVIDER_NOT_VALID` under a comment claiming the two paths cannot word one refusal two ways.
3. Compare with src/ops/list_user_associations.rs:289-295, where the same arm carries `IDENTITY_PROVIDER_NOT_REGISTERED`, and with the constants' docs at src/state.rs:345-362 that assign `IDENTITY_PROVIDER_NOT_VALID` to the registration-resolving plane only.
4. Confirm the arm is unreachable in this handler: `into_error` is only applied to `provider_key`, `validate_tags` and `merge_tags`, none of which returns `StateError::ProviderNotRegistered` (src/state.rs:294-309, 1524-1549, 2319-2340).
