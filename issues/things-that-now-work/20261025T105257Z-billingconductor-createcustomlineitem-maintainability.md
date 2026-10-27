Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 9ff0151e93167cf00d59909edc96bc1d4e3529be

# Collapse the five copies of the ordered-BillingPeriodRange refusal into one shared check in state.rs

_Filed: 2026-10-25T10:52:57Z_
_Operation: CreateCustomLineItem | Class: maintainability_

> _Note: this issue was identified against the code base at commit `9ff0151e93167cf00d59909edc96bc1d4e3529be`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule "a `CustomLineItemBillingPeriodRange` whose exclusive end is at or before its inclusive start is refused with `ValidationException` reason `ILLEGAL_BILLING_PERIOD_RANGE`, stated at member `BillingPeriodRange.ExclusiveEndBillingPeriod`" is implemented FIVE separate times in this crate's own handlers. The predicate itself is already shared (`crate::state::billing_period_range_is_ordered`), but the refusal LOGIC around it — the branch, the choice of `ValidationExceptionReason`, the member path that goes into `ValidationException.Fields`, and the sentence — is copied verbatim into each of the five handlers that take that shape. Each copy can drift independently: changing the reason or the member path in one handler leaves the other four answering differently for the same fault, which is exactly the drift this crate avoids everywhere else by putting a shared refusal in `src/state.rs` and handing the calling operation a `ValidationRefusal` triple.

The crate already has the pattern for precisely this member: `crate::state::check_billing_period_range_is_modifiable` takes the same `&CustomLineItemBillingPeriodRange`, returns `Result<(), ValidationRefusal>`, and its own doc states the reason it lives in `state.rs` — "so every operation bounded to that window answers an out-of-window range the same way". `DeleteCustomLineItem` consumes it with a two-line `if let Err((reason, field, message)) = ... { return Err(validation(reason, field, message)) }`. `crate::state::check_tag_keys_not_reserved`, `crate::state::accounts_already_associated` and `crate::state::too_many_auto_associate_billing_groups` are the same convention for other shared refusals.

Fix: add a sibling `check_billing_period_range_is_ordered(range) -> Result<(), ValidationRefusal>` next to `check_billing_period_range_is_modifiable` in `src/state.rs`, carrying the same reason, member and message the five copies carry today, and have the five handlers consume it the way `DeleteCustomLineItem` already consumes the modifiable-window check. Every response stays byte-identical (same status, same `__type`, same `Message`, same `Reason`, same `Fields[0].Name`), so every existing test keeps passing unchanged. The two OTHER range refusals in this crate — `GetBillingGroupCostReport` (which refuses through `billing_period_range_length` and also states a 12-period bound at member `BillingPeriodRange`) and `ListCustomLineItemVersions` (whose filter bounds are both inclusive and whose member path is `Filters.BillingPeriodRange.EndBillingPeriod`) — are a different rule on a different shape and are deliberately NOT in scope.

## Evidence

The crate's stated convention for a shared refusal, `services/service-com-amazonaws-billingconductor-awsbillingconductor/src/state.rs:2517-2522`:

```
/// `Err` carries the `(reason, member, message)` of the modelled `ValidationException`
/// the calling operation refuses the request with, so every operation bounded to that
/// window answers an out-of-window range the same way
pub fn check_billing_period_range_is_modifiable(
    range: &crate::ops::types::CustomLineItemBillingPeriodRange,
) -> Result<(), ValidationRefusal> {
```

and its single consumer, `src/ops/delete_custom_line_item.rs:208-212`:

```
                if let Err((reason, field, message)) =
                    crate::state::check_billing_period_range_is_modifiable(range)
                {
                    return Err(validation(reason, field, message));
                }
```

The FIVE copies of the ordered-range refusal, all naming the same reason, the same member path and the same sentence:

1. `src/ops/create_custom_line_item.rs:262-272`
```
        if let Some(range) = &period_range {
            if !crate::state::billing_period_range_is_ordered(range) {
                return Err(validation(
                    "ILLEGAL_BILLING_PERIOD_RANGE",
                    "BillingPeriodRange.ExclusiveEndBillingPeriod",
                    "the exclusive end billing period must be later than the inclusive \
                     start billing period"
                        .to_string(),
                ));
            }
        }
```

