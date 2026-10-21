Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 01973b2fa0798bae2c2d50e81b185c07c4a43584

# Bootstrap the BackupSearch emulator

_Filed: 2026-10-21T21:57:50Z_
_Operation: service-com-amazonaws-backupsearch-cryobackupsearchservice | Class: missing_

> _Note: this issue was identified against the code base at commit `01973b2fa0798bae2c2d50e81b185c07c4a43584`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All 12 operations of the BackupSearch (`backup-search`, restJson1) service are unmodified scaffolds answering HTTP 501 `NotImplemented` and holding no state. Implement a FIRST minimal happy-path slice around the primary resource, the **search job**, so a realistic client script works end-to-end. Every one of these operations must compute its response from persisted state — no canned, hardcoded, or echoed-input responses, and no random identifier that is not stored.

### Happy path to implement (in order)

1. **StartSearchJob** — `PUT /search-jobs`. Input: `SearchScope` (required; `BackupResourceTypes` required list of 1), optional `ItemFilters`, `Name`, `EncryptionKeyArn`, `Tags`, `ClientToken`. Must mint and PERSIST a new job: generate a `SearchJobIdentifier` (opaque id) and a `SearchJobArn` of the form `arn:aws:backup-search:<region>:<account>:search-job/<SearchJobIdentifier>`, record `CreationTime` (server clock), store `SearchScope`, `ItemFilters`, `Name`, `EncryptionKeyArn`, `Tags`, and set `Status = RUNNING`. Output: `SearchJobIdentifier`, `SearchJobArn`, `CreationTime` — the same values that are stored. `ClientToken` must be idempotent: a repeat call with the same token returns the SAME identifier/ARN rather than creating a second job; a same-token call with a different body is `ConflictException` (409).

2. **GetSearchJob** — `GET /search-jobs/{SearchJobIdentifier}`. Must look the job up by id and return the stored `SearchJobIdentifier`, `SearchJobArn`, `CreationTime`, `SearchScope`, `ItemFilters`, `Status` (all `@required`), plus `Name`, `EncryptionKeyArn`, `SearchScopeSummary`, `CurrentSearchProgress`, `CompletionTime`/`StatusMessage` when applicable. An unknown identifier must be `ResourceNotFoundException` (404), not a 200 with an empty body.

3. **ListSearchJobs** — `GET /search-jobs`. Must return one `SearchJobSummary` per persisted job (`SearchJobIdentifier`, `SearchJobArn`, `Name`, `Status`, `CreationTime`, `CompletionTime`, `SearchScopeSummary`, `StatusMessage`), honour the `Status` query filter (`ByStatus`, one of COMPLETED|FAILED|RUNNING|STOPPED|STOPPING), and honour `MaxResults` (default 1000) with a `NextToken` that resumes where the previous page ended. A job created in step 1 must appear here; one filtered out by status must not.

4. **ListSearchJobResults** — `GET /search-jobs/{SearchJobIdentifier}/search-results`. Must return the `Results` (list of `ResultItem`) recorded for that job from state (an empty list is acceptable for a job with no synthesized matches, but the list must come from the job record and must be consistent with `CurrentSearchProgress.ItemsMatchedCount`), paginated by `MaxResults`/`NextToken`. Unknown id ⇒ `ResourceNotFoundException` (404).

5. **StopSearchJob** — `PUT /search-jobs/{SearchJobIdentifier}/actions/cancel`. Must transition the persisted job to `STOPPING`/`STOPPED` and set `CompletionTime`, so a following `GetSearchJob`/`ListSearchJobs` observes the new status. Unknown id ⇒ 404; stopping a job already in a terminal state (`COMPLETED`/`FAILED`/`STOPPED`) ⇒ `ConflictException` (409).

6. **ListTagsForResource / TagResource** (optional sixth, cheap once state exists) — `GET|POST /tags/{ResourceArn}` keyed by the search-job ARN minted in step 1, returning exactly the tags stored (from `StartSearchJob.Tags` plus any added by `TagResource`); unknown ARN ⇒ 404.

## State Model

In `ComAmazonawsBackupsearchCryobackupsearchserviceState` (`src/state.rs`), reached via `crate::state::service_state(&*self.engine)`:

