Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 0d4ec79976a0b91eeb5a23059b2beecdecae287a

# The MaxResults page-size rule is open-coded in all four paginated handlers

_Filed: 2026-10-27T15:37:22Z_
_Operation: ListChangeSets | Class: maintainability_

> _Note: this issue was identified against the code base at commit `0d4ec79976a0b91eeb5a23059b2beecdecae287a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every other rule the list operations share is implemented once in `crate::state` and called from each handler — `filter_predicate` for `Filter`, `sorts_descending` for `SortOrder`, `decode_page_token`/`paginate` for the page token, `invalid_sort_by` for `SortBy` — each with a comment stating that it is "the one place that decision is made". The page-size decision (is the requested `MaxResults` inside the modelled range, which `ValidationException` to raise when it is not, and which page to serve when the member is absent) is instead written out inline four times, once per paginated handler, each with its own copy of the range test, its own message `format!` and its own `usize::try_from(..).expect(..)`. Four copies of one rule can drift: a change to how an out-of-range page size is refused has to be made in four places, and `ListChangeSets`/`ListEntities` additionally duplicate the "absent member serves the largest page the model allows" branch.

A single helper in `crate::state` taking the operation label, the noun and the limit reproduces every current message and page byte-for-byte, so responses and tests stay unchanged.

## Evidence

Copy 1 — src/ops/list_change_sets.rs:257-283:
```
        let page_size = match input.max_results {
            None => crate::state::LIST_CHANGE_SETS_PAGE_LIMIT,
            Some(requested)
                if (1..=crate::state::LIST_CHANGE_SETS_PAGE_LIMIT).contains(&requested) => { requested }
            Some(requested) => { return Err(... crate::state::validation("MaxResults", format!("ListChangeSets serves between 1 and {} change sets a page, not {requested}", ...))) }
        };
        let page_size = usize::try_from(page_size).expect("a page size in 1..=20 fits in usize");
```
Copy 2 — src/ops/list_entities.rs:354-379 (identical structure, `LIST_ENTITIES_PAGE_LIMIT`, "ListEntities serves between 1 and {} entities a page", `expect("a page size in 1..=50 fits in usize")`).
Copy 3 — src/ops/list_assessments.rs:206-225 (`if !(1..=crate::state::LIST_ASSESSMENTS_PAGE_LIMIT).contains(&input.max_results) { ... }` + `usize::try_from(input.max_results).expect("a page size in 1..=100 fits in usize")`).
Copy 4 — src/ops/describe_assessment.rs:235-255 (same, `DESCRIBE_ASSESSMENT_PAGE_LIMIT`).

The contrasting single-place rule, src/state.rs:147-151:
```
/// ... This is the one place that decision is made, so every list operation judges the same `Filter` shape the
/// same way: `operation` is the list being called and `filtered_on` the
/// attributes it publishes.
pub fn filter_predicate(
```
and src/state.rs:132-136: "This is the one place that decision is made, so each list operation turns a `SortOrder` into a direction the same way".

Test coverage that must stay green and unchanged: tests/all/request_validation.rs:1110-1177 (`a_page_size_outside_the_modelled_range_is_refused_as_a_client_error`) exercises all four operations with 0, -1, maximum+1 and i32::MAX plus both published bounds; tests/all/assessments.rs:337-525 and tests/all/change_set_lifecycle.rs:1047-1140 cover the served page sizes and token paging.

## Steps to Reproduce

1. Read src/ops/list_change_sets.rs:257-283, src/ops/list_entities.rs:354-379, src/ops/list_assessments.rs:206-225 and src/ops/describe_assessment.rs:235-255 side by side: the same range test, refusal and usize conversion appear four times
2. Compare with src/state.rs:132-176, where the sibling `SortOrder` and `Filter` rules are each implemented once and documented as "the one place that decision is made"
3. Run the service tests: tests/all/request_validation.rs::a_page_size_outside_the_modelled_range_is_refused_as_a_client_error covers all four copies, so collapsing them into one helper is verifiable without changing any response
