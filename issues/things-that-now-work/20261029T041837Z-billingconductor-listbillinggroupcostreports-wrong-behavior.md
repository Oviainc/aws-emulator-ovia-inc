Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 1fd13cac8c9d59f908c3acc103dd89ab8a3a1d61

# A percentage custom line item's charge counts an associated child item that applies in no generation of the billing period, using the child's LIVE charge details

_Filed: 2026-10-29T04:18:37Z_
_Operation: ListBillingGroupCostReports | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `1fd13cac8c9d59f908c3acc103dd89ab8a3a1d61`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`proforma_custom_line_item_charges` is the one helper both cost reports compute `ProformaCost` (and therefore `Margin` / `MarginPercentage`) from. It filters the TOP-LEVEL items of the billing group on `item.covers(billing_period)`, so an item that applies in no generation of the period asked about contributes nothing. The recursive path that values the children of a PERCENTAGE item applies no such filter: it filters only on the ASSOCIATION's own billing periods and then calls `custom_line_item_charge` on the child.

Inside `custom_line_item_charge` the child's charge details are read as `item.effective_in(billing_period).map_or_else(|| item.charge_details(), |version| version.charge_details)` — so when no generation of the child applies in that period (`effective_in` is `None`), the code silently falls back to the child's LIVE charge details and lends the parent its full charge value. The fallback is unreachable for a top-level item (`covers()` is true exactly when `effective_in()` is `Some`, since `current_version().spans == self.spans`), so it fires only on the child path, i.e. only in the case where the child does not exist in that period at all.

Effect: after a BOUNDED `DeleteCustomLineItem` takes a flat child out of one billing period (the item survives, so `remove_custom_line_item` is not called and the parent's association to it is kept), the cost report for that period still prices the parent's percentage over the child's charge. The period's `ProformaCost` reports money from a line item that is in effect in none of that period's generations. The wrong value is also cached (`computed.insert(item.arn, charge)`), so every other parent reached in the same pass gets it too.

The same helper is read by `GetBillingGroupCostReport`; I am filing this against `ListBillingGroupCostReports` because the (GetBillingGroupCostReport, wrong-behavior) slot is already held by a DIFFERENT open issue about the same helper (the cycle-break zero being cached, `20261029T034603Z-billingconductor-getbillinggroupcostreport-wrong-behavior.md`), and one fix of this defect fixes both readers.

## Evidence

The helper's own documented rule — `src/state.rs:3443-3445`:
```
    /// The charge values are the ones of the GENERATION of the item that applies in that
    /// billing period ([`CustomLineItemRecord::effective_in`]), so a bill of a period a
    /// later write did not claim is computed from what the item charged then.
```
but the code falls back to the LIVE record when no generation applies — `src/state.rs:3456-3458`:
```
        let details = item
            .effective_in(billing_period)
            .map_or_else(|| item.charge_details(), |version| version.charge_details);
```

The TOP-LEVEL loop does apply the period filter — `src/state.rs:3405-3408`:
```
        for item in self.custom_line_items.values() {
            if item.billing_group_arn != group_arn || !item.covers(billing_period) {
                continue;
            }
```
while the CHILD path filters only the association — `src/state.rs:3466-3480`:
```
            let combined: f64 = item
                .associated_resources
                .iter()
                .filter(|association| association.covers(billing_period))
                .map(|association| {
                    self.custom_line_items
                        .get(&association.arn)
                        .map_or(0.0, |associated| {
                            self.custom_line_item_charge(
                                associated,
                                billing_period,
                                computed,
                                being_computed,
                            )
                        })
                })
                .sum();
```

That the fallback is reachable ONLY for a child the period does not hold — `src/state.rs:1865-1891`:
```
    pub fn covers(&self, billing_period: &str) -> bool {
        self.spans.iter().any(|span| span.covers(billing_period))
            || self
                .superseded_versions
                .iter()
                .any(|version| version.covers(billing_period))
    }
...
    pub fn effective_in(&self, billing_period: &str) -> Option<CustomLineItemVersion> {
        let current = self.current_version();
        if current.covers(billing_period) {
            return Some(current);
        }
        self.superseded_versions
            .iter()
            .rev()
            .find(|version| version.covers(billing_period))
            .cloned()
    }
```

The affected reader — `src/ops/list_billing_group_cost_reports.rs:233-236`:
```
                let proforma_cost: f64 = state
                    .proforma_custom_line_item_charges(&group.arn, &billing_period)
                    .values()
                    .sum();
```

That a bounded delete leaves the association in place — `src/ops/delete_custom_line_item.rs:252-257` only calls `remove_custom_line_item` when `gone`, and `gone` is false for a `narrowed` item:
```
        if gone {
            // One state method removes the item and everything that only existed
            // because of it: the associations other percentage items held to it, and
            // the idempotency entries that named it.
            state.remove_custom_line_item(&item_arn);
        }
```

Test coverage: `tests/all/percentage_custom_line_item_resource_associations.rs` and `tests/all/billing_group_cost_report.rs` exercise percentage items whose children DO apply in the period read; no test covers a child that applies in none of that period's generations, so nothing pins the current answer.

## Steps to Reproduce

1. Designate the emulated account as the payer so the control plane is reachable, and create a pricing plan and a billing group G.
2. POST /create-custom-line-item for B in G: {"Name":"B","Description":"child","BillingGroupArn":"<G>","ChargeDetails":{"Type":"FEE","Flat":{"ChargeValue":100}}} — B applies from the current billing period P onward.
3. POST /create-custom-line-item for A in G: {"Name":"A","Description":"parent","BillingGroupArn":"<G>","ChargeDetails":{"Type":"FEE","Percentage":{"PercentageValue":10,"AssociatedValues":["<B arn>"]}}}.
4. POST /list-billing-group-cost-reports with {"BillingPeriod":"<P>"} and note ProformaCost = 110 (100 from B, 10 from A).
5. POST /delete-custom-line-item with {"Arn":"<B arn>","BillingPeriodRange":{"InclusiveStartBillingPeriod":"<P>","ExclusiveEndBillingPeriod":"<P+1>"}} — B now applies from P+1 onward and in no part of P, and its record survives, so A's association to it is untouched.
6. POST /list-billing-group-cost-reports with {"BillingPeriod":"<P>"} again: ProformaCost is 10, not 0 — A's percentage was still taken over B's live $100 although no generation of B applies in P. The same value appears in POST /get-billing-group-cost-report for P.
7. Compare: POST /list-custom-line-items with {"BillingPeriod":"<P>"} does not report B at all for that period.
