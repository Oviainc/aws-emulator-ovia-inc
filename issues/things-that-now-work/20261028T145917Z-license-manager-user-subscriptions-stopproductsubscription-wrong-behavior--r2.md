Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 436fee4160e0c164196671a7c5bd1a9f84eddc25

# StopProductSubscription ignores Product when the request also carries ProductUserArn, stopping another product's subscription

_Filed: 2026-10-28T14:59:17Z_
_Operation: StopProductSubscription | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `436fee4160e0c164196671a7c5bd1a9f84eddc25`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

When a request supplies `ProductUserArn`, `crate::state::resolve_product_user` returns the record that ARN names and then cross-checks only `Domain`. `Product` is never compared against the stored subscription, even though it was supplied and validated against `crate::state::PRODUCTS` a few lines earlier. So a request naming `ProductUserArn` of a `VISUAL_STUDIO_ENTERPRISE` subscription together with `Product: OFFICE_STANDARD` is accepted: the handler ends the VISUAL_STUDIO_ENTERPRISE subscription and answers HTTP 200 with a `ProductUserSummary` whose `Product` is the stored one, not the one the caller named. A request that names two mutually contradictory things is turned into a success against a resource the caller did not name.

This contradicts the way the same function treats `Domain` — the reason given there ("`Domain` is part of which user a request names, so a subscription addressed by ARN is also only the one the caller asked for when the domain the request names is the stored user's") applies verbatim to `Product`, which is half of a product user's identity (`product_user_key(provider, product, username, domain)`) and which the non-ARN addressing requires to match. It also contradicts the sibling ARN-addressed teardown in this crate, `DeleteLicenseServerEndpoint`, which explicitly refuses a request whose secondary discriminator does not match the stored record.

## Evidence

The ARN path checks Domain and not Product — `src/state.rs:2004-2040`:
```
        let record = if let Some(arn) = product_user_arn {
            self.live_product_user(arn).ok_or_else(|| {
                StateError::NotFound(format!("product user {arn} has no subscription"))
            })?
        } else {
...
        // `Domain` is part of which user a request names, so a subscription addressed by
        // ARN is also only the one the caller asked for when the domain the request names
        // is the stored user's. The identity lookup above already matched on it.
        let named = domain_key(domain);
        if named.is_some() && named != domain_key(record.domain.as_deref()) {
            return Err(StateError::NotFound(no_such_subscription(
                &record.username,
                &record.product,
                domain,
            )));
        }
        Ok(record)
```
The non-ARN path in the same function DOES require the product to match (`src/state.rs:2018-2024`): `self.resolve_identity_provider(None, Some(identity_provider), Some(product))?` then `self.product_user_arn_for(&provider.arn, product, username, domain)` — so one resolution answers two ways depending on how the subscription is addressed.

The sibling ARN-addressed teardown refuses the mismatch — `src/ops/delete_license_server_endpoint.rs:173-180`:
```
        // Fail closed: an ARN state holds no endpoint for, or one whose `ServerType` is
        // not the stored endpoint's, is refused without disclosing which it was.
        let arn = state
            .license_server_endpoints
            .get(&input.license_server_endpoint_arn)
            .filter(|record| record.server_type == input.server_type)
```

The handler supplies `Product` to the resolution and uses it for nothing else — `src/ops/stop_product_subscription.rs:169-184`:
```
        if let Some(product) = input.product.as_deref() {
            crate::state::validate_product(product).map_err(into_error)?;
        }
...
            .resolve_product_user(
                input.product_user_arn.as_deref(),
                input.username.as_deref(),
                input.identity_provider.as_ref(),
                input.product.as_deref(),
                input.domain.as_deref(),
            )
```

Test coverage: every ARN-addressed stop in the suite sends the ARN alone — `tests/all/product_subscription_teardown.rs:134` and `tests/all/product_user_removal_is_one_writer.rs:235` both build `json!({ "ProductUserArn": arn })` — so no test pins the behaviour for an ARN plus a conflicting `Product`.

## Steps to Reproduce

1. Register an identity provider for `VISUAL_STUDIO_ENTERPRISE` and for `OFFICE_STANDARD` (same directory, two products).
2. `StartProductSubscription` for user `u1` with `Product: VISUAL_STUDIO_ENTERPRISE`; note the returned `ProductUserArn`.
3. Call `StopProductSubscription` with `{"ProductUserArn": <that arn>, "Product": "OFFICE_STANDARD"}`.
4. Observe HTTP 200 and `ProductUserSummary.Product == "VISUAL_STUDIO_ENTERPRISE"` with `Status == "UNSUBSCRIBED"`: the subscription for a product the request did not name was ended, where the same mismatch on `Domain` (and the same mismatch on `ServerType` in `DeleteLicenseServerEndpoint`) is refused.
5. `ListProductSubscriptions` for the provider confirms the VISUAL_STUDIO_ENTERPRISE subscription is the one torn down.
