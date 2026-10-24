Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 1f773e23b3e0bd2b9ce8ba90d57212de869e61a4

# Search results are hardcoded empty and the item counts are hardcoded zero, so no ResultItem is ever returned

_Filed: 2026-10-24T08:45:25Z_
_Operation: ListSearchJobResults | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `1f773e23b3e0bd2b9ce8ba90d57212de869e61a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

ListSearchJobResults pages over the items a search job matched inside the backup indexes of the recovery points it scanned (model: Results is a list of ResultItem = S3ResultItem | EBSResultItem, with ObjectKey/ETag/Size/VersionId resp. FilePath/FileSize/LastModifiedTime). GetSearchJob's SearchScopeSummary.TotalItemsToScanCount and CurrentSearchProgress.ItemsScannedCount/ItemsMatchedCount report the same item layer.

In the emulator that whole layer is a constant. StartSearchJob writes `total_items_to_scan_count: Some(0)` and `results: Vec::new()` unconditionally (src/ops/start_search_job.rs:276-311) no matter which recovery points the scope resolved to, and the settle path then reports ItemsScannedCount = 0 and ItemsMatchedCount = 0 for a COMPLETED job (src/state.rs:333-341). Consequently ListSearchJobResults answers `{"Results": []}` for every job that can ever exist, ItemFilters (S3ItemFilters/EBSItemFilters) are stored and echoed by GetSearchJob but can never select or exclude anything, and a COMPLETED job always claims it had zero items to scan even when SearchScopeSummary.TotalRecoveryPointsToScanCount is greater than zero.

Expected: a search job that resolved to indexed recovery points reports a non-zero TotalItemsToScanCount, scans the items those backups hold (for an S3 backup, the objects of the backed-up bucket), applies ItemFilters to them, and ListSearchJobResults pages the matched ResultItems with ItemsMatchedCount agreeing with the number of results.

This is a distinct layer from the already-filed scope-resolution gap (20261024T024736Z, StartSearchJob/event-propagation): even with recovery points seeded into scope so the job settles to COMPLETED, the item counts stay zero and the results page stays empty, so fixing scope resolution alone leaves ListSearchJobResults permanently empty.

## Evidence

Spec: Smithy ListSearchJobResults output `Results` (list<ResultItem>, @required) with ResultItem = union{S3ResultItem, EBSResultItem}; GetSearchJob output SearchScopeSummary.TotalItemsToScanCount and CurrentSearchProgress.{ItemsScannedCount, ItemsMatchedCount}; StartSearchJobInput.ItemFilters documented as "all input item properties specified when the search was created". Handler/state: src/ops/start_search_job.rs:276-311 hardcodes total_items_to_scan_count=0 and results=Vec::new(); src/state.rs:333-341 copies those zeros into the COMPLETED progress; src/ops/list_search_job_results.rs:179-196 pages over job.results, which nothing ever appends to. Test coverage: tests/all/search_job_outcome_and_paging.rs and export_listing.rs only seed recovery points (the backup layer) — no test seeds or asserts a ResultItem, so the item layer is untested, not pinned.

## Steps to Reproduce

1. Boot the emulator with backup-search mounted.
2. Arrange a search job that reaches COMPLETED over at least one recovery point in scope (today only possible by seeding backup-search's recovery_points; after the scope-resolution fix, by StartBackupJob on an indexed S3 resource).
3. GetSearchJob(<id>): SearchScopeSummary.TotalRecoveryPointsToScanCount is > 0 but TotalItemsToScanCount is 0 and CurrentSearchProgress.ItemsScannedCount/ItemsMatchedCount are 0.
4. ListSearchJobResults(SearchJobIdentifier=<id>): the emulator answers {"Results": []} with no NextToken, whatever the ItemFilters the job was started with — AWS returns the matched S3ResultItem/EBSResultItem entries for an indexed backup.
