Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: f498d25066c291e3faedd02ae2bda62a819c5304

# The two batch association operations skip the current-or-previous-billing-period window the crate's three other writers of the same BillingPeriodRange shape enforce, so a bounded batch call rewrites a closed billing period

_Filed: 2026-10-29T16:33:36Z_
_Operation: BatchDisassociateResourcesFromCustomLineItem | Class: validation_

> _Note: this issue was identified against the code base at commit `f498d25066c291e3faedd02ae2bda62a819c5304`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Five operations of this crate take the same `CustomLineItemBillingPeriodRange` shape and write the billing periods it names. Three of them (`CreateCustomLineItem`, `UpdateCustomLineItem`, `DeleteCustomLineItem`) run BOTH shared checks on that range: `check_billing_period_range_is_ordered` and then `check_billing_period_range_is_modifiable`, which refuses a range whose `InclusiveStartBillingPeriod` is neither the current nor the previous billing period (`INVALID_BILLING_PERIOD_FOR_OPERATION`) and leaves the stored record untouched.

The two batch association handlers run only the ORDERED check and skip the modifiable one. The consequence is a write into a billing period whose bill is already closed:

* `BatchDisassociateResourcesFromCustomLineItem` with `BillingPeriodRange = { InclusiveStartBillingPeriod: "<a month long past>", ExclusiveEndBillingPeriod: "<the month after it>" }` is answered 200 and calls `ResourceAssociationRecord::remove_periods` for that old span, so the resource stops being reported by `ListResourcesAssociatedToCustomLineItem` for a closed month it really was associated in, and `proforma_custom_line_item_charges` — which decides a percentage item's base from `association.covers(billing_period)` — recomputes that closed month's `ProformaCost`/`Margin`. `DeleteCustomLineItem`, the sibling bounded write on the same resource family, refuses exactly that request with the comment "an already closed month ... is refused and the stored item is left exactly as it was".
* `BatchAssociateResourcesToCustomLineItem` is the symmetric half of the same one-line omission: with an old `InclusiveStartBillingPeriod` it stores an association in effect from that closed month on, which contradicts the rule `BillingPeriodSpan::started_now` states for every record of this store — "a closed bill never acquires a resource that was not in it". (`BatchAssociateResourcesToCustomLineItem` already carries a filed `validation` issue, so the fix for both halves is tracked here: the missing call is one line in each handler, right after the ordered check each already makes.)

The crate also states the invariant the divergence breaks: its own suite asserts that every operation taking a `CustomLineItemBillingPeriodRange` answers ONE range identically, "which is what deciding it in one place guarantees and copies do not" — yet for an ordered but out-of-window range three of the five answer 400 and these two answer 200. Nothing in the crate justifies the divergence: no comment, constant or test says the batch operations are deliberately unbounded, and `check_billing_period_range_is_modifiable`'s own doc speaks of "every operation bounded to that window" without naming which.

## Evidence

Side 1 — the batch handler runs ONLY the ordered check, `services/service-com-amazonaws-billingconductor-awsbillingconductor/src/ops/batch_disassociate_resources_from_custom_line_item.rs:284-294`:
```
        // The billing periods the disassociation applies to: the start is inclusive and
        // the end exclusive, so an end at or before the start would cover no billing
        // period. A request that names no range disassociates the resource in every
        // billing period, which is the fully open span.
        if let Some(range) = &input.billing_period_range {
            if let Err((reason, field, message)) =
                crate::state::check_billing_period_range_is_ordered(range)
            {
                return Err(validation(reason, field, message));
            }
        }
```
and the same omission in the associate half, `src/ops/batch_associate_resources_to_custom_line_item.rs:256-264`:
```
        // The range the association applies in: the start is inclusive and the end
        // exclusive, so an end at or before the start would cover no billing period.
        if let Some(range) = &input.billing_period_range {
            if let Err((reason, field, message)) =
                crate::state::check_billing_period_range_is_ordered(range)
            {
                return Err(validation(reason, field, message));
            }
        }
```

Side 2 — the three sibling writers of the SAME shape run both checks and say why. `src/ops/delete_custom_line_item.rs:199-208`:
```
                // "Deletes the custom line item … in the current, or previous billing
                // period": the periods this operation may take effect in are bounded
                // to that window, so a range starting outside it — an already closed
                // month or one that has not begun — is refused and the stored item is
                // left exactly as it was.
                if let Err((reason, field, message)) =
                    crate::state::check_billing_period_range_is_modifiable(range)
                {
                    return Err(validation(reason, field, message));
                }
```
`src/ops/create_custom_line_item.rs:289-298`:
```
            // "Creates a custom line item … for the current or previous billing
            // period": the periods this operation may put an item into are bounded to
            // that window, so a range starting outside it — an already closed month or
            // one that has not begun — is refused and no item is minted (the same
            // refusal `DeleteCustomLineItem` answers such a range with).
            if let Err((reason, field, message)) =
                crate::state::check_billing_period_range_is_modifiable(range)
```
`src/ops/update_custom_line_item.rs:331-342`:
```
        // "Update an existing custom line item in the current or previous billing
        // period": the periods this operation may re-state the item for are bounded to
        // that window, so a range starting outside it — a month whose bill is already
        // closed, or one that has not begun — is refused and the stored item is left
        // exactly as it was (the same refusal `CreateCustomLineItem` and
        // `DeleteCustomLineItem`, both bounded to the same window, answer such a range
        // with).
        if let Err((reason, field, message)) =
            crate::state::check_billing_period_range_is_modifiable(range)
        {
            return Err(validation(reason, field, message));
        }
```

