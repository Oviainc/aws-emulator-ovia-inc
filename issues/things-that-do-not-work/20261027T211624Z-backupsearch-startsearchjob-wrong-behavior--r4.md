Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: c0eb4295fc6918b9011f996ea325c28045b54b65

# A COMPLETED search job that left covered recovery points unscanned reports no StatusMessage

_Filed: 2026-10-27T21:16:24Z_
_Operation: StartSearchJob | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `c0eb4295fc6918b9011f996ea325c28045b54b65`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The terminal status and the `StatusMessage` the clock owes a scan are decided by `state::scan_outcome`, which is keyed on the number of recovery points the scope resolved to and nothing else: `scan_outcome(0)` is `(FAILED, Some(SCOPE_EMPTY))` and every other count is `(COMPLETED, None)`. The handler calls it with `points.len()` — the COVERED points — even though the points the scan could actually read are the smaller set it computed one line earlier (`searched_recovery_point_count(&points)`, which drops every covered point whose backup index was in no state to be read).

So a search job that covered recovery points the search could NOT get into (a backup index with a FAILED status from a permissions issue, the exact case the model names) settles to `COMPLETED` with `StatusMessage` left unset: `SearchJob::settle` writes `self.status_message = message.map(ToString::to_string)` and `message` is `None`. `GetSearchJob` and `ListSearchJobs` then report that record verbatim, so the member the model says carries the explanation for a "COMPLETED job with issues" is absent precisely when there were issues. The crate itself already knows which backups were left out and why — it keeps each covered backup's own status and status message on the record and displays them through `ListSearchJobBackups` — but nothing is carried up to the job-level `StatusMessage`.

Consequence for a caller: a client that polls to a terminal status and then reads `StatusMessage` to find out whether the search was complete gets `COMPLETED` with no message, i.e. the same answer it gets for a search that got through its whole scope, and has no way to learn from the job record that part of the scope was skipped. The all-unreadable extreme is the clearest: `TotalRecoveryPointsToScanCount` and `TotalItemsToScanCount` are 0, no result is produced, and the job still reports an unqualified success.

## Evidence

The model states the behaviour on the member itself — `smithy_query(query="doc", shape="GetSearchJobOutput", member="StatusMessage")`:

  "A status message will be returned for either a earch job with a status of ERRORED or a status of COMPLETED jobs with issues."
  "For example, a message may say that a search contained recovery points unable to be scanned because of a permissions issue."

(the identical text is on `SearchJobSummary$StatusMessage`, which `ListSearchJobs` reports). The "recovery points unable to be scanned" case is the one this crate models: `smithy_query(query="doc", shape="ListSearchJobBackups")` — "Any backups that were not included because they have a FAILED status from a permissions issue will be displayed, along with a status message. Only recovery points with a backup index that has a status of ACTIVE will be included in search results."

The code that answers the opposite — services/service-com-amazonaws-backupsearch-cryobackupsearchservice/src/state.rs:1529-1535:
```
pub fn scan_outcome(recovery_points_in_scope: usize) -> (&'static str, Option<&'static str>) {
    if recovery_points_in_scope == 0 {
        (FAILED, Some(SCOPE_EMPTY))
    } else {
        (COMPLETED, None)
    }
}
```
called with the COVERED count, not the readable one — src/ops/start_search_job.rs:299-300 and 326:
```
        let points = state.recovery_points_in_scope(&input.search_scope);
        let searched_points = crate::state::searched_recovery_point_count(&points);
...
        let (owed_status, owed_message) = crate::state::scan_outcome(points.len());
```
and the `None` is what the record stores on settling — src/state.rs:951-955:
```
            self.status = next.to_string();
            // The status and the message that status comes with are written together, so
            // a record can never explain a failure it has not reached, nor reach one
            // silently.
            self.status_message = message.map(ToString::to_string);
```
which `GetSearchJob` reports as-is (src/ops/get_search_job.rs:208: `status_message: job.status_message.clone(),`).

The crate's own description of the left-out backups confirms the information exists and is only surfaced on the other listing — src/ops/start_search_job.rs:19-22:
```
//! Both counts are over the covered recovery points the scan can READ: a backup whose
//! index was in no state to be read ([`crate::state::index_searchable`]) takes no part in
//! the search, so neither it nor any of its items is scanned, counted, matched or paged —
//! it is only displayed, with the status saying why, by `ListSearchJobBackups`.
```

Test coverage: no test pins an absent `StatusMessage` for this case. `tests/all/backup_index_status_gate.rs:583-588` drives a scope whose every covered backup is unreadable and asserts only the status —
```
    assert_eq!(
        fetched.string("Status"),
        state::COMPLETED,
        "case \"no readable backup\": the scope resolved to backups, so the scan is not the empty-scope failure, got {:?}",
```
— and the one test that does assert `StatusMessage` is null on a COMPLETED job (`tests/all/search_job_outcome_and_paging.rs:248-278`) seeds only READABLE recovery points (`tests/all/search_jobs.rs:69`: `status: Some(state::COMPLETED.to_string())`), i.e. a "COMPLETED without issues" job, which the model text does not require a message for.

## Steps to Reproduce

1. Seed the store's `recovery_points` with one `IndexedRecoveryPoint` of resource type S3 whose `backup.status` is `FAILED` and whose `backup.status_message` says why (the shape `tests/all/backup_index_status_gate.rs:85-107` builds), holding one or more items.
2. PUT /search-jobs with {"SearchScope":{"BackupResourceTypes":["S3"]}} and note the SearchJobIdentifier.
3. Wait past TRANSITION_DELAY so the clock makes the scan's terminal status due.
4. GET /search-jobs/{id}: Status is COMPLETED, SearchScopeSummary.TotalRecoveryPointsToScanCount is 0, CurrentSearchProgress.ItemsMatchedCount is 0 — and StatusMessage is absent.
5. GET /search-jobs/{id}/backups: the covered backup is displayed with its FAILED index status and its message, proving the service holds the explanation the job-level StatusMessage does not carry.
6. Compare with the modelled member documentation quoted above, which says a status message will be returned for a COMPLETED job with issues, naming recovery points unable to be scanned as the example.
