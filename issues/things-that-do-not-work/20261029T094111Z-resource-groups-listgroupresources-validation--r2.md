Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 2c7d38992bc517da436f6aedf2c705099043ad63

# ListGroupResources applies no per-request Filters bound, unlike its two sibling listings, and evaluates the unbounded list while holding the store lock

_Filed: 2026-10-29T09:41:11Z_
_Operation: ListGroupResources | Class: validation_

> _Note: this issue was identified against the code base at commit `2c7d38992bc517da436f6aedf2c705099043ad63`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Of this crate's four `Filters`-taking listings, `ListGroups` and `ListTagSyncTasks` refuse a request carrying more than `MAX_FILTERS = 10` entries with a `BadRequestException`, each BEFORE it takes the store lock. `ListGroupResources` checks only that every entry's `Name` is `resource-type` and never bounds how MANY entries arrive; it then evaluates the whole list twice inside the critical section — once per request value in `unadmitted_types` (line 254) and once per member in the page scan (lines 283-287) — with `let store = state.lock();` taken at line 232.

The modelled `ResourceFilterList` declares no `@length`, so nothing at the request boundary caps the entry count either (`ResourceFilter$Values` is bounded `1..5`, but that is per entry). A request can therefore carry an arbitrary number of `resource-type` entries and make the handler evaluate every one of them against every static membership of the addressed group while holding the service-wide store lock, blocking every other Resource Groups operation.

The two siblings' own constants document this bound as the rule the emulator needs precisely because "every entry is evaluated against every ... of the region"; this handler is the one that skips it, and nothing in `list_group_resources.rs` comments on or justifies the omission.

## Evidence

The sibling that DOES bound it — `services/service-com-amazonaws-resourcegroups-ardi/src/ops/list_groups.rs:194-201`:
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
with its stated rationale at `list_groups.rs:295-304`:
```
/// NOT an AWS quota: the modelled `GroupFilterList` declares no length, and each
/// entry's `Values` is modelled `1..5`. This is the emulator's own bound on the
/// work ONE request can ask for, since every entry is evaluated against every
/// group of the region: it caps that product beside the bound the store already
/// puts on the groups themselves.
const MAX_FILTERS: usize = 10;
```
and the identical branch in `services/service-com-amazonaws-resourcegroups-ardi/src/ops/list_tag_sync_tasks.rs:189-195` with its own `const MAX_FILTERS: usize = 10;` at `:319`.

The handler that SKIPS it — `services/service-com-amazonaws-resourcegroups-ardi/src/ops/list_group_resources.rs:221-230`, which validates names only:
```
        let filters = input.filters.unwrap_or_default();
        for filter in &filters {
            if filter.name != "resource-type" {
                return Err(bad_request(format!(
                    "Filters contains the filter name {}, which is not valid: expected one of {}",
                    filter.name,
                    RESOURCE_FILTER_NAME_VALUES.join(", "),
                )));
            }
        }
```
and then does the unbounded work after `list_group_resources.rs:232` (`let store = state.lock();`) — at `:253-254`:
```
        if let Some(limit) = group.query_resource_type_limit() {
            let invalid = unadmitted_types(&filters, &limit);
```
and at `:283-287`:
```
        let mut remaining: Vec<&crate::state::MemberRecord> = group
            .members
            .iter()
            .filter(|member| filters.iter().all(|filter| matches_filter(member, filter)))
            .collect();
```
The model offers no fallback bound — `smithy_query(query="shape",shape="ResourceFilterList")`:
```
{"shape_id": "com.amazonaws.resourcegroups#ResourceFilterList", "type": "list", "member": "ResourceFilter"}
```
(no `length`), while `ResourceFilter$Values` carries `length {min:1, max:5}` — per entry only.

Test coverage of the touched path: `services/service-com-amazonaws-resourcegroups-ardi/tests/all/list_group_resources_filter_validation.rs` exercises the `Filters` branch end to end, and `tests/all/resource_group_lifecycle.rs` pages memberships; neither sends more than a handful of entries, so the missing bound is untested. The sibling bound IS pinned at `tests/all/list_groups_filters.rs:495-523` and `tests/all/list_tag_sync_tasks.rs:458-534`, which assert 400 for `MAX_FILTERS + 1` entries.

## Steps to Reproduce

1. grep -n 'MAX_FILTERS' services/service-com-amazonaws-resourcegroups-ardi/src/ops/ — the constant and the `filters.len() > MAX_FILTERS` branch exist only in list_groups.rs and list_tag_sync_tasks.rs.
2. Read list_group_resources.rs:221-230: each entry's Name is validated, the number of entries never is.
3. Read list_group_resources.rs:232 (`let store = state.lock();`), then :254 and :283-287: the whole filter list is evaluated inside the critical section, once per request value and once per stored member.
4. Confirm the model offers no fallback: smithy_query(query="shape",shape="ResourceFilterList") returns a list with no `length` trait.
5. Create a static group, record members with GroupResources, then POST /list-group-resources with `Filters` holding many thousands of `{"Name":"resource-type","Values":["AWS::EC2::Instance"]}` entries: the request is answered 200 after evaluating every entry, where the same shaped request to /groups-list or /list-tag-sync-tasks is refused 400 past ten entries.
