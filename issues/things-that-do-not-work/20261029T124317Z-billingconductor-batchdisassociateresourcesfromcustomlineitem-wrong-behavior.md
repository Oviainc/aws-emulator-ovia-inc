Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: c73cb477601ff7c7855bce46e4f89627c43b834e

# A disassociation naming no BillingPeriodRange erases the association from the EARLIER billing periods it was in effect in, unlike every other disassociation path in the crate

_Filed: 2026-10-29T12:43:17Z_
_Operation: BatchDisassociateResourcesFromCustomLineItem | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `c73cb477601ff7c7855bce46e4f89627c43b834e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

When `BatchDisassociateResourcesFromCustomLineItem` is called WITHOUT a `BillingPeriodRange`, the handler builds the removal span with `.unwrap_or_default()`, i.e. the fully open span `{inclusive_start: None, exclusive_end: None}`. `BillingPeriodSpan::minus` with both removal bounds absent returns an empty vector, so `remove_billing_periods` replaces the association's spans with nothing and `apply_disassociation` then deletes the `ResourceAssociationRecord` outright.

That destroys the record of the billing periods the association WAS in effect in. Those periods are closed bills: a read scoped to one of them (`ListResourcesAssociatedToCustomLineItem` with `BillingPeriod`) must still report the resource, and `proforma_custom_line_item_charges` decides a period's percentage base from `association.covers(billing_period)`, so the already-billed `ProformaCost`/`Margin` of that period silently changes too.

Every other disassociation path of THIS crate does the opposite: it ENDS the record at the current billing period and keeps the earlier periods reportable. The crate states that rule three times — for pricing-rule associations (`DisassociatePricingRules`, which likewise takes no range at all), for account memberships (`release_account` / `remove_billing_group`), and for this very operation's own BOUNDED path, whose test doc says the association "keeps the earlier periods it applied in". Only the unbounded path of this one handler forgets them.

Note this is a different gap from the already-fixed `parameter-ignored` issue on this operation (`ExclusiveEndBillingPeriod` dropped): that one is about the bounded request's end bound; this one is about a request carrying no range at all.

## Evidence

The unbounded removal span, and the comment that chooses it — `services/service-com-amazonaws-billingconductor-awsbillingconductor/src/ops/batch_disassociate_resources_from_custom_line_item.rs:284-304`:

```
// The billing periods the disassociation applies to: the start is inclusive and
// the end exclusive, ... A request that names no range disassociates the resource in every
// billing period, which is the fully open span.
...
let removed = input
    .billing_period_range
    .as_ref()
    .map(|range| { crate::state::BillingPeriodSpan::new(...) })
    .unwrap_or_default();
```

What that span does — `state.rs:264-267` (`#[derive(Debug, Clone, Default, PartialEq, Eq)] pub struct BillingPeriodSpan { pub inclusive_start: Option<String>, pub exclusive_end: Option<String> }`, so `default()` is `{None, None}`) and `state.rs:334-347`:

```
let mut left = Vec::new();
if removed_start.is_some() && billing_period_precedes(start, removed_start) { left.push(...); }
if removed_end.is_some() && billing_period_precedes(removed_end, end) { left.push(...); }
left
```

with both `removed_start` and `removed_end` `None`, `left` is empty; `remove_billing_periods` then does `*spans = left;` (`state.rs:427`) and the handler drops the record — `batch_disassociate_resources_from_custom_line_item.rs:201-204`:

```
let outcome = association.remove_periods(removed);
if association.is_empty() {
    associations.remove(index);
}
```

The rule the SIBLING disassociation of a parent's associations applies, with no range argument either — `src/ops/disassociate_pricing_rules.rs:246-249`:

```
// Every check passed, so the whole request applies: each requested rule's
// association ENDS at the billing period this call is made in — in force in
// every earlier period, so a read scoped to one of those still reports it, and
// in none from this period on
```

The same rule for account memberships — `state.rs:2722-2724` (`release_account`: "the membership of that group it still has ends at the CURRENT billing period") and `state.rs:2841-2843` (`remove_billing_group`: "The memberships the group held END with it, rather than being forgotten: a read scoped to a billing period the group existed in still has to report the accounts it held then.").

The same rule for this operation's own BOUNDED path, as its suite states and pins — `tests/all/batch_disassociate_resources_from_custom_line_item.rs:745-749`:

```
/// A disassociation bounded to a billing period range takes effect FROM its inclusive
/// start: the association keeps the earlier periods it applied in and ends there, and
/// is dropped only when it would otherwise cover no period at all.
#[tokio::test]
async fn a_ranged_disassociation_ends_the_association_at_its_inclusive_start() {
```

Test coverage of the unbounded case is only for a period the association never covered, so the defect is uncovered: `tests/all/resource_association_billing_periods.rs:434-438` disassociates with `disassociated_from: None` but the association under test was made with no range of its own (so it covered only the current period onward — `state.rs:1631-1634`, `ResourceAssociationRecord::new`: "An association is in effect no EARLIER than the billing period the call that made it was made in"), and `tests/all/batch_disassociate_resources_from_custom_line_item.rs:750-800` only ever sends `{"InclusiveStartBillingPeriod": ...}`.

## Steps to Reproduce

1. Create a billing group G (`CreateBillingGroup`) and a percentage custom line item T inside it (`CreateCustomLineItem` with `ChargeDetails.Percentage`, no `BillingPeriodRange`).
2. Associate G to T over a range that starts in the PREVIOUS billing period: `BatchAssociateResourcesToCustomLineItem { TargetArn: T, ResourceArns: [G], BillingPeriodRange: { InclusiveStartBillingPeriod: "<previous billing period>" } }` → 200, G in `SuccessfullyAssociatedResources`.
3. `ListResourcesAssociatedToCustomLineItem { Arn: T, BillingPeriod: "<previous billing period>" }` → G is reported (the association is in effect there).
4. `BatchDisassociateResourcesFromCustomLineItem { TargetArn: T, ResourceArns: [G] }` with NO `BillingPeriodRange` → 200, G in `SuccessfullyDisassociatedResources`.
5. `ListResourcesAssociatedToCustomLineItem { Arn: T, BillingPeriod: "<previous billing period>" }` → G is NOT reported; the whole association record was deleted, so the closed period no longer reports the resource it was associated to there, and `GetBillingGroupCostReport` for that period computes T's percentage base without G.
6. Contrast the bounded form: repeat steps 1-3 and then send `BatchDisassociateResourcesFromCustomLineItem { TargetArn: T, ResourceArns: [G], BillingPeriodRange: { InclusiveStartBillingPeriod: "<current billing period>" } }` → the read in step 5 still reports G with `EndBillingPeriod` = the current period, which is what the sibling `DisassociatePricingRules` and the account-membership release also do.
