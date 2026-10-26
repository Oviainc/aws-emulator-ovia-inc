Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: b17317436abe1a5dae1bd1120020acb35c8cff22

# The cursor-and-page-bound rule is hand-rolled four times in the four list handlers

_Filed: 2026-10-26T04:24:50Z_
_Operation: ListWorkspaces | Class: maintainability_

> _Note: this issue was identified against the code base at commit `b17317436abe1a5dae1bd1120020acb35c8cff22`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All four list operations implement the identical paging rule in their own handler body: filter out everything up to the `nextToken` cursor by string comparison, derive the page size as `max_results.map_or(<DEFAULT>, |max| max.clamp(1, <LIMIT>)) as usize`, compute `has_more`, truncate, and emit the last key as the next token only when `has_more`. Four copies of one rule (plus four identical constant pairs, all 100/1_000) can drift: a fix to the token-emission condition or the clamp in one listing would silently leave the other three behind. A single helper in `crate::state` taking the already-selected records, the key extractor, `max_results`, the default and the limit would own the rule once and leave every response and every existing test byte-identical.

## Evidence

The same block four times:
src/ops/list_workspaces.rs:194-215:
```
        if let Some(cursor) = input.next_token.as_deref() {
            workspaces.retain(|workspace| workspace.workspace_id.as_str() > cursor);
        }
        ...
        let page_size = input.max_results.map_or(
            crate::state::WORKSPACE_PAGE_DEFAULT,
            |max| max.clamp(1, crate::state::WORKSPACE_PAGE_LIMIT),
        ) as usize;
        let has_more = workspaces.len() > page_size;
        workspaces.truncate(page_size);
        let next_token = if has_more { workspaces.last().map(...) } else { None };
```
src/ops/list_scrapers.rs:267-284 (`SCRAPER_PAGE_DEFAULT` / `SCRAPER_PAGE_LIMIT`), src/ops/list_anomaly_detectors.rs:220-244 (`ANOMALY_DETECTOR_PAGE_DEFAULT` / `_LIMIT`), src/ops/list_rule_groups_namespaces.rs:227-248 (`RULE_GROUPS_NAMESPACE_PAGE_DEFAULT` / `_LIMIT`) repeat it verbatim, and the constants themselves are four copies of the same two numbers (src/state.rs:352-388: every `*_PAGE_LIMIT: i32 = 1_000;` and `*_PAGE_DEFAULT: i32 = 100;`, the last one documented "the same default as one every other list operation of this service documents").

Tests that cover the behaviour and must stay unchanged: tests/all/workspace_lifecycle.rs, tests/all/scraper_listing.rs, tests/all/anomaly_detector_listing.rs, tests/all/rule_groups_namespace_listing.rs and tests/all/rule_groups_namespace_listing_scope.rs.

## Steps to Reproduce

1. Read src/ops/list_workspaces.rs:194-215, src/ops/list_scrapers.rs:267-284, src/ops/list_anomaly_detectors.rs:220-244 and src/ops/list_rule_groups_namespaces.rs:227-248 side by side
2. Observe the identical cursor filter, clamp, has_more/truncate and token-emission sequence in all four, differing only in the record type and the two constants
3. Change the token-emission condition in one and note that the other three keep the old rule
