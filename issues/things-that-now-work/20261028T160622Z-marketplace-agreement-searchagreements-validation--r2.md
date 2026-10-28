Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 5f1c460e5544105d602e680f733bf81495110ec8

# An inverted BeforeEndTime/AfterEndTime filter pair is answered with an empty page, skipping the crate's own time_window_refusal that both sibling listings apply

_Filed: 2026-10-28T16:06:22Z_
_Operation: SearchAgreements | Class: validation_

> _Note: this issue was identified against the code base at commit `5f1c460e5544105d602e680f733bf81495110ec8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`state::time_window_refusal` is this crate's single owner of the rule "a lower bound at or after the upper bound spans no instant, so the pair states a period rather than selecting on one", and its reason constant's doc declares the rule applies to EVERY such pair this service refuses. Two of the three listings that carry such a pair go through it: `ListBillingAdjustmentRequests` (createdAfter/createdBefore) and `ListAgreementInvoiceLineItems` (afterIssuedTime/beforeIssuedTime).

`SearchAgreements` carries three such pairs as filter names — BeforeEndTime/AfterEndTime, BeforeStartTime/AfterStartTime, BeforeLastUpdateTime/AfterLastUpdateTime — and the model's own note expects callers to send both at once ("To filter by EndTime, you can use BeforeEndTime, AfterEndTime, or both"). `validate_filters` already parses each of those values as an instant and refuses one that is not, but it never compares the pair, and `filter_combination_refusal` explicitly waves the end-time names through. So a search whose window is inverted (AfterEndTime >= BeforeEndTime) is answered HTTP 200 with an empty `agreementViewSummaries` page, while the equivalent inverted window on the two sibling listings is answered HTTP 400 ValidationException.

This is the same condition the crate already refuses elsewhere: no agreement in any store — including a store holding everything — can satisfy the pair, so an empty page is indistinguishable from "nothing matched" and the caller is told nothing about the malformed period. It also contradicts the handler's own stated principle for the sibling `catalog` selector, which is refused "rather than reported as an empty page".

## Evidence

The crate's single owner of the rule, services/service-com-amazonaws-marketplaceagreement-awsmpcommerceservice-v20200301/src/state.rs:3871-3875 and 3878-3907:
```
/// So it is the reason for EVERY pair of bounds this service refuses as a
/// malformed period, whichever instant the pair bounds — the creation of a
/// billing adjustment request and the issuing of an invoice alike: ...
pub const REASON_INVALID_PERIOD: &str = "INVALID_INVOICE_ADJUSTMENT_PERIOD";
...
/// Such a pair is a malformed period rather than a selector
/// that happens to select nothing: no record of any store could satisfy it,
/// including a store holding everything, so the request states a period rather
/// than selecting on one ...
pub fn time_window_refusal(
    after: Option<f64>, before: Option<f64>, after_member: &str, before_member: &str,
) -> Option<ValidationException> {
```

Sibling that applies it — src/ops/list_billing_adjustment_requests.rs:185-198:
```
        if let Some(refusal) = crate::state::time_window_refusal(
            created_after, created_before, "createdAfter", "createdBefore",
        ) {
            return Err(
                ListBillingAdjustmentRequestsErrorModeled::ValidationException(refusal).into(),
            );
```
and src/ops/list_agreement_invoice_line_items.rs:185-194 does the same for afterIssuedTime/beforeIssuedTime.

The path that skips it — src/ops/search_agreements.rs:198-218 validates the filter list and the combination but never the pair:
```
        let filters = input.filters.unwrap_or_default();
        if let Err(refusal) = crate::state::validate_filters(&filters) { ... }
        if let Some(refusal) = crate::state::filter_combination_refusal(&filters) { ... }
```
and state.rs:3182-3191, which checks only that each bound IS an instant:
```
        // Every `Before…`/`After…` filter selects on an instant, so its value
        // has to be one.
        if (name.starts_with("Before") || name.starts_with("After"))
            && filter_instant(value).is_none()
```
with state.rs:3277-3283 removing the end-time names from the combination check entirely (`!END_TIME_FILTER_NAMES.contains(name)`).

Smithy, `smithy_query(query="doc", shape="SearchAgreementsInput", member="filters")`: "To filter by <code>EndTime</code>, you can use <code>BeforeEndTime</code>, <code>AfterEndTime</code>, or both."

Test coverage: tests/all/time_windows.rs:15-18 scopes the pinned class to the two listings whose INPUT MEMBERS carry the pair and names no SearchAgreements row; tests/all/agreement_end_dates.rs:440-447 pins only a well-formed both-bounds window ("both bounds are an AND over one window"). No test or comment pins or justifies the empty-page answer for an inverted search window.

## Steps to Reproduce

1. CreateAgreementRequest + AcceptAgreementRequest for terms that commit a duration, so a stored agreement has an endTime.
2. Call ListBillingAdjustmentRequests with createdAfter = 2030-01-01T00:00:00Z and createdBefore = 2020-01-01T00:00:00Z: HTTP 400 ValidationException, reason INVALID_INVOICE_ADJUSTMENT_PERIOD naming both members.
3. Call SearchAgreements with filters [{name: AgreementType, values: [PurchaseAgreement]}, {name: AfterEndTime, values: [2030-01-01T00:00:00Z]}, {name: BeforeEndTime, values: [2020-01-01T00:00:00Z]}] — the same inverted window.
4. Observe HTTP 200 with an empty agreementViewSummaries list instead of the refusal the crate's time_window_refusal defines for the condition.
