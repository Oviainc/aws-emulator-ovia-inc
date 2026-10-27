Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 53db56bf3158c7205a85c81c618ec0b3c4193a64

# Fold remove_product_users_of onto the existing batched drop_product_users writer

_Filed: 2026-10-27T05:02:01Z_
_Operation: DeregisterIdentityProvider | Class: maintainability_

> _Note: this issue was identified against the code base at commit `53db56bf3158c7205a85c81c618ec0b3c4193a64`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state` implements the "drop product users from the store, the subscription order and the triple index" rule TWICE, with the same batching justification written out in both doc comments. `remove_product_users_of` (reached from `DeregisterIdentityProvider` via `begin_identity_provider_deregistration`, and from the clock sweep via `drop_identity_provider`) selects the subscriptions of one registration and then hand-rolls exactly the four mutations `drop_product_users` already performs: remove from `product_users`, remove the ARN's `tags`, `retain` the `product_user_order`, `retain` the `product_user_index`. `drop_product_users` is the writer the other two callers (`remove_product_user` and the `materialise_due` sweep) already go through. Two copies of one store-mutation rule can drift: an index or side table added to one drop path and not to the other would leave the store internally inconsistent for whichever path was missed, and the store's own `insert_product_user` doc calls itself "The ONE writer of a product user, so no path can leave the three stores disagreeing about what is subscribed" — the removal side has two. The fix is purely internal: `remove_product_users_of` collects the orphaned ARNs (as it already does) and calls `self.drop_product_users(&orphaned)`, leaving every response and every existing assertion unchanged.

## Evidence

Copy 1 — services/service-com-amazonaws-licensemanagerusersubscriptions-licensemanagerusersubscriptions/src/state.rs:2046-2066:
```
/// Remove every product user subscribed through the registration `provider_arn`
/// names, in ONE pass over each list that reaches them: a registration with many
/// subscribers must not cost a scan of the order list and of the index per subscriber.
fn remove_product_users_of(&mut self, provider_arn: &str) {
    let orphaned: HashSet<String> = self.product_users.values()
        .filter(|user| user.identity_provider_arn == provider_arn)
        .map(|user| user.arn.clone()).collect();
    if orphaned.is_empty() { return; }
    for arn in &orphaned { self.product_users.remove(arn); self.tags.remove(arn); }
    self.product_user_order.retain(|arn| !orphaned.contains(arn));
    self.product_user_index.retain(|_, indexed| !orphaned.contains(indexed));
}
```

Copy 2 — same file, state.rs:2082-2103:
```
/// Drop the product users `arns` names from the store and from every index that reaches
/// them.
///
/// Batched on purpose: the subscription ORDER and the triple index are each rebuilt in
/// ONE pass however many subscriptions are dropped together, ...
fn drop_product_users(&mut self, arns: &[String]) {
    let mut dropped: HashSet<String> = HashSet::new();
    for arn in arns { if self.product_users.remove(arn).is_some() { self.tags.remove(arn); dropped.insert(arn.clone()); } }
    if dropped.is_empty() { return; }
    self.product_user_order.retain(|arn| !dropped.contains(arn));
    self.product_user_index.retain(|_, indexed| !dropped.contains(indexed));
}
```

The single-writer rule the removal side breaks — state.rs:1773-1777: "Store one product user: the record, its place in the subscription order and the `(IdentityProviderArn, product, username)` index that reaches it. / The ONE writer of a product user, so no path can leave the three stores disagreeing about what is subscribed."

Call sites that keep the two copies alive: state.rs:1976 `self.remove_product_users_of(arn);` (inside `begin_identity_provider_deregistration`, the helper `DeregisterIdentityProvider`'s handler calls at src/ops/deregister_identity_provider.rs:189-191), state.rs:2041 `self.remove_product_users_of(arn);` (inside `drop_identity_provider`), versus state.rs:958 `self.drop_product_users(&unsubscribed);` and state.rs:2078.

Test coverage that pins the behaviour both copies must keep producing: tests/all/identity_provider_subscriptions.rs:1375-1379 (`guard.product_users.is_empty() && guard.product_user_index.is_empty(), "no product user may outlive its identity provider"`, asserted right after a deregister, i.e. through `remove_product_users_of`) and again at 1397-1400 after the clock-owed completion; tests/all/product_subscription_teardown.rs:306-315 asserts `guard.product_user_order.is_empty() && guard.product_user_index.is_empty()` for the `drop_product_users` path.

## Steps to Reproduce

1. Open services/service-com-amazonaws-licensemanagerusersubscriptions-licensemanagerusersubscriptions/src/state.rs and read `remove_product_users_of` (lines 2046-2066) and `drop_product_users` (lines 2082-2103).
2. Observe both perform the identical four mutations (store removal, tag removal, `product_user_order.retain`, `product_user_index.retain`) and both doc comments justify the same single-pass batching.
3. Note the two call sites of `remove_product_users_of` (state.rs:1976 and state.rs:2041) never reach `drop_product_users`, so any future index added to one drop path is silently missed by the other.
4. Fold the former onto the latter (collect the orphaned ARNs, call `self.drop_product_users(&orphaned)`) and re-run `tests/all/identity_provider_subscriptions.rs` and `tests/all/product_subscription_teardown.rs`: both suites must pass unchanged.
