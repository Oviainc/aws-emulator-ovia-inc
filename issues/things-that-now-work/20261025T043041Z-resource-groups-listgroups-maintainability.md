Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: dbe9ca7a8a80909c811e91f1b58fc3cbb14634ed

# The MaxResults page-size rule is implemented three times, once per listing handler

_Filed: 2026-10-25T04:30:41Z_
_Operation: ListGroups | Class: maintainability_

> _Note: this issue was identified against the code base at commit `dbe9ca7a8a80909c811e91f1b58fc3cbb14634ed`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The same rule — default the page to the modelled maximum when `MaxResults` is absent, otherwise convert it losslessly and refuse anything outside 1..=50 with one fixed message — is written out three times, once in each listing handler, together with three private `MAX_RESULTS` constants. The copies are character-identical today (including the refusal sentence), so they can silently drift: changing the bound or the wording in one listing leaves the other two answering differently for the same input. Everywhere else this crate deliberately centralises a shared rule in `state.rs` ("The ONE place this service decides whether a tag may be written", src/state.rs:337), which is where this one belongs. The fix is a single shared helper; every response and every existing test stays byte-identical.

## Evidence

Copy 1 — src/ops/list_groups.rs:268-287:
```
const MAX_RESULTS: usize = 50;
…
fn page_size(max_results: Option<i32>) -> Result<usize, ListGroupsError> {
    match max_results {
        None => Ok(MAX_RESULTS),
        Some(max) => usize::try_from(max)
            .ok()
            .filter(|size| (1..=MAX_RESULTS).contains(size))
            .ok_or_else(|| {
                bad_request(format!(
                    "MaxResults {max} is not valid: it must be between 1 and {MAX_RESULTS}"
```
Copy 2 — src/ops/list_group_resources.rs:304-322, same body:
```
const MAX_RESULTS: usize = 50;
…
fn page_size(max_results: Option<i32>) -> Result<usize, ListGroupResourcesError> {
…
                    "MaxResults {max} is not valid: it must be between 1 and {MAX_RESULTS}"
```
Copy 3 — src/ops/list_grouping_statuses.rs:318-336, same body:
```
const MAX_RESULTS: usize = 50;
…
fn page_size(max_results: Option<i32>) -> Result<usize, ListGroupingStatusesError> {
…
                    "MaxResults {max} is not valid: it must be between 1 and {MAX_RESULTS}"
```
Contrast with the crate's own convention for a shared rule, src/state.rs:336-341: "The ONE place this service decides whether a tag may be written, so every path that stores tags … refuses the same pairs the same way".

Existing coverage that must stay green and unchanged: tests/all/resource_group_lifecycle.rs:707 (ListGroups MaxResults range), :1106 and :1174 (ListGroupResources paging), :1238 (absent MaxResults defaults to the modelled maximum); tests/all/grouping_statuses.rs:343-405 (paging at every page size and with MaxResults absent) and :537-546 (MaxResults 0, 51 and -1 refusals).

## Steps to Reproduce

1. Open src/ops/list_groups.rs, src/ops/list_group_resources.rs and src/ops/list_grouping_statuses.rs side by side and compare the `MAX_RESULTS` constant and the `page_size` function: three identical implementations of one rule.
2. Change the refusal wording or the bound in one of them only: the other two listings now answer a different message for the same out-of-range MaxResults, with no test or doc stating why they should differ.
