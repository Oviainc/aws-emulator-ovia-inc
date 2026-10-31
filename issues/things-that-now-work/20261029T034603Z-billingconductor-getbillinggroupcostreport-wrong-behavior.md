Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: e79dd1ec68971fa3f627b273166ce9e704773849

# The cycle-break zero of a percentage custom line item IS cached as the item's charge, so the billing group's ProformaCost depends on which of two mutually associated items was created first

_Filed: 2026-10-29T03:46:03Z_
_Operation: GetBillingGroupCostReport | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `e79dd1ec68971fa3f627b273166ce9e704773849`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`proforma_custom_line_item_charges` computes the pro forma charges of one billing group in one billing period by walking every custom line item of the group, sharing one `computed` cache and one `being_computed` cycle guard across the whole walk.

The cycle guard works as the comment describes for the item it guards: re-entering an item whose charge is being computed returns `0.0` before `computed.insert`, so that zero is not remembered FOR THAT ITEM. But the CALLER's charge — which was computed from that zero — IS remembered: `custom_line_item_charge` ends with `computed.insert(item.arn.clone(), charge)` unconditionally, and the top-level loop then reads that cached value instead of recomputing it now that the guarded item has settled.

Because the top-level loop iterates `self.custom_line_items.values()`, i.e. in full-ARN order, and an ARN carries the zero-padded monotonic id this service mints (so ARN order is creation order), the group's reported `ProformaCost` for two percentage items that are each other's associated resource depends on which of them was created first. With A (percentage 100%) associated to B, and B (percentage 100%) associated to A and to a flat item C of 100:

* A created first: A's walk computes B with A guarded, so B = 100 and A = 100; the group's charges total 300 (A 100 + B 100 + C 100).
* B created first: B's walk computes A with B guarded, so A = 0 and B = 100; the cached A = 0 is what the top-level loop reports, and the total is 200.

Same store contents, same request, two different `ProformaCost`/`Margin`/`MarginPercentage` answers from `GetBillingGroupCostReport` and `ListBillingGroupCostReports`, decided by creation order alone — and the comment above the walk states the opposite rule, that the cycle zero "is not remembered — it is a property of the walk rather than of the item". A fix would make the cache honour that sentence: do not cache a charge computed while the cycle guard suppressed one of its inputs.

## Evidence

The rule the state module states — `src/state.rs:3436-3441`:
```
    /// `computed` carries the charges already worked out for this billing period, so an
    /// item several others are associated to is computed once. `being_computed` carries
    /// the items whose charge this walk is in the middle of: nothing stops two percentage
    /// items from being associated to each other, and an item whose own value is not
    /// settled yet has no value to lend, so it contributes nothing and that nothing is not
    /// remembered — it is a property of the walk rather than of the item.
```

The code that breaks it — `src/state.rs:3453-3455`, `:3461-3465` and `:3483-3491`:
```
        if let Some(charge) = computed.get(&item.arn) {
            return *charge;
        }
```
```
        } else if let Some(percentage) = details.percentage {
            let percentage_value = percentage.percentage_value;
            if !being_computed.insert(item.arn.clone()) {
                return 0.0;
            }
```
```
            being_computed.remove(&item.arn);
            percentage_value / 100.0 * combined
        } else {
            // Neither charge member: an item no create of this service can leave behind,
            // since exactly one of them is required.
            0.0
        };
        computed.insert(item.arn.clone(), charge);
        charge
```
(the `computed.insert` is unconditional, so the caller's charge — computed from the suppressed zero — becomes the item's remembered charge for the rest of the report).

The walk order that then decides the answer — `src/state.rs:3405-3419`:
```
        for item in self.custom_line_items.values() {
            if item.billing_group_arn != group_arn || !item.covers(billing_period) {
                continue;
            }
            let charge = charge_sign(&item.charge_type)
                * self.custom_line_item_charge(
                    item,
                    billing_period,
                    &mut computed,
                    &mut being_computed,
                );
```
and the crate's own statement that this order is creation order — `src/ops/list_custom_line_items.rs:284-286`:
```
        // `custom_line_items` is ordered by ARN, and an ARN carries the zero-padded
        // monotonic id this service mints, so the sequence is creation order and a
        // `NextToken` names the same cut point on the next call.
```

The reader that reports it — `src/ops/get_billing_group_cost_report.rs:296-299`:
```
        for billing_period in &billing_periods {
            for (product_name, charge) in
                state.proforma_custom_line_item_charges(&group.arn, billing_period)
            {
```
and `src/ops/list_billing_group_cost_reports.rs:233-237`:
```
                let proforma_cost: f64 = state
                    .proforma_custom_line_item_charges(&group.arn, &billing_period)
                    .values()
                    .sum();
```

The mutual association is reachable: `BatchAssociateResourcesToCustomLineItem` only refuses an element naming the target itself and one out of the target's billing group — `src/ops/batch_associate_resources_to_custom_line_item.rs:289-309`:
```
                    // A custom line item cannot be a resource of itself: its charge
                    // would be a percentage of itself, which has no value.
                    Some(arn) if arn == target_arn => Err((
                        "ILLEGAL_CUSTOMLINEITEM",
                        "a custom line item cannot be associated to itself".to_string(),
                    )),
```
so A←B and B←A are both accepted for two percentage items of one group.

Test coverage: none. `tests/all/billing_group_cost_report.rs` and `tests/all/percentage_custom_line_item_resource_associations.rs` never associate two percentage items to each other, so no suite exercises the cycle guard at all.

## Steps to Reproduce

1. Designate the emulated account as the payer so the control plane is reachable.
2. Create a pricing plan and a billing group G.
3. POST /create-custom-line-item for G with a FLAT charge of 100 (keep ARN C).
4. POST /create-custom-line-item for G with {"ChargeDetails":{"Type":"FEE","Percentage":{"PercentageValue":100}}} (keep ARN A), then again (keep ARN B). A's id is minted before B's, so A's ARN sorts first.
5. PUT /batch-associate-resources-to-custom-line-item with {"TargetArn":A,"ResourceArns":[B]} and again with {"TargetArn":B,"ResourceArns":[A,C]} — both answered 200 with the resources in SuccessfullyAssociatedResources.
6. POST /get-billing-group-cost-report with {"Arn":G} and note ProformaCost: A's walk runs first and computes B with A guarded, so A = 100, B = 100, C = 100 and the total is 300.
7. Repeat the whole setup in a fresh store but create B BEFORE A (so B's ARN sorts first) with the same associations. POST /get-billing-group-cost-report with {"Arn":G} — the total is now 200, because A's charge was cached as 0 while B's walk had A guarded and the top-level pass reads that cached 0.
8. Observe that the same logical configuration yields two different ProformaCost/Margin/MarginPercentage answers, decided only by which percentage item was created first — although src/state.rs:3439-3441 states the cycle zero "is not remembered — it is a property of the walk rather than of the item".
