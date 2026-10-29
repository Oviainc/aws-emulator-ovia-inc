Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: cc96b89474242e07623dda2d895b89486f0507c0

# AssociationSize counts every association record the item holds, not the ones in effect in the billing period the read is scoped to

_Filed: 2026-10-29T14:06:33Z_
_Operation: ListCustomLineItems | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `cc96b89474242e07623dda2d895b89486f0507c0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every period-scoped read of a custom line item (`ListCustomLineItems`, `ListCustomLineItemVersions`, `UpdateCustomLineItem`) reports `AssociationSize` from `CustomLineItemRecord::association_size()`, which is `self.associated_resources.len()` — the raw length of the association list, with no reference to the billing periods each association is in effect over.

The sibling period-scoped read of the SAME associations, `ListResourcesAssociatedToCustomLineItem`, asks each association record whether it covers the period before reporting it. So for one and the same billing period the two reads disagree: `ListCustomLineItems` answers `AssociationSize: 1` while `ListResourcesAssociatedToCustomLineItem` answers an empty `AssociatedResources` list for that period.

This crate states the rule it is breaking in state.rs itself: a resource the item holds a record for "can be in effect in some periods and not in others, so a caller deciding anything period-scoped asks the record". `association_size()` is precisely such a period-scoped decision — it is fed into `current_version()`, the projection `effective_in(billing_period)` hands a period-scoped read — and it never asks `ResourceAssociationRecord::covers`.

Two stored shapes reach this state with associations that are not in effect in the period being read, both through paths the crate already serves:
- `BatchAssociateResourcesToCustomLineItem` with a `BillingPeriodRange` whose `InclusiveStartBillingPeriod` is a LATER period than the one a read asks about (that handler applies only the ordered-range check, not the modifiable-window check), and
- `BatchDisassociateResourcesFromCustomLineItem` with a bounded range, which narrows the association's spans but keeps the record whenever the narrowed association is non-empty (`apply_disassociation` removes it only `if association.is_empty()`).

The same wrong count is frozen into the version history: `CustomLineItemVersion.association_size` is filled from the same unscoped length, so `ListCustomLineItemVersions` reports it too, and `UpdateCustomLineItem`'s `render` returns `item.association_size()` directly.

## Evidence

The unscoped count — `src/state.rs:1868-1872`:
```
    /// `AssociationSize`: "The number of resources that are associated to the custom
    /// line item."
    pub fn association_size(&self) -> i64 {
        i64::try_from(self.associated_resources.len()).unwrap_or(i64::MAX)
    }
```
fed straight into the PERIOD-SCOPED projection — `src/state.rs:1973` inside `CustomLineItemRecord::current_version()`:
```
            association_size: self.association_size(),
```
whose own field doc claims it is per-generation — `src/state.rs:2055-2056`:
```
    /// `AssociationSize`: the resources associated to the item in this generation.
    pub association_size: i64,
```

The rule this crate states for exactly this decision — `src/state.rs:2004-2007`:
```
    /// Whether the resource is associated IN a billing period is the record's own
    /// question ([`ResourceAssociationRecord::covers`]) and never this lookup's: a
    /// resource the item holds a record for can be in effect in some periods and not in
    /// others, so a caller deciding anything period-scoped asks the record.
```

The sibling listing that DOES apply it — `src/ops/list_resources_associated_to_custom_line_item.rs:212-217`:
```
        for association in &item.associated_resources {
            // An association is listed for a period its range covers, the start
            // inclusive and the end exclusive.
            if !association.covers(&billing_period) {
                continue;
            }
```

The handler that reports the unscoped value under a period-scoped contract — `src/ops/list_custom_line_items.rs:145-146`:
```
/// The values compared are the ones of the version in effect in the billing period the
/// read is scoped to, because they are the values this read reports.
```
and `src/ops/list_custom_line_items.rs:197-199` / `:218`:
```
/// `ChargeDetails`, `AssociationSize` and `CurrencyCode` are DERIVED from the record
/// rather than stored beside it, so none of them can drift from the item.
...
        association_size: version.association_size,
```

The bounded disassociation that keeps the record while narrowing its spans — `src/ops/batch_disassociate_resources_from_custom_line_item.rs:201-205`:
```
    let outcome = association.remove_periods(removed);
    if association.is_empty() {
        associations.remove(index);
    }
    outcome
```

Model text for the member (`smithy_query(query="doc",shape="CustomLineItemListElement",member="AssociationSize")`): "The number of resources that are associated to the custom line item."

Test coverage: `tests/all/custom_line_item_associations_accumulate.rs`, `tests/all/percentage_custom_line_item_resource_associations.rs` and `tests/all/resource_association_billing_periods.rs` exercise associations, but every `AssociationSize` assertion is made in a billing period in which all of the item's associations are in effect, so no test pins the unscoped count for a period that excludes one.

## Steps to Reproduce

1. Designate the emulated account as the payer so the control plane is reachable (set `ComAmazonawsBillingconductorAwsbillingconductorState::payer_account_id`).
2. POST /create-billing-group and POST /create-pricing-plan to get a billing group G.
3. POST /create-custom-line-item against G with ChargeDetails {"Type":"FEE","Percentage":{"PercentageValue":10}} — a percentage item P, AssociationSize 0.
4. POST /create-custom-line-item against G with ChargeDetails {"Type":"FEE","Flat":{"ChargeValue":100}} — a child item C in the same group.
5. PUT /batch-associate-resources-to-custom-line-item with {"TargetArn":"<P>","ResourceArns":["<C>"],"BillingPeriodRange":{"InclusiveStartBillingPeriod":"<next month>"}} — accepted (that handler applies only the ordered-range check).
6. POST /list-resources-associated-to-custom-line-item with {"Arn":"<P>"} (no BillingPeriod, so the CURRENT period) and observe an empty AssociatedResources: the association is not in effect this period.
7. POST /list-custom-line-items with no BillingPeriod and observe P reported with AssociationSize 1 for that same current period — the two period-scoped reads of one association disagree.
8. Equivalent second path: associate C with no range, then PUT /batch-disassociate-resources-from-custom-line-item with {"TargetArn":"<P>","ResourceArns":["<C>"],"BillingPeriodRange":{"InclusiveStartBillingPeriod":"<current month>","ExclusiveEndBillingPeriod":"<next month>"}} and repeat steps 6-7.
