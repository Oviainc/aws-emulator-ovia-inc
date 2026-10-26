Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 617cb494a3d8f9d308b6478ef47b3df7665967e2

# A scan searches the items of every in-scope recovery point, ignoring the stored backup-index status the model says excludes a backup from search results

_Filed: 2026-10-26T08:47:02Z_
_Operation: StartSearchJob | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `617cb494a3d8f9d308b6478ef47b3df7665967e2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Scope resolution and the item scan read every member of a stored recovery point EXCEPT the one the model says decides whether that recovery point takes part in the search at all: its backup-index status. `recovery_points_in_scope` filters with `in_scope`, which judges the resource type, the two ARN lists and the creation window (and explains why it cannot judge a tag filter), but never looks at `point.backup.status`. `scanned_items` then flattens the items of EVERY point the scope selected.

Consequence: a recovery point this store holds whose index status is FAILED (or anything other than ACTIVE) still contributes its indexed items to `TotalItemsToScanCount`, to `ItemsScannedCount`/`ItemsMatchedCount`, and to the `Results` that `ListSearchJobResults` pages — while the model states such a recovery point is NOT included in search results and is only DISPLAYED, with its status and status message, by `ListSearchJobBackups` (which this crate does do, out of the same record). So the emulator returns item results drawn from a backup the service says was excluded from the search, and counts it as scanned.

The status is not a field the crate lacks: `SearchJob::backups` is documented in this crate as holding each recovery point "with the status its backup index had", `SearchJobBackupsResult.status` is stored, reported and seeded by this crate's own tests — it is simply read by nothing. (A handler comment even claims the per-backup status is materialised before reporting, which `settle` never touches.)

## Evidence

Smithy, ListSearchJobBackups documentation (smithy_query(query="doc", shape="ListSearchJobBackups")), states the rule and ties it to the status member this crate stores:
"Any backups that were not included because they have a FAILED status from a permissions issue will be displayed, along with a status message." and "Only recovery points with a backup index that has a status of ACTIVE will be included in search results. If the index has any other status, its status will be displayed along with a status message."

The code that ignores it — services/service-com-amazonaws-backupsearch-cryobackupsearchservice/src/state.rs:939-945:
```
    pub fn recovery_points_in_scope(&self, scope: &SearchScope) -> Vec<IndexedRecoveryPoint> {
        self.recovery_points
            .iter()
            .filter(|point| in_scope(&point.backup, scope))
            .cloned()
            .collect()
    }
```
and src/state.rs:1113-1156 `fn in_scope(point: &SearchJobBackupsResult, scope: &SearchScope) -> bool` — it branches on `backup_resource_types`, `backup_resource_arns`, `source_resource_arns`, `backup_resource_creation_time` and `backup_resource_tags`, and carries an explicit note for the one member it cannot judge ("A tag filter: a recovery point carries no tags in this model..."), but has no branch for `point.status` at all.

The items of every selected point are then scanned — src/state.rs:1198-1203:
```
pub fn scanned_items(points: &[IndexedRecoveryPoint]) -> Vec<ResultItem> {
    points
        .iter()
        .flat_map(|point| point.items.iter().map(|item| item_of_backup(item, point)))
        .collect()
}
```
and consumed by the handler — src/ops/start_search_job.rs:291-302: `let points = state.recovery_points_in_scope(&input.search_scope); let scanned = crate::state::scanned_items(&points); ... let scan_matches: Vec<ResultItem> = scanned.iter().filter(|item| crate::state::item_matches_filters(item, &item_filters)) ...`.

This crate itself says that status is the index's status — src/state.rs:560-563:
```
    /// The recovery points the scan covered, each with the status its backup index had.
    /// `ListSearchJobBackups` pages over exactly this: ...
    pub backups: Vec<SearchJobBackupsResult>,
```
and src/ops/list_search_job_backups.rs:178-180 claims it is materialised on read ("The status each covered backup is reported with is part of what a reader observes, so materialise what the clock already owed before reporting."), although `SearchJob::settle` (src/state.rs:644-681) writes only `status`, `status_message`, `completion_time`, `results`, `scan_matches` and `current_search_progress` — never `backups`.

Test coverage: every recovery point the suite seeds carries `status: Some(state::COMPLETED.to_string())` (tests/all/search_jobs.rs:62-77 `recovery_point`), and the only rows that seed other statuses carry no items (tests/all/export_listing.rs:133-146 `backup`), so no existing test pins the behaviour of a non-ACTIVE/FAILED-index point that holds items.

## Steps to Reproduce

1. Seed the store's `recovery_points` with two IndexedRecoveryPoints of resource type S3, each holding one object in its `items`: one whose `backup.status` is COMPLETED and one whose `backup.status` is FAILED (the shape tests/all/search_job_item_results.rs:84-101 `indexed` builds).
2. PUT /search-jobs {"SearchScope":{"BackupResourceTypes":["S3"]}} and note the SearchJobIdentifier.
3. Wait past TRANSITION_DELAY so the scan settles to COMPLETED.
4. GET /search-jobs/{id} — SearchScopeSummary.TotalItemsToScanCount is 2 and CurrentSearchProgress.ItemsMatchedCount is 2, counting the item indexed under the FAILED-status backup.
5. GET /search-jobs/{id}/search-results — Results holds both items, including the one whose BackupResourceArn is the FAILED-status recovery point, although the model says only an ACTIVE-index recovery point is included in search results and a FAILED one is merely displayed by ListSearchJobBackups.
6. GET /search-jobs/{id}/backups — both recovery points are listed with their stored Status, which is the display path the model describes and the only place the status is used.
