Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: c441cdb97fe099c25286b36cfc473da57b1ed5b6

# Fold GetBillingGroupCostReport's hand-rolled empty-range refusal into the crate's one ordered-BillingPeriodRange check

_Filed: 2026-10-29T00:02:13Z_
_Operation: GetBillingGroupCostReport | Class: maintainability_

> _Note: this issue was identified against the code base at commit `c441cdb97fe099c25286b36cfc473da57b1ed5b6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate has exactly one place that decides whether a half-open billing period range covers any period and exactly one refusal for the case it does not: `state::billing_period_range_is_ordered` states the rule from the member names and `state::check_billing_period_range_is_ordered` returns the `(reason, member, message)` triple, so (quoting its own doc) "every operation taking that shape answers an empty range the same way". Five handlers decide it through that helper: `CreateCustomLineItem`, `UpdateCustomLineItem`, `DeleteCustomLineItem`, `BatchAssociateResourcesToCustomLineItem` and `BatchDisassociateResourcesFromCustomLineItem`.

`GetBillingGroupCostReport` implements the same rule a SECOND time, inline: it computes the range length itself and refuses `covered <= 0` with the identical `ValidationExceptionReason` (`ILLEGAL_BILLING_PERIOD_RANGE`), the identical `Fields` member (`BillingPeriodRange.ExclusiveEndBillingPeriod`) and the identical sentence, character for character, as the shared check. Two copies of one rule and one refusal can drift: a change to the wording, the member named or the ordering rule in `state.rs` silently leaves this operation answering the old way, and only the five helper callers are covered by the suite that drives the rule.

The only reason the copy exists is a type: the helper takes `CustomLineItemBillingPeriodRange`, while this operation's member is `BillingPeriodRange` (same two members, both required here). Giving the shared check a bounds-taking form (`&str`, `Option<&str>`) and keeping the existing shape wrapper for the five callers leaves every response byte-identical — the reason, the member and the message are already the same — and leaves every existing test unchanged.

## Evidence

The one shared rule and refusal — `src/state.rs:4121-4139` and `:4141-4164`:
```
/// Is a `CustomLineItemBillingPeriodRange` ordered, i.e. does it describe a non-empty
/// span of billing periods?
///
/// The member names carry the rule: `InclusiveStartBillingPeriod` is in the range and
/// `ExclusiveEndBillingPeriod` is not, so an end equal to or earlier than the start
/// would cover no billing period at all.
pub fn billing_period_range_is_ordered(
...
/// `Err` carries the `(reason, member, message)` of the modelled `ValidationException`
/// the calling operation refuses the request with, so every operation taking that shape
/// answers an empty range the same way.
pub fn check_billing_period_range_is_ordered(
    range: &crate::ops::types::CustomLineItemBillingPeriodRange,
) -> Result<(), ValidationRefusal> {
    if billing_period_range_is_ordered(range) {
        return Ok(());
    }
    Err((
        "ILLEGAL_BILLING_PERIOD_RANGE",
        "BillingPeriodRange.ExclusiveEndBillingPeriod",
        "the exclusive end billing period must be later than the inclusive start billing \
         period"
            .to_string(),
    ))
}
```

The second copy of the same rule and the same refusal — `src/ops/get_billing_group_cost_report.rs:233-245`:
```
                let start = range.inclusive_start_billing_period.as_str();
                let end = range.exclusive_end_billing_period.as_str();
                let covered =
                    crate::state::billing_period_range_length(start, end).unwrap_or_default();
                if covered <= 0 {
                    return Err(validation(
                        "ILLEGAL_BILLING_PERIOD_RANGE",
                        "BillingPeriodRange.ExclusiveEndBillingPeriod",
                        "the exclusive end billing period must be later than the inclusive \
                         start billing period"
                            .to_string(),
                    ));
                }
```

The five callers that do go through the helper — `src/ops/create_custom_line_item.rs:287`, `src/ops/update_custom_line_item.rs:327`, `src/ops/delete_custom_line_item.rs:195`, `src/ops/batch_associate_resources_to_custom_line_item.rs:261`, `src/ops/batch_disassociate_resources_from_custom_line_item.rs:291` (each `crate::state::check_billing_period_range_is_ordered(range)`).

Test coverage that pins the shared answer, and names only those five — `tests/all/ordered_billing_period_range_refusal.rs:3-15`:
```
//! The ONE ordered-`BillingPeriodRange` refusal, driven through every operation that
//! takes a `CustomLineItemBillingPeriodRange`: `CreateCustomLineItem`,
//! `UpdateCustomLineItem`, `DeleteCustomLineItem`,
//! `BatchAssociateResourcesToCustomLineItem` and
//! `BatchDisassociateResourcesFromCustomLineItem`.
...
//! `BillingPeriodRange.ExclusiveEndBillingPeriod` — the SAME answer whichever of those
//! operations was called, because they all decide it through the one shared check.
```
The copy's own coverage is `tests/all/billing_group_cost_report.rs:876-890` (the empty-range row for this operation), which asserts the same reason, member and message — so routing it through the helper changes no assertion.

## Steps to Reproduce

1. Read `src/state.rs:4141-4164`: the single `check_billing_period_range_is_ordered`, whose doc says every operation taking that shape answers an empty range the same way.
2. Read `src/ops/get_billing_group_cost_report.rs:233-245`: the same rule re-derived from `billing_period_range_length(...) <= 0` and the same refusal re-spelled with the same reason, member and sentence.
3. Note that `grep -n 'check_billing_period_range_is_ordered' src/ops/*.rs` lists five handlers and not this one, and that `tests/all/ordered_billing_period_range_refusal.rs` drives exactly those five, so the sixth copy is the only one no shared-rule suite covers.
4. Change the shared message in `state.rs` and observe that `GetBillingGroupCostReport` keeps answering the old sentence while the other five change — the drift the duplication allows.
