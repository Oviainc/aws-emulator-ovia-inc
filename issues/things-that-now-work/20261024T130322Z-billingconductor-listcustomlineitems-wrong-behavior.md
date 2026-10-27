Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: f04518a3ee65f2e7815d98c430f437f66966ee67

# ListCustomLineItems reports a custom line item for billing periods that precede its creation

_Filed: 2026-10-24T13:03:22Z_
_Operation: ListCustomLineItems | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f04518a3ee65f2e7815d98c430f437f66966ee67`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: a custom line item created without a `BillingPeriodRange` becomes effective in the billing period it was created in and continues forward; closed (past) billing periods never acquire a line item that did not exist in them. A `ListCustomLineItems` for a billing period earlier than the item's creation period must therefore not report it (its version's `StartBillingPeriod` is the creation period).

Actual: `CreateCustomLineItem` stores `inclusive_start_billing_period = None` when the request names no range, and `CustomLineItemRecord::covers()` treats an absent bound as OPEN, so the item matches EVERY billing period including ones decades in the past. `ListCustomLineItems` with `BillingPeriod=1970-01` reports an item created today, with its current `ChargeDetails`, `AssociationSize` and `Name`. The same open-start treatment also means the values reported for a past period are always the item's CURRENT values rather than the values effective then.

## Evidence

Smithy model: `CustomLineItemVersionListElement$StartBillingPeriod` ("The start billing period of the custom line item version") shows every item version is bounded below by a start period; `CustomLineItemBillingPeriodRange` is "the billing period range in which the custom line item request will be applied", and `ValidationExceptionReason` enumerates `ILLEGAL_BILLING_PERIOD` / `INVALID_BILLING_PERIOD_FOR_OPERATION` for billing periods an operation may not act in — closed periods are not writable, so they cannot gain a new line item. Handler/state: `src/ops/create_custom_line_item.rs` lines 337-341 store `None` for an absent range; `src/state.rs` `CustomLineItemRecord::covers` -> `billing_period_range_covers` (lines 155-177) treats an absent start as open. Test coverage: `tests/all/list_custom_line_items.rs::the_requested_billing_period_selects_the_items_effective_in_it` (lines 626-689) ASSERTS the divergent behaviour — the no-range item "always" is expected in the 1970-01, 1970-12 and 2025-12 pages.

## Steps to Reproduce

1. Authorize as the payer account; create a pricing plan and a billing group
2. POST /create-custom-line-item with Name="always", a Flat ChargeDetails and NO BillingPeriodRange
3. POST /list-custom-line-items with {"BillingPeriod":"1970-01"}
4. Observe the item is reported for a billing period that closed decades before it was created (real service reports an empty list for any period before the item's creation period)
