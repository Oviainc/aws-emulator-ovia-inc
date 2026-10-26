Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: b3c7a1d3b5e40780898598751dac20435d474802

# The paginated page walk is implemented twice, once per list operation

_Filed: 2026-10-25T16:27:42Z_
_Operation: ListClusters | Class: maintainability_

> _Note: this issue was identified against the code base at commit `b3c7a1d3b5e40780898598751dac20435d474802`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListClusters` and `ListStreams` carry two hand-copied copies of the same page-bounds rule: decode-or-refuse the cursor, convert-and-clamp `maxResults` against a per-operation default plus the same 1..100 bounds, filter the BTreeMap entries after the cursor, `take(page_size + 1)`, truncate, and emit a cursor only when the extra item appeared. The only genuine differences are the modelled default (20 vs 10), the map being walked and the summary struct built. The crate already centralises the cursor's two halves (`crate::state::encode_pagination_cursor` / `decode_pagination_cursor` / `unissued_pagination_token`), so the page walk itself is the one part left duplicated, and the duplicated `MAX_RESULTS_MIN`/`MAX_RESULTS_MAX` constants let the two copies drift (a bound fixed in one list would silently stay wrong in the other). Extracting the walk — the clamp plus the "one past the page" token rule, parameterised by the default and a key/summary mapping — leaves every response and every existing test unchanged.

## Evidence

Copy 1, `services/service-com-amazonaws-dsql-dsql/src/ops/list_clusters.rs:185-211` and `:223-227`:
```
        let page_size = usize::try_from(input.max_results)
            .unwrap_or(MAX_RESULTS_DEFAULT)
            .clamp(MAX_RESULTS_MIN, MAX_RESULTS_MAX);
...
            // One past the page, so the token is emitted only when something remains.
            .take(page_size + 1)
...
        let next_token = (page.len() > page_size).then(|| {
            page.truncate(page_size);
            crate::state::encode_pagination_cursor(&page[page_size - 1].identifier)
        });
```
```
const MAX_RESULTS_DEFAULT: usize = 20;
const MAX_RESULTS_MIN: usize = 1;
const MAX_RESULTS_MAX: usize = 100;
```

Copy 2, `services/service-com-amazonaws-dsql-dsql/src/ops/list_streams.rs:188-228` and `:240-244`:
```
        let page_size = usize::try_from(input.max_results)
            .unwrap_or(MAX_RESULTS_DEFAULT)
            .clamp(MAX_RESULTS_MIN, MAX_RESULTS_MAX);
...
            // One past the page, so the token is emitted only when something remains.
            .take(page_size + 1)
...
        let next_token = (page.len() > page_size).then(|| {
            page.truncate(page_size);
            crate::state::encode_pagination_cursor(&page[page_size - 1].stream_identifier)
        });
```
```
const MAX_RESULTS_DEFAULT: usize = 10;
const MAX_RESULTS_MIN: usize = 1;
const MAX_RESULTS_MAX: usize = 100;
```
The cursor halves are already shared (`list_clusters.rs:229-231`: "The pagination cursor this handler issues and reads back is the one every paginated reader of this service shares").

Tests that pin the behaviour the refactor must not change: `tests/all/cluster_lifecycle.rs:600 list_pages_through_every_cluster_exactly_once`, `:689 an_unissued_next_token_is_refused`, `:734 max_results_is_enforced_at_its_published_bounds`; `tests/all/stream_listing.rs:121 list_pages_through_every_stream_of_the_cluster_exactly_once`, `:209 max_results_is_enforced_at_its_published_bounds`, `:259 an_unissued_next_token_is_refused`; `tests/all/pagination_token_refusal.rs:224`, `:246`, `:265`.

## Steps to Reproduce

1. Read services/service-com-amazonaws-dsql-dsql/src/ops/list_clusters.rs:185-211 and its constants at :223-227.
2. Read services/service-com-amazonaws-dsql-dsql/src/ops/list_streams.rs:188-228 and its constants at :240-244.
3. Observe the same clamp, the same `take(page_size + 1)` / truncate / cursor-emission rule and the same 1..100 bounds written twice, differing only in the modelled default and the summary built.
4. Confirm the behaviour is covered by the tests named in the evidence, so the duplication can be collapsed without any response or test changing.
