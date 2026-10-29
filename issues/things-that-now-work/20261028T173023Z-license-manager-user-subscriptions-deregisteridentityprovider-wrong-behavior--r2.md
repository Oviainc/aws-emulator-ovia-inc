Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4a5766ed2caf698065daa3a6a1a461ee27489a64

# DeregisterIdentityProvider ignores Product when the request also carries IdentityProviderArn, tearing down another product's registration (and everything hanging off it)

_Filed: 2026-10-28T17:30:23Z_
_Operation: DeregisterIdentityProvider | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4a5766ed2caf698065daa3a6a1a461ee27489a64`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeregisterIdentityProvider` accepts `IdentityProvider`, `Product` and `IdentityProviderArn`, none of them `@required`. The handler reads `Product` far enough to refuse a value outside `crate::state::PRODUCTS`, and then hands all three members to `crate::state::resolve_identity_provider`, whose ARN branch returns the record that ARN names and compares NOTHING else: neither `Product` nor `IdentityProvider` is checked against the stored registration.

So a request naming `IdentityProviderArn` of the `VISUAL_STUDIO_ENTERPRISE` registration together with `Product: OFFICE_STANDARD` is accepted and answers HTTP 200 with an `IdentityProviderSummary` whose `Product` is `VISUAL_STUDIO_ENTERPRISE` in `DEREGISTERING` — a registration the caller did not name was torn down. Because deregistration cascades (`begin_identity_provider_deregistration` drops every product user of that registration, begins deletion of every license server endpoint created for it, removes its tags and drops the user associations orphaned when it was the directory's last live registration), the mismatch is not a cosmetic wrong-ARN answer: it destroys state the caller never addressed, and the pair the caller DID name stays registered.

This contradicts how the same crate treats the other secondary discriminators of an ARN-addressed teardown:
* `resolve_product_user` cross-checks `Domain` on its ARN path, with the stated reason that "a subscription addressed by ARN is also only the one the caller asked for when the domain the request names is the stored user's" — which applies verbatim to `Product`, half of a registration's identity (`identity_provider_index` is keyed on `(provider key, Product)`), and which this function's own non-ARN path requires to match;
* `DeleteLicenseServerEndpoint` explicitly refuses an ARN whose stored `ServerType` is not the one the request named.

The same unchecked ARN branch is reached by `UpdateIdentityProviderSettings`, so the fix belongs in `resolve_identity_provider` rather than in this one handler.

## Evidence

The handler validates `Product` and then uses it for nothing else — `src/ops/deregister_identity_provider.rs:167-184`:
```
        // Whether a product name is one this service serves does not depend on what is
        // registered, so it is checked before the store is read at all.
        if let Some(product) = input.product.as_deref() {
            crate::state::validate_product(product).map_err(into_error)?;
        }

        let mut state = state.lock();
        state.materialise_due(now);

        let arn = state
            .resolve_identity_provider(
                input.identity_provider_arn.as_deref(),
                input.identity_provider.as_ref(),
                input.product.as_deref(),
            )
            .map_err(into_error)?
            .arn
            .clone();
```

The ARN branch compares nothing — `src/state.rs:1876-1899`:
```
    pub fn resolve_identity_provider(
        &self,
        arn: Option<&str>,
        identity_provider: Option<&IdentityProvider>,
        product: Option<&str>,
    ) -> Result<&IdentityProviderRecord, StateError> {
        if let Some(arn) = arn {
            return self
                .live_identity_provider(arn)
                .ok_or(StateError::ProviderNotRegistered);
        }
...
        let arn = self
            .identity_provider_index
            .get(&(key, product.to_string()))
            .ok_or(StateError::ProviderNotRegistered)?;
```
so the pair path REQUIRES the product to match while the ARN path ignores it: one resolution answers two ways depending on how the registration is addressed.

The sibling resolution in the same file DOES cross-check its extra identity member on the ARN path — `src/state.rs:2029-2039`:
```
        // `Domain` is part of which user a request names, so a subscription addressed by
        // ARN is also only the one the caller asked for when the domain the request names
        // is the stored user's. The identity lookup above already matched on it.
        let named = domain_key(domain);
        if named.is_some() && named != domain_key(record.domain.as_deref()) {
            return Err(StateError::NotFound(no_such_subscription(
```

The sibling ARN-addressed teardown refuses the mismatch — `src/ops/delete_license_server_endpoint.rs:173-180`:
```
        // Fail closed: an ARN state holds no endpoint for, or one whose `ServerType` is
        // not the stored endpoint's, is refused without disclosing which it was.
        let arn = state
            .license_server_endpoints
            .get(&input.license_server_endpoint_arn)
            .filter(|record| record.server_type == input.server_type)
```

What the accepted call destroys — `src/state.rs:2089-2104`:
```
        self.remove_product_users_of(arn);
        let orphaned: Vec<String> = self
            .license_server_endpoints
            .values()
            .filter(|endpoint| endpoint.identity_provider_arn == arn)
            .map(|endpoint| endpoint.arn.clone())
            .collect();
        for endpoint_arn in orphaned {
            self.begin_license_server_endpoint_deletion(&endpoint_arn, now);
        }
...
        if let Ok(key) = provider_key(&deregistering.identity_provider) {
            self.drop_instance_users_orphaned_by(&key, now);
        }
```

Test coverage: the deregistration suite only ever sends ONE of the two addressings, never an ARN plus a conflicting `Product` — `tests/all/identity_provider_deregistration.rs:180-188`:
```
    fn deregistration(&self, arn: &str) -> Value {
        match self.addressed_by {
            AddressedBy::Arn => json!({ "IdentityProviderArn": arn }),
            AddressedBy::ProviderAndProduct => json!({
                "IdentityProvider": self.provider(),
                "Product": self.product,
            }),
        }
    }
```
and `tests/all/registration_pair_resolution_is_shared.rs:206` likewise deregisters with `json!({ "IdentityProviderArn": arn })` alone, so nothing pins the mixed request.

## Steps to Reproduce

1. `RegisterIdentityProvider` for directory `d-0123456789` with `Product: VISUAL_STUDIO_ENTERPRISE`; note the returned `IdentityProviderArn` (call it ARN-VSE).
2. `RegisterIdentityProvider` for the SAME directory with `Product: OFFICE_STANDARD`; note its ARN (ARN-OS).
3. `StartProductSubscription` for user `u1` through that directory with `Product: VISUAL_STUDIO_ENTERPRISE`, and `CreateLicenseServerEndpoint` for ARN-VSE, so the VSE registration has dependants.
4. Call `DeregisterIdentityProvider` with `{"IdentityProviderArn": "<ARN-VSE>", "Product": "OFFICE_STANDARD"}`.
5. Observe HTTP 200 and `IdentityProviderSummary.Product == "VISUAL_STUDIO_ENTERPRISE"`, `Status == "DEREGISTERING"`: the registration for a product the request did not name was torn down, where the same kind of mismatch is refused on `Domain` (`src/state.rs:2029-2039`) and on `ServerType` (`src/ops/delete_license_server_endpoint.rs:173-180`).
6. `ListProductSubscriptions` for the directory with `Product: VISUAL_STUDIO_ENTERPRISE` and `ListLicenseServerEndpoints`: the VSE subscription is gone and its endpoint is `DELETING`, while `ListIdentityProviders` still reports ARN-OS as live — the pair the caller named was never deregistered.
