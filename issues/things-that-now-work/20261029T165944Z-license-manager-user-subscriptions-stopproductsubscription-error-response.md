Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 097d82b3cbc72c7bfc910d8eaa03c5f730d6b648

# Second StopProductSubscription during the teardown window answers a second wording and leaks the internal ProductUserArn

_Filed: 2026-10-29T16:59:44Z_
_Operation: StopProductSubscription | Class: error-response_

> _Note: this issue was identified against the code base at commit `097d82b3cbc72c7bfc910d8eaa03c5f730d6b648`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate declares ONE sentence for "the user this request names has no subscription in state" (`crate::state::no_such_subscription`), and `StopProductSubscription` is documented to answer it for a second stop. But in `resolve_product_user`'s member-by-member branch there are TWO refusals, and the second one is reachable for exactly the request the first one is meant to word.

`begin_product_user_unsubscription` keeps the `product_user_key` index entry while the ended subscription is torn down, so during that window `product_user_arn_for(..)` still resolves the identity `(Username, IdentityProvider, Product, Domain)` to the stored ARN, and the refusal is produced by the following `live_product_user(arn)` miss instead — which formats `"product user {arn} has no subscription"`, an internally minted `ProductUserArn` the caller never named. Once the teardown the clock owes has completed the index entry is gone, `product_user_arn_for` misses, and the very same request is refused with the shared sentence `"user <name> has no subscription to product <product>"`.

So one logical refusal arrives in two wordings depending only on whether ~500 ms have passed, and the timing-dependent one discloses a resource identifier the request did not supply. The status and error code are the same (404 `ResourceNotFoundException`) in both cases, so only the sentence diverges from the rule the crate states for itself.

## Evidence

The rule the crate states for this refusal — `services/.../src/state.rs:457-461`:

    /// The sentence a `StopProductSubscription` is refused with when the user it names has no
    /// subscription in state: the one wording of that refusal, whether the identity the
    /// request named matched nothing at all or named a domain the stored subscription's user
    /// did not come from.
    pub fn no_such_subscription(username: &str, product: &str, domain: Option<&str>) -> String {

The code under it, in the member-by-member branch of `resolve_product_user` — `services/.../src/state.rs:2286-2290`:

        let arn = self
            .product_user_arn_for(&provider.arn, product, username, domain)
            .ok_or_else(|| StateError::NotFound(no_such_subscription(username, product, domain)))?;
        self.live_product_user(arn)
            .ok_or_else(|| StateError::NotFound(format!("product user {arn} has no subscription")))

Why the second arm is reachable for a plain second stop — `services/.../src/state.rs:2476-2479` (`begin_product_user_unsubscription`):

    /// [`product_user_key`] index entry is KEPT, because that identity
    /// is still occupied: the user's subscription to that product is still being torn down,
    /// so it cannot be subscribed again until the teardown has completed.

and `live_product_user` excludes exactly such a record — `services/.../src/state.rs:1190-1194`:

    pub fn live_product_user(&self, arn: &str) -> Option<&ProductUserRecord> {
        self.product_users
            .get(arn)
            .filter(|record| !unsubscribe_started(&record.status))

The handler routes both through the same modelled error, so only the sentence differs — `services/.../src/ops/stop_product_subscription.rs:176-184` and `:226-233`.

Test coverage: `tests/all/product_subscription_teardown.rs:462-479` drives the second stop through BOTH addressings inside the teardown window but asserts only `again.status == 404` and `error_type == Some("ResourceNotFoundException")` — the message is not checked, which is why the divergence is not caught. The shared wording itself IS pinned, for the ARN-cross-check case, in `tests/all/arn_addressing_cross_checks_domain.rs:203-220` ("spelled out here rather than read from the production helper so a change to it fails a test").

## Steps to Reproduce

1. RegisterIdentityProvider for a seeded directory (e.g. DirectoryId d-9f8e7d6c5b) and a product, e.g. VISUAL_STUDIO_ENTERPRISE.
2. StartProductSubscription with Username=alice, that IdentityProvider and Product, no Domain — 200, note the minted ProductUserArn.
3. StopProductSubscription addressed by members (Username=alice, IdentityProvider, Product) — 200, Status UNSUBSCRIBED.
4. Immediately (inside the ~500 ms UNSUBSCRIPTION_SPAN) repeat the same member-addressed StopProductSubscription: the 404 body reads "product user arn:aws:license-manager-user-subscriptions:...:user/pu-000000000001 has no subscription" — an ARN the request never named.
5. Wait past the owed teardown and repeat the identical request: the 404 body now reads "user alice has no subscription to product VISUAL_STUDIO_ENTERPRISE", the sentence state.rs:457-461 declares as the one wording of this refusal.
