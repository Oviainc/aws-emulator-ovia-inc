Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 2a22026c0f737b6187455fb636429eae77d015a7

# UpdateCustomLineItem accepts a BillingPeriodRange outside the current or previous billing period

_Filed: 2026-10-24T16:46:37Z_
_Operation: UpdateCustomLineItem | Class: validation_

> _Note: this issue was identified against the code base at commit `2a22026c0f737b6187455fb636429eae77d015a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateCustomLineItem` is documented as "Update an existing custom line item in the current or previous billing period." The billing periods a custom line item may be re-stated for are therefore bounded: an update whose `BillingPeriodRange` names a closed historical month, or a month in the future, is outside what the operation may change.

The handler validates only that the range is ordered (`ExclusiveEndBillingPeriod` strictly later than `InclusiveStartBillingPeriod`) and then writes it onto the stored record, so an update can move an item into 1970 or into 2099.

Expected: a `BillingPeriodRange` that is not within the current or previous billing period is refused with the modelled 400 `ValidationException` carrying one of the reasons the model enumerates for a bad period (`ILLEGAL_BILLING_PERIOD` / `INVALID_BILLING_PERIOD_FOR_OPERATION`), and the stored item is left untouched.
Actual: HTTP 200, the item's effective range becomes the out-of-window range, and `ListCustomLineItems` reports the item for those periods.

## Evidence

Spec: `UpdateCustomLineItem` operation documentation (smithy_query doc, shape="UpdateCustomLineItem"): "Update an existing custom line item in the current or previous billing period." `ValidationExceptionReason` enumerates `ILLEGAL_BILLING_PERIOD` and `INVALID_BILLING_PERIOD_FOR_OPERATION`.

Handler: `src/ops/update_custom_line_item.rs`, `merge()` lines 303-315 — the only check on the range is `crate::state::billing_period_range_is_ordered(range)`; the handler then assigns `updated.inclusive_start_billing_period` / `updated.exclusive_end_billing_period` with no comparison against `crate::state::current_billing_period()` (which the crate already provides and `ListCustomLineItems` already uses).

Tests: `tests/all/update_custom_line_item.rs` lines ~610-705 exercise ordered and unordered ranges (asserting the `BillingPeriodRange.ExclusiveEndBillingPeriod` refusal) but never an out-of-window one; `tests/all/list_custom_line_items.rs` line 647 even uses a 1970 range as an accepted value.

## Steps to Reproduce

1. Create a billing group and a custom line item in it (no BillingPeriodRange, so it applies to every period).
2. Call UpdateCustomLineItem with Arn = the item and BillingPeriodRange = {"InclusiveStartBillingPeriod": "1970-1", "ExclusiveEndBillingPeriod": "1971-1"} — a window far outside the current or previous billing period.
3. Observe HTTP 200 instead of a 400 ValidationException with an ILLEGAL_BILLING_PERIOD / INVALID_BILLING_PERIOD_FOR_OPERATION Reason.
4. Call ListCustomLineItems with BillingPeriod = "1970-06": the item is reported there, and it is no longer reported for the current billing period.
