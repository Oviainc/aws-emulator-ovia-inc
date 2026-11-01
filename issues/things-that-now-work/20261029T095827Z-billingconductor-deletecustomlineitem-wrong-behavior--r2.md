Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: d1b341c944550181b7ac630538402367c666ba11

# A bounded DeleteCustomLineItem narrows the item out of the named billing periods but leaves every resource association in effect there, unlike the full delete

_Filed: 2026-10-29T09:58:27Z_
_Operation: DeleteCustomLineItem | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d1b341c944550181b7ac630538402367c666ba11`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteCustomLineItem` has two outcomes. With no `BillingPeriodRange` it removes the record through `state.remove_custom_line_item(..)`, which also drops the association records other percentage items held TO the item. With a `BillingPeriodRange` it instead calls `item.remove_periods(&removed)`, which takes those billing periods out of EVERY generation of the item (so no read of one of those periods reports a version of it) — and touches no association at all:

* the associations the item itself holds keep covering the deleted-from periods, and
* the associations OTHER percentage items hold to this item keep covering them too.

So after a bounded delete the store holds associations that are in effect in billing periods in which the custom line item applies in no generation. The symptom is directly observable: `ListResourcesAssociatedToCustomLineItem` on the parent item, scoped to one of those periods, reports the narrowed item as an associated resource (and `AssociationSize` keeps counting it), while `ListCustomLineItems` scoped to the SAME period reports no such item at all — "an item no version of which applies in that period is not reported at all" (src/ops/list_custom_line_items.rs:288-293).

The crate already states the opposite rule for the full delete and already owns the helper the bounded path needs: `ResourceAssociationRecord::remove_periods` (src/state.rs:1672-1679), which `BatchDisassociateResourcesFromCustomLineItem` uses to end an association over a range and to drop the record once it is in effect nowhere. The bounded delete is the one write path in this crate that narrows a resource's billing periods without pushing that narrowing through the association records keyed on it.

## Evidence

The bounded branch narrows only the item's own generations — `src/ops/delete_custom_line_item.rs:217-228`:
```
                match item.remove_periods(&removed) {
                    crate::state::PeriodsRemoval::Changed => {
                        // The record changed, so the instant a read reports as its most
                        // recent modification advances with it.
                        item.last_modified_time_millis = crate::state::now_millis();
                        // An item left applying in no billing period at all has nothing
                        // for a read to report, so its record goes.
                        if item.is_empty() {
                            ("deleted", true)
                        } else {
                            ("narrowed", false)
                        }
                    }
```
and only the `gone` branch reclaims associations — `src/ops/delete_custom_line_item.rs:252-257`:
```
        if gone {
            // One state method removes the item and everything that only existed
            // because of it: the associations other percentage items held to it, and
            // the idempotency entries that named it.
            state.remove_custom_line_item(&item_arn);
        }
```

What `remove_periods` promises about the deleted-from periods — `src/state.rs:1931-1934`:
```
    /// EVERY generation loses them, because a period the item was deleted from reports
    /// no version of it at all. Nothing is written unless every generation's removal
    /// stays inside [`MAX_BILLING_PERIOD_SPANS`], so a refused removal leaves the whole
    /// item as it was.
```

The rule the FULL delete enforces — `src/state.rs:2915-2926`:
```
    /// Delete a custom line item and everything that only existed because of it: the
    /// associations other percentage custom line items held TO it (an association
    /// names a resource that exists, and its own associations go with the record) and
    /// the idempotency entries that named it ...
    pub fn remove_custom_line_item(&mut self, item_arn: &str) -> Option<CustomLineItemRecord> {
        let removed = self.custom_line_items.remove(item_arn)?;

        for item in self.custom_line_items.values_mut() {
            item.associated_resources
                .retain(|association| association.arn != item_arn);
        }
```
pinned by `tests/all/delete_custom_line_item.rs:968-970`:
```
/// THE RULE on what a delete takes with the item: an association names a resource that
/// exists, so the associations other percentage custom line items held TO the deleted
/// item go with it — and nothing else about those items changes.
```
(`deleting_an_item_releases_the_associations_other_items_held_to_it`, tests/all/delete_custom_line_item.rs:971-1010, asserts `AssociationSize` drops from 2 to 1 — but only for a delete with `&Value::Null`, i.e. no range. No test covers the associations after a NARROWING delete.)

The helper the bounded path skips, used by the sibling that ends an association over a range — `src/ops/batch_disassociate_resources_from_custom_line_item.rs:193-206`:
```
fn apply_disassociation(
    associations: &mut Vec<crate::state::ResourceAssociationRecord>,
    index: usize,
    removed: &crate::state::BillingPeriodSpan,
) -> crate::state::PeriodsRemoval {
    let Some(association) = associations.get_mut(index) else {
        return crate::state::PeriodsRemoval::Unchanged;
    };
    let outcome = association.remove_periods(removed);
    if association.is_empty() {
        associations.remove(index);
    }
    outcome
}
```

The read that surfaces the leftover — `src/ops/list_resources_associated_to_custom_line_item.rs:212-217` (it judges the ASSOCIATION's periods and never the item's):
```
        for association in &item.associated_resources {
            // An association is listed for a period its range covers, the start
            // inclusive and the end exclusive.
            if !association.covers(&billing_period) {
                continue;
            }
```
against the sibling period-scoping rule — `src/ops/list_custom_line_items.rs:288-293`:
```
            // The version of the item in effect in the billing period asked about is
            // what this read reports; an item no version of which applies in that period
            // is not reported at all.
            let Some(version) = item.effective_in(&billing_period) else {
                continue;
            };
```

Reachability: `check_billing_period_range_is_modifiable` admits a start in the PREVIOUS billing period (`src/state.rs:4237` `const MODIFIABLE_BILLING_PERIODS_BACK: i64 = 1;`), so an item created from the previous period on can be deleted from the current period on and still keep the previous one — the `("narrowed", false)` outcome above.

## Steps to Reproduce

1. Designate the emulated account as the payer so the control plane is reachable (set `ComAmazonawsBillingconductorAwsbillingconductorState::payer_account_id`).
2. Create a pricing plan and a billing group.
3. POST /create-custom-line-item for a child item C in that group with {"BillingPeriodRange":{"InclusiveStartBillingPeriod":"<previous billing period>"}} — C's spans are [previous, open).
4. POST /create-custom-line-item for a percentage parent item A in the same group with {"ChargeDetails":{"Type":"FEE","Percentage":{"PercentageValue":10.0,"AssociatedValues":["<C arn>"]}}} and no BillingPeriodRange — A's association to C runs from the current billing period on.
5. POST /delete-custom-line-item with {"Arn":"<C arn>","BillingPeriodRange":{"InclusiveStartBillingPeriod":"<current billing period>"}} — 200, outcome "narrowed": C now applies only in the previous billing period.
6. POST /list-custom-line-items with {"BillingPeriod":"<current billing period>"} and observe C is NOT reported (it applies in no generation of that period).
7. POST /list-resources-associated-to-custom-line-item with {"Arn":"<A arn>"} (defaulting to the current billing period) and observe C still reported as an associated resource, and A's AssociationSize still 1.
8. Compare with the full delete: repeat steps 2-4 and POST /delete-custom-line-item with only {"Arn":"<C arn>"} — the association is dropped and A's AssociationSize becomes 0 (`tests/all/delete_custom_line_item.rs:971-1010`), although the two deletes removed C from exactly the same billing periods a default read looks at.
