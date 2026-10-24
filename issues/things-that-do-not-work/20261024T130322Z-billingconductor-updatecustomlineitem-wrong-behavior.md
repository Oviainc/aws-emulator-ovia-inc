Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: f04518a3ee65f2e7815d98c430f437f66966ee67

# UpdateCustomLineItem rewrites the item's whole effective range instead of creating a version, so the item disappears from earlier billing periods

_Filed: 2026-10-24T13:03:22Z_
_Operation: UpdateCustomLineItem | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f04518a3ee65f2e7815d98c430f437f66966ee67`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: a custom line item is VERSIONED by billing period. `UpdateCustomLineItem.BillingPeriodRange` is "the billing period range in which the custom line item request will be applied" — the new values (Name, Description, ChargeDetails, filters) take effect for the periods that range covers, while the periods BEFORE it keep the version the item had, which is exactly what `ListCustomLineItemVersions` reports (each `CustomLineItemVersionListElement` carries its own `StartBillingPeriod` / `EndBillingPeriod` beside its own `Name`, `ChargeDetails` and `AssociationSize`). So after creating an item and then updating it with `BillingPeriodRange={InclusiveStartBillingPeriod:2026-10}`, a `ListCustomLineItems` for 2026-09 must still report the item with its OLD values.

Actual: the handler merges the request into the single stored record and OVERWRITES the record's `inclusive_start_billing_period` / `exclusive_end_billing_period` with the range the request named (`src/ops/update_custom_line_item.rs`, `merge()` lines 303-315). The item then exists only inside the newly named range: a read of any earlier billing period reports NO such item at all (`CustomLineItemRecord::covers`, `src/state.rs`), and the values the earlier periods used are destroyed rather than kept as a prior version. A narrowing update is therefore observable as a retroactive deletion of the item from closed billing periods.

## Evidence

Smithy model: `CustomLineItemVersionListElement` (StartBillingPeriod, EndBillingPeriod, Name, ChargeDetails, AssociationSize, CreationTime, LastModifiedTime) and `ListCustomLineItemVersionsBillingPeriodRangeFilter` establish that one custom line item holds several per-billing-period versions; `UpdateCustomLineItemInput$BillingPeriodRange` is documented as the range the request is applied in. Handler: `src/ops/update_custom_line_item.rs` lines 294-315 (the handler's own comment concedes "AWS keeps one VERSION of the item per billing period range ... this store holds the item's current generation"). State: `CustomLineItemRecord` has exactly one range and one set of values (`src/state.rs` lines 483-535). Test coverage: `tests/all/update_custom_line_item.rs::the_billing_period_range_decides_which_periods_report_the_updated_item` (lines 606-670) PINS the divergent behaviour — it asserts that periods outside the updated range report no item at all (e.g. after an update ranged from 2026-10, 2026-09 and 2025-10 must report nothing), so the gap is asserted-in, not merely untested.

## Steps to Reproduce

1. Authorize as the payer account; POST /create-pricing-plan and POST /create-billing-group to get a billing group
2. POST /create-custom-line-item with no BillingPeriodRange, Name="item1", ChargeDetails={Type:FEE, Flat:{ChargeValue:10}}
3. POST /list-custom-line-items with {"BillingPeriod":"<a period before now>"} and note whether the item is reported
4. POST /update-custom-line-item with {"Arn":"<item>","Name":"item2","BillingPeriodRange":{"InclusiveStartBillingPeriod":"<a future period>"}}
5. POST /list-custom-line-items with {"BillingPeriod":"<the current period>"} -> the item is absent entirely, where the real service still reports the pre-update version for every period before the named start
