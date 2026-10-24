Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 1f3e823b77ae5163683e789c9c079862bba02cf9

# User associations orphaned by a completed deregistration stay in state but are unlistable (404), leaving a hollow instance in the view

_Filed: 2026-10-24T19:30:52Z_
_Operation: ListUserAssociations | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `1f3e823b77ae5163683e789c9c079862bba02cf9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A user association made through a directory outlives the deregistration of that directory's identity provider FOREVER, but becomes invisible to the operation that reports associations.

Expected: a user association exists only through a registered identity provider — `AssociateUser`/`ListUserAssociations` resolve the directory's registration and refuse when it is absent, and License Manager tears down the VPC endpoints and the Active Directory integration a registration provisioned when it is deregistered. So after `DeregisterIdentityProvider` completes, either (a) the associations made through that directory are gone too — the instance leaves this service's view with them — or (b) they still exist and `ListUserAssociations` still reports them. The emulator does neither.

Actual: `begin_identity_provider_deregistration` deliberately leaves the user associations in place (`src/state.rs`, "User ASSOCIATIONS are not removed"), and no code path ever removes them afterwards — the only drops are the per-association disassociation teardown and the test-only `remove_instance_user`. Once the registration record is dropped by the clock, `provider_key_is_registered` answers false for that directory, so:

* `ListUserAssociations` for that `(IdentityProvider, InstanceId)` pair answers HTTP 404 `ResourceNotFoundException` ("identity provider not registered") even though the associations are still held;
* `AssociateUser` for the same user/instance answers the same 404, so the occupied `(provider key, InstanceId, Username)` triple can never be re-used;
* `ListInstances` keeps reporting the instance, with `Status: ACTIVE` and an EMPTY `Products` list, because `apply_instance_view` keeps any instance an association still names while `products_by_provider_key` only counts live registrations — i.e. an instance that "provides user-based subscriptions" is reported as providing none;
* the only way to clear the orphan is `DisassociateUser` with the exact triple or `InstanceUserArn` (that path does not consult the registration), which no client can discover because the association cannot be listed.

## Evidence

Spec: `ListUserAssociations` declares `InstanceId` and `IdentityProvider` as @required and returns `InstanceUserSummaries`; `AssociateUser.IdentityProvider` is "The identity provider for the user" and `InstanceSummary.Products` is @required and documented as "A list of provided user-based subscription products" (smithy_query operation=ListUserAssociations/AssociateUser, shape=InstanceSummary) — an instance in the view provides at least one product. Service semantics: deregistering an identity provider removes the integration every association through that directory depends on, so no association can survive it as a resource the service still holds.

Code: `src/state.rs` `begin_identity_provider_deregistration` (~line 1617) removes the product users, the license server endpoints and the tags of the registration, and explicitly keeps the user associations; `drop_identity_provider` (~line 1677) likewise only calls `remove_product_users_of`. `grep remove_instance_user|drop_instance_users` over `src/` shows the only production drop is the disassociation teardown in `materialise_due` (~line 831). `src/ops/list_user_associations.rs` returns `ResourceNotFoundException` when `state.provider_key_is_registered(&key)` is false (lines ~196-203); `src/ops/associate_user.rs` does the same (lines ~199-210). `src/state.rs` `apply_instance_view` (~line 1440) keeps the instance record whenever `naming` is non-empty and sets `products` from `products_by_provider_key`, which filters to live registrations (~line 1419).

Tests: `tests/all/resource_resolution_refusals.rs::the_instance_view_is_recomputed_from_the_associations_that_remain` and `tests/all/identity_provider_subscriptions.rs` cover a registration removed while associations remain only for the PRODUCTS list, and `tests/all/user_associations.rs` covers the 404 only for a directory that was never registered; no test asserts anything about an association whose registration has been deregistered, nor about an instance reported with an empty `Products` list.

## Steps to Reproduce

1. Register an identity provider for an Active Directory directory D and Product=OFFICE_PROFESSIONAL_PLUS; note the IdentityProviderArn.
2. AssociateUser with Username=alice, that IdentityProvider and an authorized InstanceId; observe 200 with Status=ASSOCIATING.
3. Call ListUserAssociations for (D, that InstanceId) and observe the association is reported.
4. Call DeregisterIdentityProvider for (D, OFFICE_PROFESSIONAL_PLUS) and wait for the DEREGISTERING teardown to complete (the registration stops being reported by ListIdentityProviders).
5. Call ListUserAssociations for (D, that InstanceId) again. Expected: either an empty list (the association was torn down with the registration) or the association as it stands. Actual: HTTP 404 ResourceNotFoundException, while the association is still held in state.
6. Call ListInstances. Actual: the instance is still reported with Status=ACTIVE and Products: [] — an instance providing no user-based subscription at all. Expected: the instance has left the view with the associations the deregistration tore down.
7. Call AssociateUser again for the same (D, InstanceId, alice). Actual: HTTP 404 — the triple is permanently occupied by an association nothing can list.
