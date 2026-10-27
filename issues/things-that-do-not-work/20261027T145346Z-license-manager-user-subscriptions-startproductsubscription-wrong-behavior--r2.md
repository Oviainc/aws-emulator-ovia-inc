Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 83df1cdc32a5bdf510e34eb14380f1c3f1e573c9

# StartProductSubscription conflicts on (provider, product, username) while StopProductSubscription treats Domain as part of the identity

_Filed: 2026-10-27T14:53:46Z_
_Operation: StartProductSubscription | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `83df1cdc32a5bdf510e34eb14380f1c3f1e573c9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`StartProductSubscription` refuses a subscription whenever the `(IdentityProviderArn, Product, Username)` triple is occupied, without comparing the request's `Domain` to the stored subscription's. The state layer's `resolve_product_user`, which `StopProductSubscription` uses, narrows that same triple by `Domain` and reports "no subscription in domain …" when it differs.

So for the same username in two Active Directory domains the service answers two contradictory things about one identity: `StartProductSubscription{alice, P, Domain: "b.example"}` is refused with `ConflictException` ("user alice already has a subscription to product P") because alice@a.example is subscribed, while `StopProductSubscription{alice, P, Domain: "b.example"}` answers `ResourceNotFoundException` ("user alice has no subscription in domain b.example"). A user that the stop path says has no subscription cannot be subscribed either, so that user is unreachable: `Domain` either distinguishes a product user or it does not, and the two paths must not disagree.

## Evidence

src/ops/start_product_subscription.rs:193-211 — the occupancy check drops `input.domain`:
```
        let index_key = (
            provider_arn.clone(),
            input.product.clone(),
            input.username.clone(),
        );
        // The triple is occupied by whatever subscription state holds for it, including
        // one still being torn down after a stop: the user's subscription to this product
        // can be created again only once that teardown has completed.
        if let Some(existing) = state.product_user_index.get(&index_key) {
            return Err(StartProductSubscriptionErrorModeled::ConflictException(
                ConflictException {
                    message: Some(format!(
                        "user {} already has a subscription to product {} as {existing}",
                        input.username, input.product
                    )),
```
The index it consults carries no domain either — src/state.rs:1779-1783 (`insert_product_user`):
```
        let key = (
            record.identity_provider_arn.clone(),
            record.product.clone(),
            record.username.clone(),
        );
```
But the resolution the stop path uses says a differing domain is a DIFFERENT subscription — src/state.rs:1917-1926 (`resolve_product_user`):
```
        // `Domain` narrows the addressing rather than being ignored: a subscription from
        // another domain is not the one the caller named.
        if let Some(domain) = domain.filter(|d| !d.is_empty()) {
            if record.domain.as_deref() != Some(domain) {
                return Err(StateError::NotFound(format!(
                    "user {} has no subscription in domain {domain}",
                    record.username
                )));
```
reached from src/ops/stop_product_subscription.rs:172-180, which passes `input.domain` straight into it.

Test coverage: the conflict test replays the IDENTICAL request body (tests/all/identity_provider_subscriptions.rs:811-826 — `let request = subscription_request(...); start_subscription(&h, &request).await.ok(row.case); let again = start_subscription(&h, &request).await;`), and the SUBSCRIBERS rows never repeat one username with two domains, so the differing-domain case is unpinned.

## Steps to Reproduce

1. RegisterIdentityProvider for a seeded directory and Product=VISUAL_STUDIO_ENTERPRISE.
2. StartProductSubscription with Username=alice, that IdentityProvider, that Product and Domain="a.example" — 200, SUBSCRIBED.
3. StartProductSubscription with Username=alice, the same IdentityProvider/Product and Domain="b.example" — observe ConflictException (HTTP 500) claiming alice already has a subscription.
4. StopProductSubscription with Username=alice, the same IdentityProvider/Product and Domain="b.example" — observe ResourceNotFoundException saying alice has no subscription in domain b.example, contradicting step 3.
