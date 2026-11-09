Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: a7f9a115bc9a804744e4f0c3783ea962e700da67

# An association is stored for billing periods the target custom line item applies in in no generation, so a period reports an association to an item it reports no version of

_Filed: 2026-11-09T10:54:02Z_
_Operation: BatchAssociateResourcesToCustomLineItem | Class: validation_

> _Note: this issue was identified against the code base at commit `a7f9a115bc9a804744e4f0c3783ea962e700da67`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`BatchAssociateResourcesToCustomLineItem` validates its `BillingPeriodRange` for being ordered and for starting in an open bill, but never against the billing periods the TARGET item itself applies in. The span it stores is built from the request's range alone, so a request whose range begins where the item has already stopped applying stores an association record in effect in periods no generation of the item covers.

That is the state the crate's own writer goes out of its way to prevent: `remove_custom_line_item_periods` narrows every association keyed on an item by exactly the periods the item loses, and states the invariant as a flat rule — an association is in effect only in periods the resource it names is there to be associated in, so "no read of that period can report an association to an item it reports no version of". After this call that is precisely what happens: `ListResourcesAssociatedToCustomLineItem` with that billing period reports the resource (its only test is `association.covers(billing_period)`), while `ListCustomLineItems` for the same period reports no item and `ListCustomLineItemVersions` reports no version covering it.

The sibling write path does not have the hole: `CreateCustomLineItem` builds each association's span from the SAME `period_range` it builds the item's own span from, so a created association can never reach past the item. Only the batch associate takes an independent range and compares it with nothing.

## Evidence

The invariant the crate states, in the writer that maintains it — `src/state.rs:3436-3447`:

    /// A resource association is in effect only in billing periods the resource it names
    /// is there to be associated in, so narrowing the periods a resource applies in
    /// narrows by the same periods every association keyed on that resource — the ones
    /// the resource itself holds and the ones other resources hold TO it — and an
    /// association left in effect in no period at all goes
    /// ([`remove_association_periods`]). It is the bounded form of the rule a delete that
    /// names no range enforces by removing the record outright
    /// ([`Self::remove_custom_line_item`]): a period the item applies in in no generation
    /// of it holds no association of it either, so no read of that period can report an
    /// association to an item it reports no version of.

The batch associate stores the request's range with no comparison against the item — `src/ops/batch_associate_resources_to_custom_line_item.rs:363-370` and `:428-434`:

            let added = crate::state::BillingPeriodSpan::started_now(
                inclusive_start_billing_period.clone(),
                exclusive_end_billing_period.clone(),
            );
    ...
            target
                .associated_resources
                .push(crate::state::ResourceAssociationRecord::new(
                    resource_arn, added,
                ));
            target.last_modified_time_millis = now;

Its only checks on that range are ordering and the open bill — `src/ops/batch_associate_resources_to_custom_line_item.rs:261-281` (`check_billing_period_range_is_ordered`, `check_billing_period_range_starts_in_an_open_bill`); neither reads `target.spans`.

The sibling create derives the association span from the item's own range — `src/ops/create_custom_line_item.rs:344-353`:

            associated_resources.push(crate::state::ResourceAssociationRecord::new(
                arn,
                crate::state::BillingPeriodSpan::new(
                    period_range
                        .as_ref()
                        .map(|range| range.inclusive_start_billing_period.clone()),
                    period_range
                        .as_ref()
                        .and_then(|range| range.exclusive_end_billing_period.clone()),
                ),

— the same `period_range` it gives the item at `src/ops/create_custom_line_item.rs:403-408` (`spans: vec![crate::state::BillingPeriodSpan::started_now(...)]`).

The reader that then disagrees with the item's own periods — `src/ops/list_resources_associated_to_custom_line_item.rs:212-235`:

        for association in &item.associated_resources {
            // An association is listed for a period its range covers, the start
            // inclusive and the end exclusive.
            if !association.covers(&billing_period) {
                continue;
            }

Test coverage: `tests/all/resource_association_billing_periods.rs` and `tests/all/batch_resource_association_closed_billing_period.rs` cover ranges inside the item's own periods and the closed-bill refusal; `tests/all/bounded_delete_narrows_resource_associations.rs` covers the narrowing side of the invariant. No test associates a range that begins after the target item has stopped applying.

## Steps to Reproduce

1. Start the emulator with the emulated account designated the payer account (as the suites do: `engine.state::<BillingConductorState>().lock().payer_account_id = Some(emulator_account_id())`).
2. POST /create-billing-group with `ComputationPreference.PricingPlanArn = arn:aws:billingconductor::aws:pricingplan/BasicPricingPlan`.
3. POST /create-custom-line-item with `{Name:"P", Description:"d", BillingGroupArn:<group>, ChargeDetails:{Type:"FEE", Percentage:{PercentageValue:10}}, BillingPeriodRange:{InclusiveStartBillingPeriod:<previous billing period>, ExclusiveEndBillingPeriod:<current billing period>}}` — the item applies in the PREVIOUS period only.
4. PUT /batch-associate-resources-to-custom-line-item with `{TargetArn:<item>, ResourceArns:[<group>], BillingPeriodRange:{InclusiveStartBillingPeriod:<current billing period>}}` -> HTTP 200, the resource in `SuccessfullyAssociatedResources`.
5. POST /list-resources-associated-to-custom-line-item with `{Arn:<item>, BillingPeriod:<current billing period>}` -> the group is reported as an associated resource.
6. POST /list-custom-line-items with `{BillingPeriod:<current billing period>, Filters:{Arns:[<item>]}}` -> empty, and POST /list-custom-line-item-versions with `{Arn:<item>}` -> no version whose [StartBillingPeriod, EndBillingPeriod) covers the current period. The period reports an association to an item it reports no version of, which `src/state.rs:3443-3447` states must not happen.
