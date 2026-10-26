Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 9b93cbc54d897386c30bfd66ea5bd8f85f03a356

# Factor the one maxResults page-bound rule this crate implements five times into a single helper

_Filed: 2026-10-26T01:05:34Z_
_Operation: GetServerDetails | Class: maintainability_

> _Note: this issue was identified against the code base at commit `9b93cbc54d897386c30bfd66ea5bd8f85f03a356`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate states its `maxResults` rule once (`MAX_LIST_PAGE_SIZE`, src/state.rs:327-336: an absent `maxResults` takes the documented maximum, a value inside 1..=100 is taken as asked, anything else is refused with `invalid_request_body()` rather than clamped) and then implements that same three-arm match FIVE times, in two files:

* `list_page` (src/state.rs:511-515) — used by ListServers and ListApplicationComponents,
* `RegionState::page_of_import_tasks` (src/state.rs:1889-1893) — ListImportFileTask,
* `RegionState::page_of_collectors` (src/state.rs:1943-1947) — ListCollectors,
* `get_server_details::page_of` (src/ops/get_server_details.rs:201-205),
* `list_analyzable_servers::page_of` (src/ops/list_analyzable_servers.rs:200-204).

The five copies are character-for-character the same branch over the same constant and the same refusal, so they can drift independently: changing the rule in one place silently leaves four lists on the old rule. They have already drifted cosmetically — `list_page` and the two handler copies finish with `usize::try_from(size).unwrap_or(0)` while the two `RegionState` copies use `unwrap_or(1)` — two different fallbacks for the same impossible case.

The duplicated LOGIC is the bound-and-refuse decision; extracting it into one `fn page_size(max_results: Option<i32>) -> Result<usize, ValidationException>` beside `MAX_LIST_PAGE_SIZE` and calling it from all five sites changes no response: every site already produces the same size for an accepted value and the same `invalid_request_body()` `ValidationException` for a rejected one, and each operation keeps lifting that exception into its own error channel exactly as it does today.

## Evidence

Copy 1 — src/state.rs:511-515 (`list_page`):
  "    let size = match max_results {
           None => MAX_LIST_PAGE_SIZE,
           Some(requested) if (1..=MAX_LIST_PAGE_SIZE).contains(&requested) => requested,
           Some(_) => return Err(invalid_request_body()),
       };"

Copy 2 — src/state.rs:1889-1893 (`page_of_import_tasks`):
  "        let limit = match max_results {
               None => MAX_LIST_PAGE_SIZE,
               Some(requested) if (1..=MAX_LIST_PAGE_SIZE).contains(&requested) => requested,
               Some(_) => return Err(invalid_request_body()),
           };"

Copy 3 — src/state.rs:1943-1947 (`page_of_collectors`): identical four lines.

Copy 4 — src/ops/get_server_details.rs:201-205:
  "    let size = match max_results {
           None => MAX_LIST_PAGE_SIZE,
           Some(requested) if (1..=MAX_LIST_PAGE_SIZE).contains(&requested) => requested,
           Some(_) => return Err(refused()),
       };"

Copy 5 — src/ops/list_analyzable_servers.rs:200-204: identical, also via `refused()` (both `refused()` helpers wrap `crate::state::invalid_request_body()`: get_server_details.rs:171-173, list_analyzable_servers.rs:165-167).

The single rule the five copies implement — src/state.rs:330-336:
  "/// Every `maxResults` member the service takes documents the same bound —
   /// `ListImportFileTaskRequest$maxResults` \"The total number of items to return. The
   /// maximum value is 100.\" ... The members carry no `@range`,
   /// so the boundary validator passes any integer and the bound is enforced where the page
   /// is taken.
   pub const MAX_LIST_PAGE_SIZE: i32 = 100;"

Existing tests that pin every copy's behaviour and must keep passing unchanged:
* tests/all/server_inventory.rs:635-648 (ListServers: 0, -1, 101, 1000 refused) and :791-798 (1, 100, absent accepted)
* tests/all/application_component_listing.rs:704-717 and :888-898
* tests/all/collector_inventory.rs:520-536 and :579
* tests/all/import_file_task_listing.rs:326-355 (`max_results_is_held_to_its_documented_bound`)
* tests/all/server_details.rs:312-344 (`max_results_is_held_to_its_documented_bounds`)
* tests/all/analyzable_servers.rs:419-427

## Steps to Reproduce

1. Open src/state.rs and read lines 511-515, 1889-1893 and 1943-1947: the same `match max_results` three-arm bound appears three times.
2. Open src/ops/get_server_details.rs:201-205 and src/ops/list_analyzable_servers.rs:200-204: the same bound appears twice more, differing only in which local `refused()` wraps `invalid_request_body()`.
3. Note the already-drifted tail of the same computation: `usize::try_from(size).unwrap_or(0)` (state.rs:528, get_server_details.rs:217, list_analyzable_servers.rs:216) versus `usize::try_from(limit).unwrap_or(1)` (state.rs:1894, 1948).
4. Change the rule in any one copy (e.g. accept 0 as "no limit") and observe that the other four lists keep the old rule — only the tests listed above catch it, one per copy.
