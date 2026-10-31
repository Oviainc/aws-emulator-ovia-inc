Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: a49d8e01a4c28a60ab66366055e0f1d39927af0d

# Deduplicate the per-request Filters bound shared by ListGroups and ListTagSyncTasks

_Filed: 2026-10-29T00:39:53Z_
_Operation: ListGroups | Class: maintainability_

> _Note: this issue was identified against the code base at commit `a49d8e01a4c28a60ab66366055e0f1d39927af0d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The emulator's own bound on how many `Filters` entries ONE request may carry is implemented twice, once per handler, as two private `MAX_FILTERS` constants with the same value (10), the same rationale in their doc comments, and a byte-identical refusal sentence. The two copies can drift: changing the bound or the sentence in one listing silently leaves the other answering a different number and a different message for the same rule.

This contradicts the crate's own established pattern: every other rule two handlers share is decided in exactly ONE place in `src/state.rs`, which the module documents as such — `page_size` ("The ONE place this service decides a page size, so every listing ... refuses a value outside `1..=MAX_PAGE_SIZE` with the same sentence, and the bound and that sentence cannot drift apart between them"), `check_resource_arns`, `check_tags`, `check_configuration`, `check_resource_query`. The filter-count rule is the one shared rule that was left copied into the two handlers instead.

Fix: hoist the constant and the `filters.len() > MAX_FILTERS` branch (with its message) into `crate::state` as one constant plus one helper, and have both handlers call it. Because the two messages and the two values are already identical, every response stays byte-for-byte what it is today and no existing test changes.

## Evidence

Copy 1 — `services/service-com-amazonaws-resourcegroups-ardi/src/ops/list_groups.rs:195`:
```
        if filters.len() > MAX_FILTERS {
            return Err(bad_request(format!(
                "Filters carries {} filters, which is more than the {MAX_FILTERS} this \
                 emulator applies in one call",
                filters.len(),
            )));
        }
```
with its own constant at `list_groups.rs:304`:
```
/// The most `Filters` entries this operation applies to one call.
///
/// NOT an AWS quota: the modelled `GroupFilterList` declares no length ...
const MAX_FILTERS: usize = 10;
```

Copy 2 — `services/service-com-amazonaws-resourcegroups-ardi/src/ops/list_tag_sync_tasks.rs:189`:
```
        if filters.len() > MAX_FILTERS {
            return Err(bad_request(format!(
                "Filters carries {} filters, which is more than the {MAX_FILTERS} this \
                 emulator applies in one call",
                filters.len(),
            )));
        }
```
with its own constant at `list_tag_sync_tasks.rs:319`:
```
/// The most `Filters` entries this operation applies to one call.
///
/// NOT an AWS quota: the modelled `ListTagSyncTasksFilterList` declares no length.
/// This is the emulator's own bound on the work ONE request can ask for ...
const MAX_FILTERS: usize = 10;
```

The crate's own norm for a rule two handlers share, `services/service-com-amazonaws-resourcegroups-ardi/src/state.rs:1795`:
```
/// The ONE place this service decides a page size, so every listing — of groups,
/// of one group's memberships, of its grouping statuses and of the account's
/// tag-sync tasks — defaults an absent `MaxResults` to [`MAX_PAGE_SIZE`] and
/// refuses a value outside `1..=MAX_PAGE_SIZE` with the same sentence, and the
/// bound and that sentence cannot drift apart between them.
```
and both handlers already defer to it two lines later (`list_groups.rs:267`, `list_tag_sync_tasks.rs:233`: `crate::state::page_size(input.max_results)`).

Test coverage for the two touched paths: `services/service-com-amazonaws-resourcegroups-ardi/tests/all/list_groups_filters.rs` ("a filter name the model does not declare is `BadRequestException` (400)", plus a 200 listing for every modelled filter name) and `services/service-com-amazonaws-resourcegroups-ardi/tests/all/list_tag_sync_tasks.rs` ("a `Filters` entry that names no group at all, a `NextToken` no listing issued, and a `MaxResults` outside the modelled range are each refused with 400"). Both suites drive the `Filters` branch of each handler end to end and must keep passing unchanged.

## Steps to Reproduce

1. grep -n 'MAX_FILTERS' services/service-com-amazonaws-resourcegroups-ardi/src/ops/ — two private constants, both `usize = 10`, in list_groups.rs:304 and list_tag_sync_tasks.rs:319.
2. Read list_groups.rs:195-201 and list_tag_sync_tasks.rs:189-195: the same branch and the same format string, duplicated.
3. Change the bound (or one word of the message) in one file only: the two listings now refuse a different number of Filters entries, or word the same refusal differently, with nothing in the crate tying them together.
4. Compare with crate::state::page_size (state.rs:1795-1814), which both handlers call for the sibling per-request bound: that rule is decided once and documented as 'The ONE place this service decides a page size ... the bound and that sentence cannot drift apart between them'.
5. Run the existing suites tests/all/list_groups_filters.rs and tests/all/list_tag_sync_tasks.rs: they must pass unchanged after the two copies are hoisted into crate::state, since both the value and the message are already identical.
