Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4e0be56b24c0fc13c424c1a1b18bb5f0120957ed

# GetMetricsSummary counts open findings by current status only, while its sibling metrics operation honours the close instant

_Filed: 2026-10-27T02:55:00Z_
_Operation: GetMetricsSummary | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4e0be56b24c0fc13c424c1a1b18bb5f0120957ed`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetMetricsSummary` resolves the requested `date` to a whole UTC day and reports the account "as of the end of that day" — it correctly EXCLUDES a finding created after that day. But for closure it is time-blind: it drops every finding whose CURRENT status is not `Open`, so a finding that was open on the requested day and closed only afterwards is missing from `openFindings`, from `scansWithMostOpenFindings` and from `scansWithMostOpenCriticalFindings` for that past day.

The sibling metrics handler in this same crate, `ListFindingsMetrics`, answers the same question ("open findings of each severity as of the specified date") with close-instant awareness: it derives a close instant from the finding's `updatedAt` when the status is `Closed`, and counts a finding as open as of a day when `closed_at > day_end`. It even documents itself as taking "the same reading `GetMetricsSummary` takes". So the two handlers, reading the SAME `store.findings`, return different counts for the same day whenever a finding carries `status = Closed` and an `updatedAt` after that day.

The justification written above the skipping line — "Only a finding's current status is stored, so that is the status the summary reads" — is contradicted inside the crate: the sibling reconstructs the close instant from the very same stored `Finding` fields (`status` + `updatedAt`), so the information the summary needs is present in the store it already holds a lock on. The summary is self-inconsistent too: it applies the as-of rule to `createdAt` and not to the close.

## Evidence

Handler that ignores the close instant — `src/ops/get_metrics_summary.rs:249-262`:
```
            // A finding created after the day asked about did not exist as of that
            // date, so it is not in this summary. ...
            if created_at_millis.is_some_and(|created| created > as_of_millis) {
                continue;
            }
            // "The number of open findings of each severity"
            // (`MetricsSummary$openFindings`) — a closed finding is in none of these
            // metrics. Only a finding's current status is stored, so that is the status
            // the summary reads.
            if finding.status.as_deref() != Some(crate::state::STATUS_OPEN) {
                continue;
            }
```
Sibling handler in the same crate that DOES honour it — `src/ops/list_findings_metrics.rs:183-188`:
```
            closed_at_millis: if status == Some(crate::state::STATUS_CLOSED) {
                finding.updated_at.as_ref().map(crate::state::from_timestamp)
            } else {
                None
            },
            open_now: status == Some(crate::state::STATUS_OPEN),
```
and `src/ops/list_findings_metrics.rs:208-218`:
```
    /// Whether this finding was open AS OF `day_end`: "The number of open findings of
    /// each severity AS OF the specified date" (`AccountFindingsMetric$openFindings`) — it
    /// existed by then and had not been closed by then. ... (the
    /// same reading `GetMetricsSummary` takes).
    fn open_as_of(&self, day_end: i64) -> bool {
        if self.created_at_millis.is_some_and(|created| created > day_end) {
            return false;
        }
        self.open_now || self.closed_at_millis.is_some_and(|closed| closed > day_end)
    }
```
Both read one store: `src/state.rs:613-617` — "Findings, keyed by `(scanName, findingId)` — the SINGLE store `GetFindings` and `BatchGetFindings` both read".

Test coverage: `tests/all/metrics_summary.rs:79-81` pins that a `Closed` finding is "a finding the summary must never count", but every seeded finding is built by `tests/all/metrics_summary.rs:191-199` (`Finding { id, status, severity, detector_name, created_at, title, ..Finding::default() }`) and so carries NO `updatedAt` — under a close-instant-aware rule such a finding records no close and is still counted in no day's open findings, so the existing assertions are unaffected. `tests/all/findings_metrics.rs:198-233` pins the sibling's per-day `openFindings`/`closedFindings` tables.

## Steps to Reproduce

1. Seed the store with one finding of severity `High` on scan `alpha`: `status = "Closed"`, `createdAt` = day D-10, `updatedAt` (the close) = today.
2. Call `GET /metrics/findings?startDate=<D>&endDate=<D>` (ListFindingsMetrics): the day's `openFindings.high` is 1, because the finding was closed after D.
3. Call `GET /metrics/summary?date=<D>` (GetMetricsSummary): `openFindings.high` is 0 and `scansWithMostOpenFindings` is empty.
4. Observe that two handlers of the same crate report different open-finding counts for the same day over the same stored finding.
