Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: a61b7e3a8f074f2acfd74e18fad3041c10471e17

# Export jobs are listed oldest-created first instead of newest-created first

_Filed: 2026-10-24T09:35:48Z_
_Operation: ListSearchResultExportJobs | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a61b7e3a8f074f2acfd74e18fad3041c10471e17`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

ListSearchResultExportJobs pages over this service's export jobs in CREATION order (oldest first): the handler walks `state.export_jobs_in_order()`, which iterates `export_job_order` forward, and `insert_export_job` appends. So with three exports started in sequence, the first page opens with the OLDEST export job and a single-page read (MaxResults=1) returns the first export ever started rather than the latest one.

The sibling listing in the same service — ListSearchJobs — was corrected to newest-created first against the recorded corpus (see the fixed issue 20261022T215802Z-backupsearch-search-job-wire-replay-... "ListSearchJobs page order"), and src/state.rs documents that rule explicitly: "the service reports the most recently created job first, so a single-page read is 'the latest search job'". Job listings in the AWS Backup family are reported most-recent-first, and nothing in the model or in this emulator justifies the two job listings of one service disagreeing about order.

Expected: ListSearchResultExportJobs reports export jobs newest-created first (and pages that order stably).
Actual: it reports them oldest-created first.

## Evidence

Spec/grounding: smithy_query(query="operation", operation="ListSearchResultExportJobs") — GET /export-search-jobs, @paginated(items=ExportJobs) over ExportJobSummary{CreationTime,...}; the model states no order, so the service's recorded order for its other job listing is the authority, and that one is newest-first (src/state.rs `jobs_newest_first`, added for the recorded ListSearchJobs page order). Handler: src/ops/list_search_result_export_jobs.rs:179-202 uses `export_jobs_in_order()`; src/state.rs `export_jobs_in_order` iterates `export_job_order` forward (creation order) while `jobs_newest_first` iterates `job_order` in reverse. Test coverage: no test asserts export-listing order — tests/all/export_listing.rs asserts filtering, paging-exactly-once and token scoping only (grep for `newest|oldest|creation order` in tests/ finds nothing for exports), while tests/all/search_jobs.rs:401,549,574 do pin newest-first for search jobs.

## Steps to Reproduce

1. Boot the emulator and point a botocore backupsearch client at it.
2. Create a search job and drive it to an exportable status (start_search_job, stop_search_job, poll get_search_job until Status=STOPPED).
3. start_search_result_export_job three times for that search job with distinct DestinationBuckets, recording each returned ExportJobIdentifier in order.
4. list_search_result_export_jobs(MaxResults=1): the emulator returns the FIRST export job started; AWS reports the most recently created one first.
5. Follow NextToken to exhaustion and compare the full sequence with the reverse creation order.