2. `src/ops/update_custom_line_item.rs:310-319`
```
    if let Some(range) = input.billing_period_range.as_ref() {
        if !crate::state::billing_period_range_is_ordered(range) {
            return Err(validation(
                "ILLEGAL_BILLING_PERIOD_RANGE",
                "BillingPeriodRange.ExclusiveEndBillingPeriod",
                "the exclusive end billing period must be later than the inclusive start \
                 billing period"
                    .to_string(),
            ));
        }
```

3. `src/ops/delete_custom_line_item.rs:194-202` — in the SAME handler, immediately above the shared `check_billing_period_range_is_modifiable` call quoted above
```
                if !crate::state::billing_period_range_is_ordered(range) {
                    return Err(validation(
                        "ILLEGAL_BILLING_PERIOD_RANGE",
                        "BillingPeriodRange.ExclusiveEndBillingPeriod",
                        "the exclusive end billing period must be later than the inclusive \
                         start billing period"
                            .to_string(),
                    ));
                }
```

4. `src/ops/batch_associate_resources_to_custom_line_item.rs:253-263`
```
        if let Some(range) = &input.billing_period_range {
            if !crate::state::billing_period_range_is_ordered(range) {
                return Err(validation(
                    "ILLEGAL_BILLING_PERIOD_RANGE",
                    "BillingPeriodRange.ExclusiveEndBillingPeriod",
```

5. `src/ops/batch_disassociate_resources_from_custom_line_item.rs:289-299`
```
        if let Some(range) = &input.billing_period_range {
            if !crate::state::billing_period_range_is_ordered(range) {
                return Err(validation(
                    "ILLEGAL_BILLING_PERIOD_RANGE",
                    "BillingPeriodRange.ExclusiveEndBillingPeriod",
```

The same crate already shows this collapse is the house style for a refusal several operations owe — `src/state.rs:2580` `accounts_already_associated(...)` ("built here so every operation that takes account ids answers it the same way"), `check_tag_keys_not_reserved(...)` at `src/state.rs:2348`, and `too_many_auto_associate_billing_groups(...)`.

Test coverage that must stay green and unchanged (each asserts HTTP 400, `__type` `ValidationException`, `Reason` `ILLEGAL_BILLING_PERIOD_RANGE` and `Fields[0].Name` `BillingPeriodRange.ExclusiveEndBillingPeriod`, plus "a refused call changes nothing"):
- `tests/all/update_custom_line_item.rs:672-709` (the `unordered` table: equal bounds, equal bounds spelled differently, reversed, reversed across a year) — assertion at line 701
- `tests/all/delete_custom_line_item.rs:780` (and the suite header at lines 20-23)
- `tests/all/batch_disassociate_resources_from_custom_line_item.rs:680`
- `tests/all/percentage_custom_line_item_resource_associations.rs:616` (covers CreateCustomLineItem and BatchAssociateResourcesToCustomLineItem)
Out of scope and untouched: `tests/all/list_custom_line_item_versions.rs:959` and `tests/all/billing_group_cost_report.rs:903`, which pin the two different range rules named above.

## Steps to Reproduce

1. grep -n 'ILLEGAL_BILLING_PERIOD_RANGE' services/service-com-amazonaws-billingconductor-awsbillingconductor/src/ops/*.rs
2. Observe five handlers (create_custom_line_item.rs, update_custom_line_item.rs, delete_custom_line_item.rs, batch_associate_resources_to_custom_line_item.rs, batch_disassociate_resources_from_custom_line_item.rs) each carrying their own copy of the branch, the reason, the member path 'BillingPeriodRange.ExclusiveEndBillingPeriod' and the identical message text
3. Open services/service-com-amazonaws-billingconductor-awsbillingconductor/src/state.rs:2504-2544 and see that the sibling rule on the SAME shape (check_billing_period_range_is_modifiable) is instead a single shared function returning ValidationRefusal, documented as living there 'so every operation ... answers ... the same way'
4. Open src/ops/delete_custom_line_item.rs:194-212 and see both forms side by side in one handler: the copied ordered-range branch at 194-202 and the shared-helper call at 208-212
5. Change the member path in only one of the five copies (e.g. to 'BillingPeriodRange') and re-run `cargo test -p service-com-amazonaws-billingconductor-awsbillingconductor`: only that one operation's suite fails, demonstrating the five copies are free to drift from each other
