Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 564feb857476f3247a0693cb65aa2d44e3322d3a

# The page-window computation is implemented four times, once per list handler

_Filed: 2026-10-24T22:02:50Z_
_Operation: ListKeyspaces | Class: maintainability_

> _Note: this issue was identified against the code base at commit `564feb857476f3247a0693cb65aa2d44e3322d3a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All four `@paginated` handlers carry their own copy of the same page-window logic: a private `MAX_PAGE_SIZE` constant, the `max_results` fallback conversion, the `take(page_size + 1)` lookahead with `truncated`/`truncate`, and the `next_token` built from the last item of the page. The four copies are character-for-character the same rule over four different `BTreeMap`s, so a fix or change to the windowing (for example the lookahead or the final-page token rule) has to be made four times and can drift between operations. One helper in `crate::state` taking an iterator and a page size and returning `(page, truncated)` would leave every response byte-identical, since the only per-operation parts (the range start and the token encoding) stay in the handlers.

## Evidence

Copy 1 — `src/ops/list_keyspaces.rs:85` `const MAX_PAGE_SIZE: i32 = 1000;`, `:202-203` `let page_size = usize::try_from(input.max_results.unwrap_or(MAX_PAGE_SIZE)).unwrap_or(MAX_PAGE_SIZE as usize);`, `:225-246`:
```
// One page, plus one lookahead item: the token is present exactly when a keyspace
// follows the last one on this page, so the final page carries none.
... .take(page_size + 1).collect(),
let truncated = scanned.len() > page_size;
scanned.truncate(page_size);
let next_token = match truncated { true => scanned.last().map(...), false => None };
```
Copy 2 — `src/ops/list_tables.rs:89`, `:195-196`, `:227-248` ("// One page, plus one lookahead item: the token is present exactly when a table follows the last one on this page, so the final page carries none." + the identical four statements).
Copy 3 — `src/ops/list_types.rs:91`, `:206-207`, `:240-266` (same comment for a type, same four statements).
Copy 4 — `src/ops/list_tags_for_resource.rs:93`, `:198-199`, `:232-251` ("// One page, plus one lookahead entry ..." + the identical four statements).

Existing tests that pin the behaviour this consolidation must not change: `tests/all/list_keyspaces.rs:285 list_keyspaces_pages_the_account_in_one_total_order_for_any_page_size`, `tests/all/list_types.rs:294 list_types_pages_a_keyspace_in_one_total_order_for_any_page_size`, `tests/all/keyspace_table_lifecycle.rs:544 list_tables_pages_a_keyspace_in_one_total_order_for_any_page_size`, `tests/all/resource_tagging.rs:784 listing_pages_the_tag_set_in_one_total_order_for_any_page_size`.

## Steps to Reproduce

1. Open src/ops/list_keyspaces.rs:225-246, src/ops/list_tables.rs:227-248, src/ops/list_types.rs:240-266 and src/ops/list_tags_for_resource.rs:232-251 side by side — the lookahead/truncate/next_token sequence is the same four statements in each
2. Compare the four private `MAX_PAGE_SIZE` constants (list_keyspaces.rs:85, list_tables.rs:89, list_types.rs:91, list_tags_for_resource.rs:93) and the four identical `page_size` conversions (list_keyspaces.rs:202, list_tables.rs:195, list_types.rs:206, list_tags_for_resource.rs:198)
3. Run the four pagination tests named above after factoring the window into one helper: responses and tokens must be unchanged
