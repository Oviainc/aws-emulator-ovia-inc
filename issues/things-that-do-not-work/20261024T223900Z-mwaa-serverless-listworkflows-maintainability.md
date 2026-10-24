Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 5b32bce31d5638057695fa0f3e53c870e68936d8

# The MaxResults page-size derivation is written out once per list handler, and the three copies have already diverged

_Filed: 2026-10-24T22:39:00Z_
_Operation: ListWorkflows | Class: maintainability_

> _Note: this issue was identified against the code base at commit `5b32bce31d5638057695fa0f3e53c870e68936d8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The same computation — turn the modelled `MaxResults` i32 into a usize page size — is implemented separately in each of this crate's three implemented list handlers. Two copies are identical and the third has already drifted: `ListWorkflowVersions` adds a `clamp(MIN_PAGE_SIZE, MAX_PAGE_SIZE)` with a comment explaining why a zero page size must never reach the walk, while `ListWorkflows` and `ListWorkflowRuns` keep the bare `unwrap_or(0)` that the clamp exists to prevent. Hoisting one helper into `crate::state` (beside `encode_page_token`/`decode_page_token`, which the same handlers already share) removes the drift; every response and every existing test stays the same, because the model already bounds `MaxResults` to `@range(min: 1, max: 100)` at the request boundary, so neither branch is reachable today.

## Evidence

Copy 1, `src/ops/list_workflows.rs:203-205`:
```
        // `MaxResults` carries `@default: 20` and `@range(1..100)`, both applied
        // before this handler runs, so the value is already a usable page size.
        let page_size = usize::try_from(input.max_results).unwrap_or(0);
```
Copy 2, `src/ops/list_workflow_runs.rs:263-265`:
```
        // `MaxResults` carries `@default: 20` and `@range(1..100)`, both applied before
        // this handler runs, so the value is already a usable page size.
        let page_size = usize::try_from(input.max_results).unwrap_or(0);
```
Copy 3, already diverged, `src/ops/list_workflow_versions.rs:254-261`:
```
        // ... it is held to
        // those published bounds anyway, because a page size of zero would report "this
        // workflow has no versions" for a workflow that has them — a wrong answer dressed
        // as success — and the walk must always make progress.
        let page_size = usize::try_from(input.max_results)
            .unwrap_or(0)
            .clamp(MIN_PAGE_SIZE, MAX_PAGE_SIZE);
```
with its own private constants `src/ops/list_workflow_versions.rs:103-106` (`MIN_PAGE_SIZE: usize = 1`, `MAX_PAGE_SIZE: usize = 100`) duplicating the model bound `smithy_query shape=ListWorkflowsRequest` -> `"MaxResults": {... "range": {"min": 1, "max": 100}, "default": 20}`.

Tests that cover the three walks and must stay green unchanged: `tests/all/workflow_lifecycle.rs:517` (`list_workflows_pages_over_every_persisted_workflow_exactly_once`), `tests/all/workflow_run_listing.rs`, `tests/all/workflow_run_listing_order.rs`, `tests/all/workflow_version_listing.rs:220` (`every_stored_version_is_listed_exactly_once_across_pages_in_append_order`).

## Steps to Reproduce

1. Open src/ops/list_workflows.rs:205, src/ops/list_workflow_runs.rs:265 and src/ops/list_workflow_versions.rs:259-261 side by side.
2. Observe the same MaxResults -> page-size derivation written three times, with only the third applying the clamp its comment argues is required.
3. Note that src/state.rs already hosts the shared pagination helpers (encode_page_token/decode_page_token at src/state.rs:748-761) the same three handlers call, so the derivation has an obvious single home.
