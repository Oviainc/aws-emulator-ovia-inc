Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 2d45ab8ecba5c3bbeb0b6029f45083afacab7d74

# Fold the "an uncancelled order of this Outpost × its line items' published catalog capacity × quantity" rule into one place instead of three copies that have already drifted

_Filed: 2026-10-29T11:04:01Z_
_Operation: GetRenewalPricing | Class: maintainability_

> _Note: this issue was identified against the code base at commit `2d45ab8ecba5c3bbeb0b6029f45083afacab7d74`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

One rule — "walk the orders placed on this Outpost that are not CANCELLED, and for each line item multiply the capacity the published catalog carries for its CatalogItemId by the item's Quantity" — is written out three times in this crate's own `state.rs`:

* `ordered_instance_type_names` (state.rs:3340) — the order-derived half of what `GetOutpostInstanceTypes` reports;
* `subscribed_capacity_vcpus` (state.rs:3380) — what `GetRenewalPricing` / `CreateRenewal` price a renewal off, through `renewal_price` (state.rs:3746);
* `provisioned_s3_capacity` (state.rs:3417) — the same walk again, summing the S3 figure instead of the vCPU one.

The crate's own doc comments assert the three are ONE rule over the SAME records, so nothing here is three different rules that happen to look alike. Yet the copies have ALREADY drifted on how they add up: `subscribed_capacity_vcpus` multiplies and sums with plain arithmetic (`i64::from(capacity) * quantity`, `.sum()`), while `provisioned_s3_capacity` uses `saturating_mul` / `saturating_add` and additionally skips any line item whose published capacity is not positive — two different answers to the same question about overflow and about a zero-capacity configuration, in two copies of one walk.

The fix is to state the walk once (an iterator over "the uncancelled orders on this Outpost" plus one per-line-item capacity fold the caller parameterises with the catalog figure it wants), and have the three callers use it. Every response and every existing test stays byte-for-byte the same: the inputs the suite exercises are all well inside i64 and none of the published configurations carries a non-positive capacity, so the two arithmetic spellings agree on everything currently covered — which is exactly why the drift is invisible today and will bite the next person who changes the rule in one copy.

## Evidence

The crate declares the three read the SAME records and follow the SAME rule.

services/service-com-amazonaws-outposts-outpostsolafservice/src/state.rs:3230-3236 (doc of `configured_instance_types`, the caller of copy 1):
```
    /// * the ORDER its hardware arrived on, until a capacity task reconfigures that
    ///   hardware. A published configuration carries EC2 capacity of one or more
    ///   instance families, each at the largest size that hardware runs
    ///   ([`ec2_capacities`]), so an Outpost is configured with those sizes of the
    ///   configurations its uncancelled orders subscribe to — the same records the
    ///   subscribed capacity is read off ([`Self::subscribed_capacity_vcpus`]).
```

src/state.rs:3405-3412 (doc of copy 3, pointing at copy 2 for its own rule):
```
    /// Storage capacity is installed on an Outpost by ordering hardware that carries
    /// it, so what is provisioned is what the orders on it bought, and an Outpost
    /// nobody ordered S3-capable hardware for is absent rather than present with
    /// zero: it has no S3 storage on it at all. A line item naming a catalog item
    /// this service publishes no configuration for contributes nothing, for the
    /// reason it contributes no vCPUs to [`Self::subscribed_capacity_vcpus`]: the
    /// capacity of hardware the catalog does not describe is not a figure the
    /// emulator has, and never one it invents.
```

Copy 1 — src/state.rs:3340-3347:
```
    fn ordered_instance_type_names(&self, region: &str, outpost_id: &str) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        let mut orders: Vec<&OrderRecord> = all_in_region(&self.orders, region)
            .filter(|record| {
                record.order.outpost_id.as_deref() == Some(outpost_id)
                    && record.order.status.as_deref() != Some(ORDER_STATUS_CANCELLED)
            })
            .collect();
```

