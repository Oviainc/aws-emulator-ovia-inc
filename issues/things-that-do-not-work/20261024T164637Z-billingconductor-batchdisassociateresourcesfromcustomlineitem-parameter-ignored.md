Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: 2a22026c0f737b6187455fb636429eae77d015a7

# BillingPeriodRange.ExclusiveEndBillingPeriod is validated then dropped, so a bounded disassociation removes the association from every later billing period

_Filed: 2026-10-24T16:46:37Z_
_Operation: BatchDisassociateResourcesFromCustomLineItem | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `2a22026c0f737b6187455fb636429eae77d015a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`BatchDisassociateResourcesFromCustomLineItem` takes a `BillingPeriodRange` (`CustomLineItemBillingPeriodRange`) with a required `InclusiveStartBillingPeriod` and an optional `ExclusiveEndBillingPeriod`. The range names the billing periods the disassociation applies to, so a request naming `{InclusiveStartBillingPeriod: 2026-01, ExclusiveEndBillingPeriod: 2026-03}` must leave the resource associated again from 2026-03 onward.

The handler reads the end bound only to reject an unordered range, then throws it away: `effective_from` is built from `inclusive_start_billing_period` alone, and `apply_disassociation` either drops the association entirely or sets `exclusive_end_billing_period = <start of the disassociation>`. The association is therefore ended at the request's START and never resumes, so every billing period from the start onward loses the association even though the request bounded the change to a half-open window.

Expected: after a bounded disassociation, `ListResourcesAssociatedToCustomLineItem` for a period at or after the request's `ExclusiveEndBillingPeriod` still reports the resource (and `AssociationSize` still counts it for those periods).
Actual: the resource is reported for no period at or after the request's `InclusiveStartBillingPeriod`; the request's end bound has no observable effect at all.

## Evidence

Spec: `CustomLineItemBillingPeriodRange` (smithy_query shape) models `InclusiveStartBillingPeriod` (@required) and `ExclusiveEndBillingPeriod`, documented as "the billing period range in which the custom line item request will be applied"; `BatchDisassociateResourcesFromCustomLineItemInput$BillingPeriodRange` targets it.

Handler: `src/ops/batch_disassociate_resources_from_custom_line_item.rs` lines 305-320 — the range is checked with `crate::state::billing_period_range_is_ordered`, then `let effective_from = input.billing_period_range.as_ref().map(|range| range.inclusive_start_billing_period.clone());` discards the end. `apply_disassociation` (lines 189-224) only ever removes the record or sets `association.exclusive_end_billing_period = Some(from_period)`; it never re-opens the association after the requested end.

Tests: `tests/all/batch_disassociate_resources_from_custom_line_item.rs` — the `RangeRow` table (lines ~680-749) only ever disassociates with `{"InclusiveStartBillingPeriod": ...}`; the only test that sends `ExclusiveEndBillingPeriod` (lines 629-657) asserts the 400 for an unordered range. No test asserts the behaviour of a well-ordered bounded range, so this is uncovered.

This is the disassociation analogue of the already-filed `DeleteCustomLineItem` gap ("A bounded DeleteCustomLineItem range deletes the item from every later billing period too"), and is a different operation and a different stored field (the resource association's range, not the item's).

## Steps to Reproduce

1. Create a billing group, then a percentage custom line item in it, and associate a resource (another billing group ARN) to the item with no billing period range, so the association is open-ended.
2. Call BatchDisassociateResourcesFromCustomLineItem with TargetArn = the item, ResourceArns = [the resource], BillingPeriodRange = {"InclusiveStartBillingPeriod": "2026-01", "ExclusiveEndBillingPeriod": "2026-03"} — it answers 200 with the resource in SuccessfullyDisassociatedResources.
3. Call ListResourcesAssociatedToCustomLineItem with Arn = the item and BillingPeriod = "2026-02": the resource is correctly absent.
4. Call ListResourcesAssociatedToCustomLineItem with Arn = the item and BillingPeriod = "2026-04" (at/after the request's exclusive end): the resource is absent, but the request only disassociated it for 2026-01 and 2026-02, so it should still be reported.
