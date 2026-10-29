Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 2c7d38992bc517da436f6aedf2c705099043ad63

# ListGroupingStatuses applies no per-request Filters bound, unlike its two sibling listings, and evaluates the unbounded list while holding the store lock

_Filed: 2026-10-29T09:41:11Z_
_Operation: ListGroupingStatuses | Class: validation_

> _Note: this issue was identified against the code base at commit `2c7d38992bc517da436f6aedf2c705099043ad63`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate has four `Filters`-taking listings. Two of them (`ListGroups`, `ListTagSyncTasks`) refuse a request carrying more than `MAX_FILTERS = 10` entries with a `BadRequestException`, and both do so BEFORE taking the store lock. `ListGroupingStatuses` validates each entry's NAME and its VALUES but never bounds how MANY entries the request carries, and it evaluates the whole list inside the critical section: `let store = state.lock();` is taken at line 225 and the per-record `filters.iter().all(...)` scan runs at lines 258-266.

The modelled `ListGroupingStatusesFilterList` declares no `@length`, so there is nothing at the request boundary to cap the entry count either; a single request may therefore carry an arbitrary number of entries (each with up to 10 values) and the handler will evaluate every one of them against every grouping-status record of the addressed group — all while holding the service-wide store lock, blocking every other Resource Groups operation for the duration.

The two siblings' own doc comments state that this bound exists for exactly this reason ("the emulator's own bound on the work ONE request can ask for, since every entry is evaluated against every ... of the region"), so the rule is one this crate already decided it needs; this handler is the one that skips it. Nothing in `list_grouping_statuses.rs` comments on, or justifies, the absence.

## Evidence

The sibling that DOES bound it — `services/service-com-amazonaws-resourcegroups-ardi/src/ops/list_groups.rs:194`:
```
        let filters = input.filters.unwrap_or_default();
        if filters.len() > MAX_FILTERS {
            return Err(bad_request(format!(
                "Filters carries {} filters, which is more than the {MAX_FILTERS} this \
                 emulator applies in one call",
                filters.len(),
            )));
        }
```
and the stated rationale, `list_groups.rs:295-304`:
```
/// The most `Filters` entries this operation applies to one call.
///
/// NOT an AWS quota: the modelled `GroupFilterList` declares no length, and each
/// entry's `Values` is modelled `1..5`. This is the emulator's own bound on the
/// work ONE request can ask for, since every entry is evaluated against every
/// group of the region: it caps that product beside the bound the store already
/// puts on the groups themselves.
const MAX_FILTERS: usize = 10;
```
The same check and the same rationale again in `services/service-com-amazonaws-resourcegroups-ardi/src/ops/list_tag_sync_tasks.rs:189` and `:312-319`:
```
/// NOT an AWS quota: the modelled `ListTagSyncTasksFilterList` declares no length.
/// This is the emulator's own bound on the work ONE request can ask for, since
/// every entry is evaluated against every task of the region; it sits beside the
/// bound the store already puts on the tasks themselves. Reaching it is a
/// `BadRequestException`.
const MAX_FILTERS: usize = 10;
```
The handler that SKIPS it — `services/service-com-amazonaws-resourcegroups-ardi/src/ops/list_grouping_statuses.rs:185-223` validates only name and values:
```
        // Every filter is checked BEFORE anything is read, so a request carrying a
        // filter this listing cannot apply is refused rather than answering a page it
        // did not ask for. The names are the modelled `ListGroupingStatusesFilterName`
        // values, and each one's values must lie in the domain it selects over.
        let filters = input.filters.unwrap_or_default();
        for filter in &filters {
            match filter.name.as_str() {
```
and then does the unbounded work inside the lock — `list_grouping_statuses.rs:225` and `:258-266`:
```
        let store = state.lock();
...
        let mut remaining: Vec<(&String, &crate::state::GroupingStatusRecord)> = group
            .grouping_statuses
            .iter()
            .filter(|(resource_arn, record)| {
                filters
                    .iter()
                    .all(|filter| matches_filter(resource_arn, record, filter))
            })
            .collect();
```
The model confirms there is no boundary cap to fall back on — `smithy_query(query="shape",shape="ListGroupingStatusesFilterList")`:
```
{"shape_id": "com.amazonaws.resourcegroups#ListGroupingStatusesFilterList", "type": "list", "member": "ListGroupingStatusesFilter"}
```
(no `length`), while `ListGroupingStatusesFilter$Values` is `length {min:1, max:10}` — a per-entry bound only.

Test coverage of the touched path: `services/service-com-amazonaws-resourcegroups-ardi/tests/all/grouping_statuses.rs:233` (`filters_select_over_the_member_each_one_names`) and `:453` (`invalid_requests_are_refused_as_bad_request`, which pins 400 for a bad filter name, a bad status value and a bad resource-arn value) drive the `Filters` branch end to end; neither suite sends more than two entries, so the missing bound is untested. The sibling bound IS pinned, at `tests/all/list_groups_filters.rs:495-523` and `tests/all/list_tag_sync_tasks.rs:458-534`, both of which assert 400 for `MAX_FILTERS + 1` entries.

## Steps to Reproduce

1. grep -n 'MAX_FILTERS' services/service-com-amazonaws-resourcegroups-ardi/src/ops/ — the constant and its `filters.len() > MAX_FILTERS` branch exist only in list_groups.rs and list_tag_sync_tasks.rs.
2. Read list_grouping_statuses.rs:189-223: every entry's Name and Values are checked, but the number of entries never is.
3. Read list_grouping_statuses.rs:225 (`let store = state.lock();`) and :258-266: the whole filter list is evaluated against every grouping-status record while the store lock is held.
4. Confirm the model offers no fallback: smithy_query(query="shape",shape="ListGroupingStatusesFilterList") returns a list with no `length` trait, so the request boundary admits any number of entries.
5. Create a group, record a member with GroupResources, then POST /list-grouping-statuses with `Filters` holding many thousands of `{"Name":"status","Values":["SUCCESS"]}` entries: the request is answered 200 after evaluating every entry per record, where the same shaped request to /groups-list or /list-tag-sync-tasks is refused 400 past ten entries.
