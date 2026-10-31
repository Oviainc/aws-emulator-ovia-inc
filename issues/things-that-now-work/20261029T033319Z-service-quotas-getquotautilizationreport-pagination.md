Issue-Severity: LOW
Issue-Gap-Class: pagination
Issue-Commit: 2bb16450f75bc57f8f5e668b05a4384e1672b4e8

# GetQuotaUtilizationReport skips the NextToken check for a report that has not completed, so a token naming no position is answered 200

_Filed: 2026-10-29T03:33:19Z_
_Operation: GetQuotaUtilizationReport | Class: pagination_

> _Note: this issue was identified against the code base at commit `2bb16450f75bc57f8f5e668b05a4384e1672b4e8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetQuotaUtilizationReport` declares `InvalidPaginationTokenException`, and every other paginated path in this crate refuses a `NextToken` that does not decode to a position in the listing it was issued for. This handler only runs that check on the COMPLETED branch: a report that is still PENDING or IN_PROGRESS returns early with just `ReportId` and `Status`, and the `NextToken` (and `MaxResults`) the request carried are never looked at.

So a caller that passes a `NextToken` which names no position in any listing of this service — it only has to satisfy the modelled `^[a-zA-Z0-9/+]*={0,2}$` — is answered HTTP 200 while the report is pending, and the very same token on the very same `ReportId` becomes a 400 `InvalidPaginationTokenException` a few hundred milliseconds later, once `settle_due_reports` has moved the report to COMPLETED. A documented failure mode is turned into a success for as long as the clock has not completed the report.

## Evidence

The branch that drops the token — `src/ops/get_quota_utilization_report.rs:218-226`:
```
        // A report that has not completed has no results yet: the caller polls the
        // status until it does.
        if report.status != crate::state::REPORT_STATUS_COMPLETED {
            return Ok(GetQuotaUtilizationReportOutput {
                report_id: Some(report.id),
                status: Some(report.status),
                ..GetQuotaUtilizationReportOutput::default()
            });
        }
```
`input.next_token` is first read only afterwards, at `src/ops/get_quota_utilization_report.rs:232-248`, where the SAME handler does apply the check:
```
        let page = crate::state::paginate_bounded(
            &listing,
            report.quotas,
            input.max_results,
            input.next_token.as_deref(),
            crate::state::UTILIZATION_REPORT_PAGE_SIZE,
            crate::state::UTILIZATION_REPORT_PAGE_SIZE,
        )
        .map_err(|_| {
            GetQuotaUtilizationReportErrorModeled::InvalidPaginationTokenException(
```
and so do its siblings, unconditionally — e.g. `src/ops/list_services.rs:126-140` and `src/ops/list_service_quotas.rs:201-215` (`crate::state::paginate(...).map_err(|_| ...InvalidPaginationTokenException...)`).

The token is refusable by construction: `src/state.rs:2147-2158` (`decode_page_token`) returns `InvalidPageToken` for anything that is not base64 of `<listing>:<offset>`, and the operation models the error — `OPERATIONS.md:73`: "**Errors:** ... `InvalidPaginationTokenException (HTTP 400)` ...".

The status the early return reports is itself clock-driven, `src/state.rs:1440-1454` (`settle_due_reports`), with `REPORT_COMPLETION_DELAY` of 500 ms (`src/state.rs:293`), which is what makes the same request answer 200 and then 400.

Test coverage: `tests/all/utilization_report.rs` and `tests/all/pagination_token_scope.rs` exercise tokens only against COMPLETED reports and the list operations; no test passes a token to a PENDING/IN_PROGRESS report.

## Steps to Reproduce

1. Start the emulator and sign a request with a us-east-1 SigV4 credential scope.
2. POST / with `X-Amz-Target: ServiceQuotasV20190624.StartQuotaUtilizationReport` and body {} — note the returned ReportId and Status PENDING.
3. Immediately (within REPORT_COMPLETION_DELAY, 500 ms) POST / with `X-Amz-Target: ServiceQuotasV20190624.GetQuotaUtilizationReport` and body {"ReportId":"<id>","NextToken":"Zm9v"} — observe HTTP 200 with just ReportId and Status; the bogus token is ignored.
4. Wait ~600 ms and repeat the identical call — observe HTTP 400 InvalidPaginationTokenException "NextToken does not name a position in this report".
5. Compare with any sibling listing, e.g. ListServices with {"NextToken":"Zm9v"}, which refuses the same token on every call.
