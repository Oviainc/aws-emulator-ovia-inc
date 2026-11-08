Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 20f62a5c947e5bdfd6ecb788e0936a3bc521f6a6

# An UpdateCustomLineItem naming no BillingPeriodRange erases every superseded generation, so the item vanishes from the earlier billing periods they held

_Filed: 2026-11-08T23:54:54Z_
_Operation: UpdateCustomLineItem | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `20f62a5c947e5bdfd6ecb788e0936a3bc521f6a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

When `UpdateCustomLineItem` carries a `BillingPeriodRange`, the handler passes its start to `put_custom_line_item` as `takes_effect_from`, and the writer correctly freezes the generation it replaces holding exactly the periods before that start. When the request carries NO range, the handler passes `takes_effect_from = None`, and the writer then builds the "taken" span as `BillingPeriodSpan::new(None, None)` — a span open at BOTH ends — and subtracts it from every generation's spans, including the ones the item already had frozen. Because an absent bound is open on this axis, that subtraction leaves EVERY generation with an empty span list, and `kept.retain(CustomLineItemVersion::applies_anywhere)` then drops all of them.

The new generation does not take those periods over in their place: `merge()` only replaces `updated.spans` when the request names a range, so after a no-range update the record's spans are exactly what they were (e.g. `[(current, open)]`) while `superseded_versions` is now empty. The billing periods an earlier generation held are therefore not re-homed, they are LOST: `CustomLineItemRecord::covers` answers false for them, so `ListCustomLineItems` reports no item for those periods, `ListCustomLineItemVersions` reports one fewer version, and `GetBillingGroupCostReport` / `ListBillingGroupCostReports` stop counting the charge the item really carried in them. A request that named no billing period at all thus retroactively rewrites billing periods the crate's own window check (`check_billing_period_range_is_modifiable`) refuses a range-bearing request from touching.

This is a different path from the already-fixed range-bearing case: the fix that introduced `takes_effect_from` handled `Some(start)` and left `None` subtracting an unbounded span from the kept history.

## Evidence

The writer's own doc states the opposite of what the code does — `services/service-com-amazonaws-billingconductor-awsbillingconductor/src/state.rs:3493-3495`:

    /// A write that names
    /// no billing period to be applied from (`takes_effect_from` `None`) takes over
    /// wherever the item applies, so it REPLACES the current generation and the item's
    /// history is left as it was.

and the code under it (`src/state.rs:3516-3532`):

    // The billing periods this write takes over: the one it is applied from and
    // every later one, which is every period at all when it names none.
    let taken = BillingPeriodSpan::new(takes_effect_from.map(str::to_string), None);
    for generation in kept.iter_mut().chain(std::iter::once(&mut replaced)) {
        remove_billing_periods(&mut generation.spans, &taken);
    }
    // A generation this write left no billing period to is not a version of the
    // item in any period, so there is nothing for a read to report it for.
    kept.retain(CustomLineItemVersion::applies_anywhere);
    ...
    updated.superseded_versions = kept;

A both-open removal empties every span list: `BillingPeriodSpan::minus` (`src/state.rs:389-413`) returns `vec![]` when `removed_start.is_some()` and `removed_end.is_some()` are both false and the overlap guard passes, and the guard passes because `billing_period_precedes` treats an absent bound as open — `src/state.rs:280-288`: "An absent `start` is the unbounded past and an absent `end` the unbounded future, so either one absent orders." `CustomLineItemVersion::applies_anywhere` is then `!self.spans.is_empty()` (`src/state.rs:2447-2449`), so every kept generation is discarded.

A second crate statement the code breaks — `CustomLineItemVersion::covers`, `src/state.rs:2433-2436`:

    /// Does this generation apply in `billing_period`? Each stretch it applies in is
    /// half-open, and a write that named no billing period range leaves the generation
    /// holding the stretches the item already applied in

The handler also contradicts itself about what "no range" means — `src/ops/update_custom_line_item.rs:219-223` treats it as the CURRENT period:

    let applied_from = input
        .billing_period_range
        .as_ref()
        .map(|range| range.inclusive_start_billing_period.clone())
        .unwrap_or_else(crate::state::current_billing_period);

while `src/ops/update_custom_line_item.rs:230-233` + `:255` hand the writer `None`, i.e. "every period at all":

    let takes_effect_from = input
        .billing_period_range
        .as_ref()
        .map(|range| range.inclusive_start_billing_period.clone());
    ...
    state.put_custom_line_item(updated, now, takes_effect_from.as_deref());

And the spans are not re-homed: `merge()` assigns `updated.spans` only inside `if let Some(range) = input.billing_period_range.as_ref()` (`src/ops/update_custom_line_item.rs:359-381`).

Test coverage: `tests/all/custom_line_item_versions_by_billing_period.rs:694-719` (`a_write_naming_no_range_replaces_the_version_it_applies_in`) drives this path, but its fixture has only ONE generation (`fixture("whole", json!({}))`, item created from `created_from()` = `period(-1)`), so emptying a kept history that is already empty is indistinguishable there and the test passes either way. `tests/all/update_custom_line_item.rs:681-734` (`the_billing_period_range_decides_which_periods_report_the_updated_item`) only exercises updates that DO name a range. No test covers a no-range update against an item that already holds a superseded generation.

## Steps to Reproduce

1. Start the emulator with the emulated account designated the payer account (as the test harnesses do: `engine.state::<BillingConductorState>().lock().payer_account_id = Some(emulator_account_id())`).
2. POST /create-billing-group with `ComputationPreference.PricingPlanArn = arn:aws:billingconductor::aws:pricingplan/BasicPricingPlan`.
3. POST /create-custom-line-item with `Name="A"`, `ChargeDetails={Type:FEE, Flat:{ChargeValue:10}}` and `BillingPeriodRange={InclusiveStartBillingPeriod:<previous billing period>}`.
4. POST /update-custom-line-item with `{Arn, Name:"B", BillingPeriodRange:{InclusiveStartBillingPeriod:<current billing period>}}` — the item now holds two generations.
5. POST /list-custom-line-items with `{BillingPeriod:<previous billing period>, Filters:{Arns:[<item>]}}` -> the item is reported with Name "A"; POST /list-custom-line-item-versions with `{Arn}` -> 2 versions.
6. POST /update-custom-line-item with `{Arn, Description:"D2"}` and NO BillingPeriodRange.
7. POST /list-custom-line-items with `{BillingPeriod:<previous billing period>, Filters:{Arns:[<item>]}}` -> now EMPTY: the item no longer applies in a period it applied in, and nothing in the request named that period. POST /list-custom-line-item-versions -> 1 version. POST /get-billing-group-cost-report over `[<previous>, <current>+1)` with `GroupBy:["BILLING_PERIOD"]` -> the previous period's ProformaCost has lost the 10.00 charge.