- `search_jobs: HashMap<String /* SearchJobIdentifier */, SearchJob>`
- `SearchJob { identifier: String, arn: String, name: Option<String>, status: SearchJobState, creation_time: Timestamp, completion_time: Option<Timestamp>, status_message: Option<String>, encryption_key_arn: Option<String>, search_scope: SearchScope, item_filters: ItemFilters, search_scope_summary: SearchScopeSummary { total_recovery_points_to_scan_count, total_items_to_scan_count }, current_search_progress: CurrentSearchProgress { recovery_points_scanned_count, items_scanned_count, items_matched_count }, results: Vec<ResultItem>, backups: Vec<SearchJobBackupsResult>, tags: HashMap<String,String> }`
- `client_tokens: HashMap<String /* ClientToken */, String /* SearchJobIdentifier */>` for StartSearchJob idempotency
- `tags_by_arn: HashMap<String /* ResourceArn */, HashMap<String,String>>` (or resolve the ARN back to the owning `SearchJob.tags`)
- `job_order: Vec<String>` (or an insertion counter) so ListSearchJobs paginates deterministically and `NextToken` is a stable cursor into that order.

Out of scope for this first slice (stay 501, filed separately later): StartSearchResultExportJob, GetSearchResultExportJob, ListSearchResultExportJobs, ListSearchJobBackups, UntagResource.

Done = a client script that starts a search job, gets it, lists it, lists its results, stops it, and re-gets it to observe the stopped status passes with correct bodies — not merely 2xx.

## Evidence

`services/service-com-amazonaws-backupsearch-cryobackupsearchservice/OPERATIONS.md` lists all 12 operations with their routes, members and errors (restJson1, sdkId `BackupSearch`, signing name `backup-search`): StartSearchJob `PUT /search-jobs` → `SearchJobArn`/`CreationTime`/`SearchJobIdentifier`; GetSearchJob `GET /search-jobs/{SearchJobIdentifier}` with `@required` `Status`, `SearchScope`, `ItemFilters`, `CreationTime`, `SearchJobIdentifier`, `SearchJobArn`; ListSearchJobs `GET /search-jobs` `@paginated(items=SearchJobs)` with `ByStatus`/`NextToken`/`MaxResults(default 1000)`; ListSearchJobResults `GET /search-jobs/{SearchJobIdentifier}/search-results` `@paginated(items=Results)`; StopSearchJob `PUT /search-jobs/{SearchJobIdentifier}/actions/cancel` declaring `ConflictException (409)` and `ResourceNotFoundException (404)`; ListTagsForResource/TagResource `GET|POST /tags/{ResourceArn}`. Shape details from the model via smithy_query: `SearchScope.BackupResourceTypes` is `@required` with length 1..1; `SearchScopeSummary{TotalRecoveryPointsToScanCount,TotalItemsToScanCount}`; `CurrentSearchProgress{RecoveryPointsScannedCount,ItemsScannedCount,ItemsMatchedCount}`; `SearchJobSummary{SearchJobIdentifier,SearchJobArn,Name,Status,CreationTime,CompletionTime,SearchScopeSummary,StatusMessage}`. All 12 handler files under `src/ops/` still carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker (grep output: untag_resource.rs, list_search_result_export_jobs.rs, list_search_jobs.rs, start_search_result_export_job.rs, list_search_job_results.rs, start_search_job.rs, list_search_job_backups.rs, get_search_result_export_job.rs, list_tags_for_resource.rs, get_search_job.rs, tag_resource.rs, stop_search_job.rs) and return `Err(crate::scaffolded::Scaffolded::not_implemented(..))` ⇒ HTTP 501, so no state exists anywhere in the crate.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI client for `backup-search` at it.
2. Call StartSearchJob with Name="demo", SearchScope={BackupResourceTypes:["S3"]}, Tags={env:test} and a ClientToken — expect 200 with a non-empty SearchJobIdentifier, a matching SearchJobArn and a CreationTime (currently HTTP 501).
3. Repeat the identical StartSearchJob call with the SAME ClientToken — expect the same SearchJobIdentifier/SearchJobArn, and no second job.
4. Call GetSearchJob with that SearchJobIdentifier — expect 200 echoing the PERSISTED job: same identifier/ARN/CreationTime, the SearchScope submitted in step 2, and Status=RUNNING.
5. Call ListSearchJobs — expect the job in SearchJobs with its stored Name/Status/CreationTime; repeat with Status=COMPLETED and expect it absent.
6. Call ListSearchJobs with MaxResults=1 after creating two jobs — expect one item plus a NextToken, and the second page to return the other job.
7. Call ListSearchJobResults for the job — expect 200 with the Results list held in state (paginated), not 501.
8. Call StopSearchJob for the job — expect 200, then GetSearchJob shows Status=STOPPING/STOPPED with a CompletionTime; calling StopSearchJob again on the now-terminal job returns ConflictException (409).
9. Call GetSearchJob / ListSearchJobResults / StopSearchJob with the identifier "does-not-exist" — expect ResourceNotFoundException with HTTP 404.
10. Call ListTagsForResource on the search-job ARN — expect {env:test}; TagResource adding {team:a} then ListTagsForResource returns both.
