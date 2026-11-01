Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 043b7f8056dcef57c0c9baaf75abb2dd363a8bd0

# The activeTimeRange filter accepts a range wider than one calendar month, the one bound its modelled doc states

_Filed: 2026-11-01T14:10:54Z_
_Operation: ListBillingViews | Class: validation_

> _Note: this issue was identified against the code base at commit `043b7f8056dcef57c0c9baaf75abb2dd363a8bd0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListBillingViews` validates only that `activeTimeRange.activeAfterInclusive` is not later than `activeBeforeInclusive`. The modelled documentation of that member states a second, explicit rule — the range must be within one calendar month — and the handler never checks it, so a request spanning a year (or a decade) is accepted and answered with a page instead of the modelled `ValidationException` (400).

This is the one documented input rule of this operation that the boundary validator cannot enforce (the bound is a relation between two members, not a per-member constraint), which is exactly the kind of rule the sibling read in this same crate does enforce: `GetCreditAllocationHistory` quotes its own documented range-width sentence ("The range from `startDate` to `endDate` cannot exceed 24 billing months"), turns it into a named constant and refuses past it with `ValidationException`. `ListBillingViews` quotes the equivalent sentence in its module doc and then ignores it.

## Evidence

Smithy (`smithy_query(query="doc", shape="ListBillingViewsRequest", member="activeTimeRange")`): "The time range for the billing views listed. PRIMARY billing view is always listed. BILLING_GROUP billing views are listed for time ranges when the associated billing group resource in Billing Conductor is active. **The time range must be within one calendar month.**"

The same sentence restated in the handler's own module doc — services/service-com-amazonaws-billing-awsbilling/src/ops/list_billing_views.rs:57-60:
```
//! * `activeTimeRange` (ActiveTimeRange) — The time range for the billing views listed.
//!   PRIMARY billing view is always listed. BILLING_GROUP billing views are listed for time
//!   ranges when the associated billing group resource in Billing Conductor is active. The
//!   time range must be within one calendar month.
```
The only check the handler makes — services/service-com-amazonaws-billing-awsbilling/src/ops/list_billing_views.rs:316-325:
```
        if let Some(range) = &input.active_time_range {
            if crate::state::date_time_secs(&range.active_after_inclusive)
                > crate::state::date_time_secs(&range.active_before_inclusive)
            {
                return Err(invalid_field(
                    "activeTimeRange",
                    "activeAfterInclusive must not be later than activeBeforeInclusive",
                ));
```
The sibling that enforces its documented range-width rule — services/service-com-amazonaws-billing-awsbilling/src/ops/get_credit_allocation_history.rs:162-172 and :333-345:
```
/// From the `startDate` doc: "The range from `startDate` to `endDate` cannot
/// exceed 24 billing months." ...
const MAX_BILLING_MONTHS_SPANNED: i32 = 24;
...
        if end_index.checked_sub(start_index).is_none_or(|spanned| spanned > MAX_BILLING_MONTHS_SPANNED) {
            return Err(invalid_field("endDate", "must be within 24 billing months of startDate"));
```
Test coverage: the only `activeTimeRange` rows in the suite are a 1-second range (tests/all/billing_views.rs:487-490) and an inverted range (tests/all/billing_views.rs:929-931, expected 400). No test pins a wider-than-a-month range as acceptable, and no comment in the crate justifies skipping the bound.

## Steps to Reproduce

1. Boot the emulator and point an awsJson1_0 `billing` client at it.
2. Call ListBillingViews with {"activeTimeRange":{"activeAfterInclusive":1704067200,"activeBeforeInclusive":1735689600}} (1 Jan 2024 → 1 Jan 2025, twelve months wide).
3. Observe HTTP 200 with a page of billing views.
4. Expected per the modelled `activeTimeRange` doc ("The time range must be within one calendar month"): 400 ValidationException naming `activeTimeRange`, the way GetCreditAllocationHistory refuses a range wider than its own documented bound.
