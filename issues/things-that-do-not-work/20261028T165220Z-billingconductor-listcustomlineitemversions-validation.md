Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 62bd53a2191a4112059965729a42c5aa0aaceb39

# The versions filter reads its documented EXCLUSIVE end bound as inclusive, so an empty range is served and versions outside the range are reported

_Filed: 2026-10-28T16:52:20Z_
_Operation: ListCustomLineItemVersions | Class: validation_

> _Note: this issue was identified against the code base at commit `62bd53a2191a4112059965729a42c5aa0aaceb39`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListCustomLineItemVersions` applies `Filters.BillingPeriodRange` with BOTH bounds inclusive, on the explicit premise that the model does not say which they are. The model says exactly which they are: `StartBillingPeriod` is documented "The inclusive start billing period ..." and `EndBillingPeriod` is documented "The **exclusive** end billing period that defines a billing period range where a custom line item version is applied."

Two consequences of reading the exclusive end as inclusive:

1. VALIDATION: a filter whose `EndBillingPeriod` EQUALS its `StartBillingPeriod` names no billing period at all under the documented half-open reading, yet the handler serves it and reports the generation in effect at that month. The handler only refuses `start > end`. Everywhere else in this crate a billing-period range whose end is at or before its start is refused with `ILLEGAL_BILLING_PERIOD_RANGE` through the one shared rule `crate::state::billing_period_range_is_ordered` (`CreateCustomLineItem`, `UpdateCustomLineItem`, `DeleteCustomLineItem`, both batch association operations) — so this one filter is the single place the crate accepts an empty billing-period range.

2. SELECTION: `not_after_filter` keeps a generation when `start <= filter_end`, so a generation whose first billing period IS the filter's end bound is reported although it applies in no period of the documented range `[StartBillingPeriod, EndBillingPeriod)`. A filter of `{StartBillingPeriod: 2026-01, EndBillingPeriod: 2026-03}` reports a generation that first applies in 2026-03, which is outside the range.

The fix is one comparison (`start < filter_end`) plus extending the existing refusal to `start >= end`, i.e. applying the same half-open rule the rest of the crate already shares.

## Evidence

MODEL (smithy_query query="doc", shape="ListCustomLineItemVersionsBillingPeriodRangeFilter", member="EndBillingPeriod"):
```
"The exclusive end billing period that defines a billing period range where a custom line item version is applied."
```
and member="StartBillingPeriod":
```
"The inclusive start billing period that defines a billing period range where a custom line item version is applied."
```

HANDLER premise, which contradicts that text — `src/ops/list_custom_line_item_versions.rs:151-157`:
```
/// The filter is the span of billing periods the versions "are applied" in, and its
/// member names carry no inclusive/exclusive marker — unlike the half-open
/// `CustomLineItemBillingPeriodRange` a write carries, whose members spell both out —
/// so both of its bounds are read as INCLUSIVE and an absent bound as open.
```

HANDLER code, end bound applied inclusively — `src/ops/list_custom_line_item_versions.rs:168-172`:
```
    let not_after_filter = filter
        .end_billing_period
        .as_deref()
        .and_then(crate::state::billing_period_key)
        .is_none_or(|filter_end| start.is_none_or(|start| start <= filter_end));
```

HANDLER refusal, which therefore only catches `start > end` — `src/ops/list_custom_line_item_versions.rs:299-321`:
```
        // Both bounds of the filter are inclusive (see [`in_filter`]), so a range whose
        // end comes before its start names no billing period at all.
...
            if let (Some(start), Some(end)) = bounds {
                if start > end {
                    return Err(validation(
                        "ILLEGAL_BILLING_PERIOD_RANGE",
                        "Filters.BillingPeriodRange.EndBillingPeriod",
```

THE SAME CRATE's one rule for an exclusive end — `src/state.rs:3966-3972` and `:3989-3991`:
```
/// Is a `CustomLineItemBillingPeriodRange` ordered, i.e. does it describe a non-empty
/// span of billing periods?
///
/// The member names carry the rule: `InclusiveStartBillingPeriod` is in the range and
/// `ExclusiveEndBillingPeriod` is not, so an end equal to or earlier than the start
/// would cover no billing period at all.
...
/// The rule is [`billing_period_range_is_ordered`]: the end bound is exclusive, so a
/// range whose end is at or before its inclusive start covers no billing period, which
/// `ValidationExceptionReason` enumerates `ILLEGAL_BILLING_PERIOD_RANGE` for.
```

TEST COVERAGE: the inclusive reading is currently pinned, so the fix must re-state these rows — `tests/all/list_custom_line_item_versions.rs:744-745`:
```
/// THE RULE on the filter: a billing-period range selects the generations in effect
/// anywhere inside it — both of its bounds inclusive, an absent bound open.
```
the empty range accepted, `:760-770`:
```
            "one period, the first generation's own start",
            json!({ "Filters": { "BillingPeriodRange": {
                "StartBillingPeriod": period(-1), "EndBillingPeriod": period(-1) } } }),
            vec![period(-1)],
```
and the over-selection at the end bound, `:772-777`:
```
            "a span across two generations",
            json!({ "Filters": { "BillingPeriodRange": {
                "StartBillingPeriod": period(-1), "EndBillingPeriod": period(0) } } }),
            generations.clone(),
```
(the second generation's own `StartBillingPeriod` IS `period(0)`, the filter's exclusive end). By contrast the write side of the same rule is pinned the documented way — `tests/all/update_custom_line_item.rs:764-768`:
```
    // A range whose exclusive end is at or before its inclusive start covers no billing
    // period, in either spelling of a month.
    let unordered = [
        ("equal bounds", "2026-05", "2026-05"),
```

## Steps to Reproduce

1. Designate the emulated account as the payer so the control plane is reachable, then POST /create-pricing-plan and POST /create-billing-group.
2. POST /create-custom-line-item with a Flat ChargeDetails and BillingPeriodRange {"InclusiveStartBillingPeriod": <previous billing period>}; keep the item Arn.
3. POST /update-custom-line-item with {"Arn": <item>, "Name": "v2", "BillingPeriodRange": {"InclusiveStartBillingPeriod": <current billing period>}} so the item has two generations: one starting in the previous period and one starting in the current period.
4. POST /list-custom-line-item-versions with {"Arn": <item>, "Filters": {"BillingPeriodRange": {"StartBillingPeriod": <previous period>, "EndBillingPeriod": <current period>}}}. Observe BOTH generations reported, although the documented range [previous, current) contains only the previous period and the second generation applies in no period of it.
5. POST /list-custom-line-item-versions with {"Arn": <item>, "Filters": {"BillingPeriodRange": {"StartBillingPeriod": <previous period>, "EndBillingPeriod": <previous period>}}} — an empty half-open range. Observe HTTP 200 with one version instead of the 400 ValidationException / ILLEGAL_BILLING_PERIOD_RANGE that every other operation of this crate answers an end-at-or-before-start range with.
