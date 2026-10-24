Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: ba63228bbc1032132c1fbdbb0b54a8c844ce19f7

# Deregistered identity provider vanishes instantly instead of being observable in DEREGISTERING

_Filed: 2026-10-24T03:46:38Z_
_Operation: DeregisterIdentityProvider | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ba63228bbc1032132c1fbdbb0b54a8c844ce19f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeregisterIdentityProvider` removes the registration from the store synchronously (`state.remove_identity_provider(&arn)`), and only then stamps `status = DEREGISTERING` on the detached copy it returns.

Expected: deregistration is asynchronous — License Manager has to tear down the VPC endpoints the registration provisioned — which is exactly why the operation answers the non-terminal `DEREGISTERING`. After the call returns, a client that polls `ListIdentityProviders` must still see that registration with `Status = DEREGISTERING` until the teardown completes, and only then stop seeing it; and while it is still deregistering, a `RegisterIdentityProvider` for the same (IdentityProvider, Product) pair is a conflict rather than a fresh success. The emulator already models the mirror-image transition (`REGISTERING -> REGISTERED`) through `schedule_transition`/`materialise_due`, so the machinery for a clock-owed teardown exists.

Actual: the single deregister response is the only place `DEREGISTERING` is ever observable. `ListIdentityProviders` reports nothing for that registration on the very next call, its product users and license server endpoints are destroyed in the same breath, and an immediate re-register of the same (provider, product) pair succeeds with a brand-new ARN. A client that polls for the teardown to finish sees the resource disappear between two reads with no transitional state.

## Evidence

Spec: `IdentityProviderSummary$Status` is the registration's status and `DeregisterIdentityProvider` is documented as removing the registration while returning a summary (OPERATIONS.md: output `IdentityProviderSummary` required); the handler itself chooses the non-terminal wording `DEREGISTERING` (`src/state.rs`: "The status returned by `DeregisterIdentityProvider` for the registration it removed"), a status that is meaningless unless the resource is observable in it — the same reasoning already filed and accepted for DeleteLicenseServerEndpoint/DELETING. Handler: `src/ops/deregister_identity_provider.rs` lines 166-184; state: `src/state.rs::remove_identity_provider` drops the record, the order list, both indexes, the dependent product users, the dependent license server endpoints and the tags in one step, and `materialise_due` only ever transitions REGISTERING -> REGISTERED. Tests: `tests/all/identity_provider_subscriptions.rs` (around line 1250) asserts the reply says DEREGISTERING and that the registration is gone immediately; nothing asserts a DEREGISTERING registration is reported by ListIdentityProviders.

## Steps to Reproduce

1. RegisterIdentityProvider for an Active Directory provider with Product=OFFICE_STANDARD, and wait until ListIdentityProviders reports Status=REGISTERED.
2. Call DeregisterIdentityProvider with the same IdentityProvider and Product — the reply carries IdentityProviderSummary.Status=DEREGISTERING.
3. Immediately call ListIdentityProviders: the registration is absent. Expected: it is still listed with Status=DEREGISTERING until the teardown the status announces completes.
4. Immediately call RegisterIdentityProvider again for the same (IdentityProvider, Product): it succeeds with a new ARN. Expected: a conflict while the previous registration is still deregistering.