Copy 2 — src/state.rs:3380-3396:
```
    pub fn subscribed_capacity_vcpus(&self, region: &str, outpost_id: &str) -> i64 {
        all_in_region(&self.orders, region)
            .filter(|record| {
                record.order.outpost_id.as_deref() == Some(outpost_id)
                    && record.order.status.as_deref() != Some(ORDER_STATUS_CANCELLED)
            })
            .flat_map(|record| record.order.line_items.iter().flatten())
            .filter_map(|item| {
                let capacity =
                    self.catalog_item_capacity_vcpus(item.catalog_item_id.as_deref()?)?;
                // `LineItem$Quantity` is modelled `range(min: 1)`, so a quantity the
                // boundary accepted is at least one unit of that configuration.
                let quantity = i64::from(item.quantity.unwrap_or(1).max(0));
                Some(i64::from(capacity) * quantity)
            })
            .sum()
    }
```

Copy 3 — src/state.rs:3417-3447 (same walk, different arithmetic and an extra skip):
```
    pub fn provisioned_s3_capacity(&self) -> BTreeMap<String, BTreeMap<String, i64>> {
        let mut provisioned: BTreeMap<String, BTreeMap<String, i64>> = BTreeMap::new();
        for record in self.orders.values() {
            if record.order.status.as_deref() == Some(ORDER_STATUS_CANCELLED) {
                continue;
            }
            ...
            for item in record.order.line_items.iter().flatten() {
                let capacity = item
                    .catalog_item_id
                    .as_deref()
                    .and_then(|sku| self.catalog_item_s3_capacity_bytes(sku))
                    .unwrap_or_default();
                if capacity <= 0 {
                    continue;
                }
                // `LineItem$Quantity` is modelled `range(min: 1)`, so a quantity the
                // boundary accepted is at least one unit of that configuration. The
                // floor is belt and braces against a value the boundary cannot deliver:
                // a negative quantity would otherwise subtract capacity the order never
                // cancelled.
                let quantity = i64::from(item.quantity.unwrap_or(1).max(0));
                let total = provisioned
                    .entry(record.region.clone())
                    .or_default()
                    .entry(outpost_id.to_string())
                    .or_default();
                *total = total.saturating_add(capacity.saturating_mul(quantity));
            }
        }
        provisioned
    }
```

The drift is in the quoted text: copy 2 is `i64::from(capacity) * quantity` + `.sum()`, copy 3 is `capacity.saturating_mul(quantity)` + `saturating_add`, and only copy 3 drops a line item whose published capacity is `<= 0`.

The entry point that reaches copy 2 — src/ops/get_renewal_pricing.rs:201:
```
        let options = store.renewal_pricing(&region, &outpost_id);
```
via src/state.rs:3746-3758:
```
    pub fn renewal_price(
        &self,
        region: &str,
        outpost_id: &str,
        payment_option: &str,
        payment_term: &str,
    ) -> Option<SubscriptionPricingDetails> {
        subscription_price(
            self.subscribed_capacity_vcpus(region, outpost_id) as f32,
```

Test coverage that must stay unchanged: tests/all/renewal_pricing.rs and tests/all/renewal_contract.rs exercise copy 2 (`GetRenewalPricing` / `CreateRenewal`); tests/all/configured_instance_types.rs exercises copy 1 (`GetOutpostInstanceTypes`, including the deliberately-accepted unpublished SKU row at :481-487); copy 3 is exercised through the sibling crate that consumes it, services/service-com-amazonaws-s3outposts-s3outposts/src/state.rs:746 (`control_plane.provisioned_s3_capacity()`).

## Steps to Reproduce

1. Open services/service-com-amazonaws-outposts-outpostsolafservice/src/state.rs and read `ordered_instance_type_names` (line 3340), `subscribed_capacity_vcpus` (line 3380) and `provisioned_s3_capacity` (line 3417).
2. Observe the same predicate spelled three times: an order whose `order.outpost_id` is this Outpost and whose `order.status` is not `ORDER_STATUS_CANCELLED`.
3. Observe the per-line-item fold spelled twice (lines 3386-3394 and 3426-3447): resolve `catalog_item_id` against the published catalog, multiply by `quantity.unwrap_or(1).max(0)`, total it.
4. Compare the two folds: line 3393 multiplies and sums with plain arithmetic, line 3446 uses `saturating_mul`/`saturating_add`, and line 3432 additionally skips a line item whose published capacity is not positive — one rule, two answers.
5. Read the doc comments at state.rs:3230-3236 and state.rs:3405-3412, which both assert the copies read the same records and follow the same rule, so the divergence above is drift and not design.
6. Fix: state the walk and the fold once in `state.rs` and route all three callers through it, leaving every response and every existing test unchanged.