The shared check already exists and already names its refusal, `src/state.rs:4234-4237` and `4269-4272`:
```
const MODIFIABLE_BILLING_PERIODS_BACK: i64 = 1;
...
/// `Err` carries the `(reason, member, message)` of the modelled `ValidationException`
/// the calling operation refuses the request with, so every operation bounded to that
/// window answers an out-of-window range the same way.
pub fn check_billing_period_range_is_modifiable(
```

The rule the associate half breaks, `src/state.rs:281-285`:
```
    /// A resource is in effect no earlier than the billing period the call that minted it
    /// was made in, because it did not exist in any period before that one and a closed
    /// bill never acquires a resource that was not in it.
```

The invariant the crate's own suite asserts over exactly these five operations, `tests/all/ordered_billing_period_range_refusal.rs:148-157`:
```
        // One rule, one refusal: the operations differ in nothing but the operation
        // asked, which is what deciding it in one place guarantees and copies do not.
        let (first_operation, first_status, first_body) = &answers[0];
        for (operation, status, body) in &answers[1..] {
            assert_eq!(
                (status, body),
                (first_status, first_body),
```
(the five it drives are listed at `tests/all/ordered_billing_period_range_refusal.rs:78,87,100,113,122`: `CreateCustomLineItem`, `UpdateCustomLineItem`, `BatchAssociateResourcesToCustomLineItem`, `BatchDisassociateResourcesFromCustomLineItem`, `DeleteCustomLineItem`).

Test coverage: the window is pinned for the three bounded writers only — `tests/all/create_custom_line_item_billing_period_window.rs:416` and `tests/all/update_custom_line_item_billing_period_window.rs:447` (both asserting `"INVALID_BILLING_PERIOD_FOR_OPERATION"`) and `tests/all/delete_custom_line_item.rs:910`. No test sends an OUT-OF-WINDOW range to either batch operation: the ordered-range suites deliberately keep their accepted starts inside the window — `tests/all/shared_ordered_billing_period_range_check.rs:167-170` ("The starts are the current billing period, the one the operations bounded to 'the current, or previous billing period' may also act in, so the only thing deciding these answers is the range") and `tests/all/ordered_billing_period_range_refusal.rs:28-30`. Adding the missing check therefore leaves every existing assertion unchanged.

## Steps to Reproduce

1. Designate the emulated account as the payer so the control plane is reachable (set `ComAmazonawsBillingconductorAwsbillingconductorState::payer_account_id`).
2. POST /create-pricing-plan, then POST /create-billing-group (keep the group ARN G).
3. POST /create-custom-line-item in G with `ChargeDetails.Percentage` and no `BillingPeriodRange` (keep the item ARN T).
4. PUT /batch-associate-resources-to-custom-line-item with {"TargetArn": T, "ResourceArns": [G], "BillingPeriodRange": {"InclusiveStartBillingPeriod": "<a month several billing periods in the past>"}} — answered 200 with G in `SuccessfullyAssociatedResources`, although the same range sent to POST /create-custom-line-item, POST /update-custom-line-item or POST /delete-custom-line-item is answered 400 `ValidationException` with `Reason = INVALID_BILLING_PERIOD_FOR_OPERATION`.
5. POST /list-resources-associated-to-custom-line-item with {"Arn": T, "BillingPeriod": "<that past month>"} — G is reported, i.e. a closed bill acquired a resource that was not in it.
6. PUT /batch-disassociate-resources-from-custom-line-item with {"TargetArn": T, "ResourceArns": [G], "BillingPeriodRange": {"InclusiveStartBillingPeriod": "<that past month>", "ExclusiveEndBillingPeriod": "<the month after it>"}} — answered 200, and the read in step 5 now reports nothing for that month: the closed period's association record, and the `ProformaCost`/`Margin` that POST /get-billing-group-cost-report computes for it, both changed.
7. Contrast: POST /delete-custom-line-item with the same `BillingPeriodRange` is refused 400 `INVALID_BILLING_PERIOD_FOR_OPERATION` and writes nothing, which is the answer the crate's `tests/all/ordered_billing_period_range_refusal.rs` says all five operations taking this shape must share for one range.
