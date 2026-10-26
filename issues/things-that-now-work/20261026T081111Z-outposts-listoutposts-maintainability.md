Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 3af8ecaf4ce2325789e9d5260e121bf04ff83dc7

# The "a *Filter is a list of accepted values, an absent or empty filter matches everything" rule is implemented five times in this crate

_Filed: 2026-10-26T08:11:11Z_
_Operation: ListOutposts | Class: maintainability_

> _Note: this issue was identified against the code base at commit `3af8ecaf4ce2325789e9d5260e121bf04ff83dc7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Five list handlers each define a private `matches_filters` whose body re-states the identical selection rule: a filter member is `Option<Vec<String>>`; when it is absent or empty it narrows nothing; otherwise the record's value for that member must be present and be one of the accepted values. Four of them spell it out as the same three-line closure, character for character, and the fifth writes the same two arms inline for its single list filter. The five copies can drift apart on the rule they share (what an EMPTY filter list means, and whether a record with no value for a filtered member is narrowed out) even though every handler's own doc comment says they must agree — `list_asset_instances.rs:245-249` explicitly describes its rule as "the rule `ListAssets` and `ListOutposts` apply to their own filters". The crate already factors other cross-list rules into `state.rs` for exactly this reason (`state::page`, documented at `state.rs:4805-4810` as "Shared by both list operations so they cannot drift apart", and `state::apply` at `state.rs:4101-4106` "Shared by the three update operations so they cannot drift apart on that rule"), so the shared home for this rule already exists. Lifting the predicate into one helper there changes no response and no test.

## Evidence

Copy 1 — `src/ops/list_outposts.rs:201-207`:

    fn matches_filters(outpost: &Outpost, input: &ListOutpostsInput) -> bool {
        let matches = |filter: Option<&Vec<String>>, value: Option<&String>| match filter {
            Some(accepted) if !accepted.is_empty() => {
                value.is_some_and(|value| accepted.contains(value))
            }
            _ => true,
        };

Copy 2 — `src/ops/list_sites.rs:196-202`:

    fn matches_filters(site: &Site, input: &ListSitesInput) -> bool {
        let matches = |filter: Option<&Vec<String>>, value: Option<&String>| match filter {
            Some(accepted) if !accepted.is_empty() => {
                value.is_some_and(|value| accepted.contains(value))
            }
            _ => true,
        };

Copy 3 — `src/ops/list_assets.rs:254-260`: the same `fn matches_filters(asset: &AssetInfo, input: &ListAssetsInput) -> bool { let matches = |filter: Option<&Vec<String>>, value: Option<&String>| match filter { Some(accepted) if !accepted.is_empty() => { value.is_some_and(|value| accepted.contains(value)) } _ => true, };`

Copy 4 — `src/ops/list_asset_instances.rs:256-262`: the same closure again, under a doc comment that names the duplication (`:245-249`): "several filters together narrow rather than widen — the rule `ListAssets` and `ListOutposts` apply to their own filters".

Copy 5 — `src/ops/list_capacity_tasks.rs:246-249`, the same two arms written inline:

        let status_matches = match status_filter {
            Some(accepted) if !accepted.is_empty() => accepted.contains(&task.status),
            _ => true,
        };

The existing shared home for such a rule, in this same crate — `src/state.rs:4805-4810`: "Shared by both list operations so they cannot drift apart: the records are ordered by creation sequence…" and `src/state.rs:4101-4106`: "Shared by the three update operations so they cannot drift apart on that rule."

Tests that pin the behaviour the refactor must leave untouched: `tests/all/site_and_outpost_lifecycle.rs:783-912` (ListOutposts' `AvailabilityZoneFilter` / `AvailabilityZoneIdFilter` / `LifeCycleStatusFilter` and ListSites' three `OperatingAddress*Filter` rows, including multi-value and no-match rows), `tests/all/outpost_assets.rs:465-591` (ListAssets' `AssetTypeFilter` / `StatusFilter` / `HostIdFilter`), `tests/all/asset_instances.rs:278-456` (ListAssetInstances' four filters, all combinations), `tests/all/capacity_task_listing.rs:540-599` (ListCapacityTasks' `CapacityTaskStatusFilter` / `OutpostIdentifierFilter`), and `tests/all/outpost_hardware_installation.rs:556-591`.

## Steps to Reproduce

1. Open src/ops/list_outposts.rs at line 201 and src/ops/list_sites.rs at line 196 — the `matches` closure inside `matches_filters` is identical.
2. Open src/ops/list_assets.rs:254 and src/ops/list_asset_instances.rs:256 — the same closure a third and fourth time.
3. Open src/ops/list_capacity_tasks.rs:246 — the same two match arms inline for the status filter.
4. Note src/state.rs:4805-4810 and src/state.rs:4101-4106, where this crate already keeps the cross-operation paging and PATCH-merge rules precisely so the handlers "cannot drift apart": the filter-selection rule has no such single home.
5. Change what an empty filter list means in one copy only and observe that the other four lists keep the old meaning.
