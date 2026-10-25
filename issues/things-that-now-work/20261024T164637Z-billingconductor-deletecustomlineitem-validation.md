Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 2a22026c0f737b6187455fb636429eae77d015a7

# DeleteCustomLineItem accepts a BillingPeriodRange outside the current or previous billing period

_Filed: 2026-10-24T16:46:37Z_
_Operation: DeleteCustomLineItem | Class: validation_

> _Note: this issue was identified against the code base at commit `2a22026c0f737b6187455fb636429eae77d015a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteCustomLineItem` is documented as deleting "the custom line item identified by the given ARN in the current, or previous billing period." The periods a delete may address are therefore bounded to that two-month window; a request naming a closed historical month or a future month asks to change a bill the operation may not touch.

The handler checks only that the supplied range is ordered and then applies the deletion to whatever periods the request named, narrowing or removing the stored item accordingly.

Expected: a `BillingPeriodRange` outside the current or previous billing period is refused with the modelled 400 `ValidationException` carrying one of the reasons the model enumerates for a bad period (`ILLEGAL_BILLING_PERIOD` / `INVALID_BILLING_PERIOD_FOR_OPERATION`), and the stored item is unchanged.
Actual: HTTP 200, and the item's stored effective range is narrowed (or the item removed) for arbitrarily old or future periods.

## Evidence

Spec: `DeleteCustomLineItem` operation documentation (smithy_query doc, shape="DeleteCustomLineItem"): "Deletes the custom line item identified by the given ARN in the current, or previous billing period." `ValidationExceptionReason` enumerates `ILLEGAL_BILLING_PERIOD` and `INVALID_BILLING_PERIOD_FOR_OPERATION`; the operation already answers other faults through `crate::state::validation_exception`.

Handler: `src/ops/delete_custom_line_item.rs` lines 180-200 — the only range check is `crate::state::billing_period_range_is_ordered(range)`, after which `remaining_span()` computes the narrowing for any period the request names. `crate::state::current_billing_period()` is never consulted.

Tests: `tests/all/delete_custom_line_item.rs` — the range helper (lines 225-230) and the table around lines 300-320 exercise in-window ranges, and line 669 asserts the unordered-range refusal; no test asserts a refusal for a range outside the current/previous billing period. (The separate open issue on this operation is about a bounded range deleting later periods too — a different defect from accepting an out-of-window range at all.)

## Steps to Reproduce

1. Create a billing group and a custom line item in it with BillingPeriodRange {"InclusiveStartBillingPeriod": "2026-01"}.
2. Call DeleteCustomLineItem with Arn = the item and BillingPeriodRange = {"InclusiveStartBillingPeriod": "2026-02", "ExclusiveEndBillingPeriod": "2026-03"} where those months are neither the current nor the previous billing period.
3. Observe HTTP 200 with the item ARN instead of a 400 ValidationException with an ILLEGAL_BILLING_PERIOD / INVALID_BILLING_PERIOD_FOR_OPERATION Reason.
4. Call ListCustomLineItems for those months: the item's stored effective range has been changed for billing periods the operation is documented not to be able to modify.
