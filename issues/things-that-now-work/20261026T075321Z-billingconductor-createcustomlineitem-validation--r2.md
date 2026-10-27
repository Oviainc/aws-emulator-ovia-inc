Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 653ea257673c095df18531588ef6bfc9ad1ec3b6

# CreateCustomLineItem accepts a BillingPeriodRange outside the current or previous billing period

_Filed: 2026-10-26T07:53:21Z_
_Operation: CreateCustomLineItem | Class: validation_

> _Note: this issue was identified against the code base at commit `653ea257673c095df18531588ef6bfc9ad1ec3b6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateCustomLineItem` is documented as creating a charge "that can be applied to a single billing group **for the current or previous billing period**". The billing periods a create may put an item into are therefore bounded to that two-month window: a `BillingPeriodRange` whose `InclusiveStartBillingPeriod` names a month whose bill is already closed, or a month that has not begun, is outside what the operation may act in.

The handler checks only that the range is ORDERED (`billing_period_range_is_ordered`) and then stores it verbatim as the item's effective span. It never calls `crate::state::check_billing_period_range_is_modifiable`, the helper this crate already has for exactly this rule and whose own doc comment says it exists "so every operation bounded to that window answers an out-of-window range the same way". `DeleteCustomLineItem` — whose model doc carries the same sentence ("Deletes the custom line item identified by the given ARN in the current, or previous billing period") — does call it and refuses an out-of-window range with the modelled 400 `ValidationException` (`ILLEGAL_BILLING_PERIOD` / `INVALID_BILLING_PERIOD_FOR_OPERATION`).

So the crate is inconsistent with itself: a create may put an item into 1970 or 2999, but the delete that would take it out of those periods refuses the same range. (This is the create counterpart of the already-open `UpdateCustomLineItem` finding `20261024T164637Z-billingconductor-updatecustomlineitem-validation`; that issue is scoped to the update path only and names only `src/ops/update_custom_line_item.rs`.)

Expected: a `BillingPeriodRange` whose start is neither the current nor the previous billing period is refused with the modelled 400 `ValidationException` carrying the reason the model enumerates for it, and nothing is minted.
Actual: HTTP 200, the item is persisted with that span, and `ListCustomLineItems` reports it for those out-of-window periods.

## Evidence

Smithy (kind A) — `smithy_query(query="doc",shape="com.amazonaws.billingconductor#CreateCustomLineItem")`: "Creates a custom line item that can be used to create a one-time fixed charge that can be applied to a single billing group **for the current or previous billing period**. The one-time fixed charge is either a fee or discount."

Handler that skips the rule — `src/ops/create_custom_line_item.rs:276-289`:
```
// The range the item is effective in: the start is inclusive and the end
// exclusive, so the end has to be a LATER billing period than the start.
let period_range = input.billing_period_range.clone();
if let Some(range) = &period_range {
    if !crate::state::billing_period_range_is_ordered(range) {
        return Err(validation(
            "ILLEGAL_BILLING_PERIOD_RANGE",
            "BillingPeriodRange.ExclusiveEndBillingPeriod", ...
```
and `src/ops/create_custom_line_item.rs:387-392` stores it unchecked: `spans: vec![crate::state::BillingPeriodSpan::new(period_range.as_ref().map(|range| range.inclusive_start_billing_period.clone()), period_range.and_then(|range| range.exclusive_end_billing_period))],`.

Sibling that applies the rule (kind B) — `src/ops/delete_custom_line_item.rs:203-212`:
```
// "Deletes the custom line item … in the current, or previous billing
// period": the periods this operation may take effect in are bounded
// to that window, so a range starting outside it — an already closed
// month or one that has not begun — is refused and the stored item is
// left exactly as it was.
if let Err((reason, field, message)) =
    crate::state::check_billing_period_range_is_modifiable(range)
```

The shared helper's own doc — `src/state.rs:3355-3371`: "May a request carrying `range` be applied by an operation the model documents as acting in the current, or previous, billing period — or is that the refusal it earns? … `Err` carries the `(reason, member, message)` of the modelled `ValidationException` the calling operation refuses the request with, **so every operation bounded to that window answers an out-of-window range the same way**." `grep` shows exactly one caller: `src/ops/delete_custom_line_item.rs:209`.

Test coverage: `tests/all/delete_custom_line_item.rs:791-797` pins the rule for the delete path ("THE RULE on the billing period a delete may act in … is refused with the modelled 400 `ValidationException` and the reason the model enumerates for a period this operation may not act in") and asserts `INVALID_BILLING_PERIOD_FOR_OPERATION` at line 899. No test covers the create path: `tests/all/list_custom_line_items.rs:664-671` instead creates items with `{"InclusiveStartBillingPeriod": "1970-1", "ExclusiveEndBillingPeriod": "1971-1"}` and `{"InclusiveStartBillingPeriod": "2999-1"}` and expects them to be minted, so fixing this will require those two fixtures to move inside the window (as the open UpdateCustomLineItem issue already notes for the same file).

## Steps to Reproduce

1. Make the emulated account the payer account so the control plane is reachable (`state.payer_account_id`).
2. POST /create-pricing-plan, then POST /create-billing-group naming that plan — note the group ARN.
3. POST /create-custom-line-item with {"BillingGroupArn": <group>, "Name": "longover", "Description": "d", "ChargeDetails": {"Type": "FEE", "Flat": {"ChargeValue": 1}}, "BillingPeriodRange": {"InclusiveStartBillingPeriod": "1970-1", "ExclusiveEndBillingPeriod": "1971-1"}}.
4. Observe HTTP 200 and a minted ARN, instead of a 400 ValidationException with Reason ILLEGAL_BILLING_PERIOD / INVALID_BILLING_PERIOD_FOR_OPERATION.
5. POST /list-custom-line-items with {"BillingPeriod": "1970-06"}: the item is reported for a closed bill, and it is reported for no current period at all.
6. POST /delete-custom-line-item with {"Arn": <item>, "BillingPeriodRange": {"InclusiveStartBillingPeriod": "1970-1"}}: the delete REFUSES the same window with INVALID_BILLING_PERIOD_FOR_OPERATION, so the period the create accepted cannot be undone through the range path.
