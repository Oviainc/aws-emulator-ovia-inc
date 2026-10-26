Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 0c701d5f8ae664c771f5c5cdfd2b2b982b891619

# ListJobTemplates hand-rolls the ordering/cursor/page-size logic the state module already owns for queues, presets, jobs and versions

_Filed: 2026-10-25T04:07:54Z_
_Operation: ListJobTemplates | Class: maintainability_

> _Note: this issue was identified against the code base at commit `0c701d5f8ae664c771f5c5cdfd2b2b982b891619`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every other paged listing in this crate delegates ordering, cursor resumption and page sizing to one function in `src/state.rs` — `page_of_jobs`, `page_of_queues`, `page_of_presets`, `page_of_versions` — each documented as "ONE place that owns the ordering, the cursor and the page size, so a later paged reader cannot drift from this one". `ListJobTemplates` is the exception: its handler re-implements that same block inline (sort on `ListBySortKey::compare` over `ordering_key()`, reverse for DESCENDING, `position()` for the first record strictly past the decoded anchor, `usize::try_from(max_results.unwrap_or(DEFAULT_PAGE_SIZE))`, the `start.min(len)` / `page_size.min(len)` slice pair, and the `(start + page.len() < matching.len())` token rule).

The copies are line-for-line equivalent to `page_of_presets`, which differs from the handler's block only in the record type, so the two can drift: a fix to the anchor comparison, the page-size fallback or the token condition in `state.rs` silently leaves the job-template listing behind. The state module's own doc even describes `ListJobTemplates` as paging "the same way" without naming a shared function, so there is no single place that rule actually lives.

A fix is response-preserving: lift the block into the state module (one helper generic over a record exposing `ordering_key()`/`cursor()`, which `QueueRecord`, `PresetRecord`, `JobTemplateRecord` and `JobRecord` all already have) and call it from `list_job_templates.rs`. No response body and no existing test changes.

## Evidence

The duplicated block — services/service-com-amazonaws-mediaconvert-mediaconvert/src/ops/list_job_templates.rs:206-261:
```
matching.sort_by(|left, right| sort_key.compare(left.ordering_key(), right.ordering_key()));
... if !ascending { matching.reverse(); }
matching.iter().position(|record| { let side = sort_key.compare(record.ordering_key(), anchor); if ascending { side == Ordering::Greater } else { side == Ordering::Less } }).unwrap_or(matching.len())
let page_size = usize::try_from(input.max_results.unwrap_or(crate::state::DEFAULT_PAGE_SIZE)).unwrap_or(crate::state::DEFAULT_PAGE_SIZE as usize);
let page = &matching[start.min(matching.len())..];
let page = &page[..page_size.min(page.len())];
let next_token = (start + page.len() < matching.len()).then(|| page.last().map(|record| record.cursor())).flatten();
```

The copy that already owns the rule — src/state.rs:2598-2648 `page_of_presets` (same five steps, `PresetRecord` instead of `JobTemplateRecord`), documented at src/state.rs:2581-2593 as "The preset listing's own [`page_of_queues`]: ONE place that owns the ordering, the cursor and the page size"; `page_of_queues` is src/state.rs:2519-2570, `page_of_jobs` src/state.rs:2406-2458, `page_of_versions` src/state.rs:2746-2782.

The module doc that asserts the shared rule it does not have — src/state.rs:65-68:
```
//!   queries. `ListJobTemplates` orders the job templates the same way and pages them the
//!   same way, and `ListQueues` orders and pages the queues through [`page_of_queues`],
//!   over at most [`MAX_QUEUES`] records. `ListPresets` orders and pages the presets
//!   through [`page_of_presets`], over at most [`MAX_PRESETS`] records.
```

Tests that cover the behaviour and must stay green unchanged: tests/all/job_templates.rs (ordering, `category` narrowing, `maxResults`/`nextToken` paging — e.g. lines 333-352, 460-471, 622), tests/all/preset_listing.rs (lines 360-379, 485-496, 669) and tests/all/queue_listing.rs (line 227 onward).

## Steps to Reproduce

1. Open services/service-com-amazonaws-mediaconvert-mediaconvert/src/ops/list_job_templates.rs and read lines 206-261.
2. Open services/service-com-amazonaws-mediaconvert-mediaconvert/src/state.rs and read `page_of_presets` at lines 2598-2648.
3. Compare: the sort, the reverse, the anchor `position()`, the `max_results` fallback, the two-step slice and the next-token condition are the same logic twice, once in a handler and once in the state module.
4. Note src/state.rs:65-66 claims ListJobTemplates "pages them the same way" while naming a shared function only for ListQueues and ListPresets.
5. Run `cargo test -p service-com-amazonaws-mediaconvert-mediaconvert` before and after factoring the handler block into the state module: tests/all/job_templates.rs, preset_listing.rs and queue_listing.rs must pass unchanged.
