Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 8ae41775096a7a231d5f0cb65fbb6a931bd10404

# A present-but-empty BackupResourceCreationTime excludes every recovery point with no BackupCreationTime, unlike every other empty filter in the same function

_Filed: 2026-10-28T20:16:06Z_
_Operation: StartSearchJob | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `8ae41775096a7a231d5f0cb65fbb6a931bd10404`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`in_scope` (the scope resolution `StartSearchJob` runs through `recovery_points_in_scope`) treats a `BackupResourceCreationTime` that is PRESENT but states no bound — `{"BackupResourceCreationTime": {}}`, which the model allows because both `CreatedAfter` and `CreatedBefore` are optional — as a filter a record has to be judgeable against: it returns `false` for any recovery point whose record carries no `BackupCreationTime`, before it has looked at a single bound. With no bound set, nothing in the filter is ever compared against the creation time it just demanded, so the member is used only to reject records.

Every OTHER optional filter of the same function normalises "present but empty" to "no filter at all": `arn_admits` answers `true` for `Some([])`, the tag branch admits when `tags.is_empty()`, and the item-filter level does the same (`satisfied_by_any`: "an absent or empty list states no requirement at all"). Only the creation window discriminates on a filter that constrains nothing.

The consequence is not cosmetic: when the empty window drops every covered recovery point, `recovery_points_in_scope` answers an empty vector, `scan_outcome` then owes the job the terminal `FAILED` status with the `SCOPE_EMPTY` message, and `ListSearchJobBackups`/`ListSearchJobResults` report nothing — for a request that asked for no creation-time restriction at all. Recovery-point records carrying no `BackupCreationTime` are the normal case in this crate's own seeds (three of the four test seeds set it to `None`), and `SearchJobBackupsResult$BackupCreationTime` is optional in the model, so such a record is legitimate.

## Evidence

Side 1 — the creation-window branch demands a creation time before reading any bound, services/service-com-amazonaws-backupsearch-cryobackupsearchservice/src/state.rs:1525-1540:
```
    if let Some(window) = scope.backup_resource_creation_time.as_ref() {
        let Some(created) = point.backup_creation_time.map(instant_key) else {
            return false;
        };
        // "only created after"/"only created before" the given instant, with the bound
        // itself in scope: the filter names the edge of the window, not a point outside it.
        if window
            .created_after
            .is_some_and(|after| created < instant_key(after))
            || window
                .created_before
                .is_some_and(|before| created > instant_key(before))
        {
            return false;
        }
    }
```
With `created_after == None` and `created_before == None`, `created` is never used — yet a record that does not carry one has already been rejected.

Side 2 — the sibling filters of the SAME function read a present-but-empty filter as no filter, state.rs:1552-1560:
```
/// Whether an ARN filter admits `candidate`: an absent or empty list is no filter at all,
/// and a list admits exactly the ARNs it names.
fn arn_admits(filter: Option<&[String]>, candidate: Option<&str>) -> bool {
    match filter {
        None => true,
        Some([]) => true,
        Some(arns) => candidate.is_some_and(|arn| arns.iter().any(|wanted| wanted == arn)),
    }
}
```
and the tag branch, state.rs:1546-1549:
```
    scope
        .backup_resource_tags
        .as_ref()
        .is_none_or(|tags| tags.is_empty())
```
and the item-filter level, state.rs:1754-1759: "Whether an optional list of ALTERNATIVES is satisfied: an absent or empty list states no requirement at all" / `alternatives.is_none_or(|list| list.is_empty() || list.iter().any(met))`.

The consequence — state.rs:1588-1591:
```
pub fn scan_outcome(points: &[IndexedRecoveryPoint]) -> (&'static str, Option<String>) {
    if points.is_empty() {
        return (FAILED, Some(SCOPE_EMPTY.to_string()));
    }
```
with `SCOPE_EMPTY` = "Search scope is empty. Revise search scope input and retry search job" (state.rs:507-508), reached from the handler at src/ops/start_search_job.rs:305 (`let points = state.recovery_points_in_scope(&input.search_scope);`) and :336 (`let (owed_status, owed_message) = crate::state::scan_outcome(&points);`).

Model: `BackupCreationTimeFilter` declares BOTH members optional — `smithy_query(query="shape", shape="BackupCreationTimeFilter")`: `{"CreatedAfter": {"target": "Timestamp"}, "CreatedBefore": {"target": "Timestamp"}}` — so `{}` is a valid value for `SearchScope$BackupResourceCreationTime`; and `SearchJobBackupsResult$BackupCreationTime` is optional too.

Test coverage: tests/all/search_job_outcome_and_paging.rs:152-196 exercises creation windows only WITH bounds (`CreatedAfter`/`CreatedBefore` at, one second past, and spanning the backup's instant) against a record that carries a creation time (tests/all/search_jobs.rs:85 `backup_creation_time: Some(state::timestamp(created))`). No test sends a window with no bound, and no test combines a window with a record seeded `backup_creation_time: None` — the shape three other seeds use (tests/all/backup_index_status_gate.rs:100, tests/all/completed_with_issues_status_message.rs:84, tests/all/export_listing.rs:144). So the gap is unpinned.

## Steps to Reproduce

1. Seed the store with one recovery point whose record carries no creation time, as the crate's own suites do: `state::IndexedRecoveryPoint { backup: types::SearchJobBackupsResult { status: Some(state::INDEX_ACTIVE), resource_type: Some("S3".into()), backup_creation_time: None, .. }, items: vec![one S3 item], .. }` (see tests/all/backup_index_status_gate.rs:85-107).
2. Call StartSearchJob with `SearchScope = {"BackupResourceTypes": ["S3"]}` and poll GetSearchJob past the owed spans: the job settles to COMPLETED and ListSearchJobResults pages the item — the point is in scope.
3. Repeat with `SearchScope = {"BackupResourceTypes": ["S3"], "BackupResourceCreationTime": {}}` — a filter that states no bound. `in_scope` returns false at state.rs:1526-1528, the scope resolves to nothing, and GetSearchJob reports Status=FAILED with StatusMessage "Search scope is empty. Revise search scope input and retry search job"; ListSearchJobBackups and ListSearchJobResults both answer empty.
4. Compare with the sibling filters on the same request: `"BackupResourceArns": []` and `"BackupResourceTags": {}` leave the same recovery point in scope (state.rs:1557 `Some([]) => true`, state.rs:1549 `is_none_or(|tags| tags.is_empty())`), so only the empty creation window rejects it.
