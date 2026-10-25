Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 215626b640e395abbef6c78b28a02b35719dbae3

# The listing cursor/page-size logic is implemented twice, once per list handler

_Filed: 2026-10-24T22:27:15Z_
_Operation: ListDashboards | Class: maintainability_

> _Note: this issue was identified against the code base at commit `215626b640e395abbef6c78b28a02b35719dbae3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListDashboards` and `ListScheduledReports` each carry their own full copy of the same pagination rule: decode `nextToken` for this resource's ARN segment or refuse it with a `ValidationException`, convert `maxResults` to a page size and refuse a non-positive one, walk the `BTreeMap` from `Bound::Included(cursor)`, take `page_size` items, and mint the cursor from the first item after the page. The two copies differ only in the map walked, the projection applied, the ARN segment constant and the operation name in the message — so a change to the cursor rule (or a fix to one of them) has to be made twice and the two can drift apart, even though the crate already keeps the token codec itself in one place (`state::encode_page_token` / `state::decode_page_token`).

The same logic could be expressed once (a helper beside the token codec in `src/state.rs`, parameterised by the map, the ARN segment, the operation name and the projection), leaving both responses and both existing pagination suites byte-for-byte unchanged.

## Evidence

Copy 1 — services/service-com-amazonaws-bcmdashboards-awsbcmdashboardsservice/src/ops/list_dashboards.rs:164-220:
```
        let resume_from = match input.next_token.as_deref() {
            None => None,
            Some(token) => match crate::state::decode_page_token(
                token,
                crate::state::DASHBOARD_ARN_SEGMENT,
            ) { Some(arn) => Some(arn), None => { return Err(... ValidationException ...) } },
        };
...
        let page_size = match usize::try_from(input.max_results) {
            Ok(size) if size >= 1 => size,
            _ => { return Err(... "Member must have value greater than or equal to 1: maxResults ...") }
        };
        let start = match resume_from.as_deref() {
            Some(arn) => ::std::ops::Bound::Included(arn),
            None => ::std::ops::Bound::Unbounded,
        };
        let mut remaining = state.dashboards.range::<str, _>((start, ::std::ops::Bound::Unbounded));
...
        let next_token = remaining.next().map(|(arn, _)| crate::state::encode_page_token(arn));
```
Copy 2 — services/service-com-amazonaws-bcmdashboards-awsbcmdashboardsservice/src/ops/list_scheduled_reports.rs:165-221, the identical sequence with `SCHEDULED_REPORT_ARN_SEGMENT` and `state.scheduled_reports`:
```
        let page_size = match usize::try_from(input.max_results) {
            Ok(size) if size >= 1 => size,
            _ => { return Err(... "Member must have value greater than or equal to 1: maxResults ...") }
        };
...
        let mut remaining = state.scheduled_reports.range::<str, _>((start, ::std::ops::Bound::Unbounded));
...
        let next_token = remaining.next().map(|(arn, _)| crate::state::encode_page_token(arn));
```
Existing tests that pin both copies' observable behaviour (and must stay unchanged): `tests/all/dashboard_lifecycle.rs:571-728` (pages for real, `nextToken` exactly when more remain, `maxResults` bounds, uninterpretable token refused) and `tests/all/scheduled_report_listing.rs:213-525` (same table for scheduled reports, including a dashboard cursor refused by the report listing).

## Steps to Reproduce

1. Read src/ops/list_dashboards.rs:164-220 and src/ops/list_scheduled_reports.rs:165-221 side by side: the cursor-decode refusal, the page-size refusal, the range walk and the next-cursor computation are the same code twice.
2. Change the pagination rule in one file only (for example the page-size refusal message or the cursor position) and observe that the other listing keeps the old rule — the two copies have no shared definition to keep them in step.
3. Run `cargo test -p service-com-amazonaws-bcmdashboards-awsbcmdashboardsservice`: dashboard_lifecycle and scheduled_report_listing both pass today and must keep passing unchanged after the logic is unified.
