Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 65fd8797851bb5c5937d1ea0792f5c718c73df47

# The history status/level filter-and-view chain is implemented twice, once per history listing

_Filed: 2026-10-27T22:21:24Z_
_Operation: ListRequestedServiceQuotaChangeHistory | Class: maintainability_

> _Note: this issue was identified against the code base at commit `65fd8797851bb5c5937d1ea0792f5c718c73df47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Both history listings apply the same three-step rule to the requests they select — keep the ones whose stored `Status` equals the requested one, keep the ones whose level passes `requested_at_level`, then render each through `request_view` — and the chain is copy-pasted verbatim into the two handlers rather than living in one place. The crate already extracted the level half into a single shared predicate (`crate::state::requested_at_level`, whose doc says the point is that one rule has one implementation), so the remaining two steps are the only part that can drift between the account-wide listing and the per-quota listing. Collapsing the chain into one helper beside `requests_in_history` / `requests_for_quota` leaves every response byte and every existing test unchanged.

## Evidence

Copy 1 — `src/ops/list_requested_service_quota_change_history.rs:201-217`:
```
            quotas_state
                .requests_in_history(&region, input.service_code.as_deref(), now)
                .into_iter()
                .filter(|request| {
                    input
                        .status
                        .as_deref()
                        .is_none_or(|status| request.status == status)
                })
                .filter(|request| {
                    crate::state::requested_at_level(
                        &request.quota_requested_at_level,
                        input.quota_requested_at_level.as_deref(),
                    )
                })
                .map(|request| quotas_state.request_view(&request))
                .collect::<Vec<_>>()
```
Copy 2 — `src/ops/list_requested_service_quota_change_history_by_quota.rs:144-160`, character for character the same but seeded from `requests_for_quota(&region, &input.service_code, &input.quota_code)`.

The crate's own precedent for sharing exactly this kind of rule, `src/state.rs:1944-1956`: "Whether a request made at `level` belongs in a history listing filtered to `requested`. ... A caller that names no level gets [`LEVEL_ACCOUNT`], the documented default of the member on both history listings" — one predicate for the level half, no shared code for the status half.

Tests that pin the behaviour and must keep passing unchanged: `tests/all/requested_change_history.rs::the_history_filters_on_the_status_a_request_carries_now` (line 255) and `::the_history_filters_on_the_level_a_request_was_made_at` (line 385), plus `tests/all/support_case_lifecycle.rs:604` and `tests/all/support_cases.rs:346`, which exercise the same filters on the per-quota listing.

## Steps to Reproduce

1. Open `src/ops/list_requested_service_quota_change_history.rs` at lines 201-217 and `src/ops/list_requested_service_quota_change_history_by_quota.rs` at lines 144-160 and compare the two filter-and-map chains.
2. Change the status-filter semantics in one file only (for example to compare case-insensitively) and observe that the other listing keeps the old rule, with no compile error and no test in the suite relating the two.
