Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: c56dd21dd1ebda0a2d29e841f8875a573d0c3ba4

# The "take one beyond the page, cut, mint the token at offset+page.len()" walk is open-coded in seven paginated handlers

_Filed: 2026-10-27T23:05:22Z_
_Operation: ListAutomationRules | Class: maintainability_

> _Note: this issue was identified against the code base at commit `c56dd21dd1ebda0a2d29e841f8875a573d0c3ba4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Seven of this crate's paginated handlers each write out the same three-step page walk by hand: take `page_size + 1` items from the selected sequence, use the presence of the extra item as the has-more signal, cut the page to `page_size`, and mint a `PageCursor { offset: offset + page.len(), filter_fingerprint }` only when the extra item was there. The crate already factored the neighbouring rules of the same walk into `state` — `page_size` (the default/floor), `encode_page_token`/`decode_page_token` and `PageCursor` — but the walk itself stayed duplicated, so the seven copies can drift from one another (an off-by-one in the cut, or a token minted from `offset + page_size` instead of `offset + page.len()`, would be a per-operation bug rather than a single-helper bug).

A single generic helper in `crate::state` taking the selected iterator, `offset`, `page_size` and the fingerprint, and answering `(Vec<T>, Option<String>)`, reproduces each copy exactly: every response body and every existing test stays unchanged.

## Evidence

Copy 1 — `src/ops/list_automation_rules.rs:214-232`:
```
        let matching: Vec<AutomationRule> = guard
            .rules
            .iter_in_order()
            .filter(|rule| crate::state::rule_matches_filters(rule, &filters))
            .skip(offset)
            .take(page_size.saturating_add(1))
            .map(crate::state::StoredRule::to_automation_rule)
            .collect();

        // One extra rule was taken: its presence is what says another page
        // exists, and the token resumes exactly where this page stopped.
        let has_more = matching.len() > page_size;
        let page: Vec<AutomationRule> = matching.into_iter().take(page_size).collect();
        let next_token = has_more.then(|| {
            crate::state::encode_page_token(&crate::state::PageCursor {
                offset: offset + page.len(),
                filter_fingerprint,
            })
        });
```
Copy 2 — `src/ops/list_recommended_actions.rs:234-252`, the identical walk over another store:
```
        let matching: Vec<RecommendedAction> = guard
            .recommended_actions
            .iter_in_order()
            .filter(|action| crate::state::recommended_action_matches_filters(action, &filters))
            .skip(offset)
            .take(page_size.saturating_add(1))
            .cloned()
            .collect();

        // One extra action was taken: its presence is what says another page
        // exists, and the token resumes exactly where this page stopped.
        let has_more = matching.len() > page_size;
        let page: Vec<RecommendedAction> = matching.into_iter().take(page_size).collect();
        let next_token = has_more.then(|| {
            crate::state::encode_page_token(&crate::state::PageCursor {
                offset: offset + page.len(),
                filter_fingerprint,
            })
        });
```
The same block appears five more times: `src/ops/list_automation_events.rs:252-277`, `src/ops/list_automation_event_summaries.rs:344-375`, `src/ops/list_recommended_action_summaries.rs:265-287`, `src/ops/list_automation_rule_preview.rs:270-290` and `src/ops/list_automation_rule_preview_summaries.rs:288-312` — the last four even repeat the same comment, e.g. `list_recommended_action_summaries.rs:262-264`: "The page walk is the same one every paginated list of this service performs — one group beyond the page is what says another page exists, and the token resumes exactly where this page stopped."

The neighbouring rules of the same walk are already single-sourced, which is the precedent for this one — `src/state.rs:3547-3561`:
```
/// How many items one page of a paginated listing serves, from the caller's
/// `maxResults`.
///
/// One rule for every paginated operation of this service: ...
pub fn page_size(max_results: Option<i32>) -> usize {
```
Tests that pin the behaviour the fix must not change: `tests/all/page_size_rule.rs` (`every_paginated_listing_sizes_its_page_by_the_shared_rule`, line 364; `a_page_larger_than_the_default_is_served_where_the_model_admits_one`, line 430 — both walk every paginated operation page by page) and `tests/all/page_token_refusals.rs` (`a_minted_token_resumes_its_own_query_and_is_refused_by_any_other`, line 276).

## Steps to Reproduce

1. Read `src/ops/list_automation_rules.rs:214-232` and `src/ops/list_recommended_actions.rs:234-252`: the same take(page_size+1) / has_more / cut / mint-at-offset+page.len() sequence, character for character apart from the item type and the store it reads.
2. Find the same block in `src/ops/list_automation_events.rs:252-277`, `src/ops/list_automation_event_summaries.rs:344-375`, `src/ops/list_recommended_action_summaries.rs:265-287`, `src/ops/list_automation_rule_preview.rs:270-290` and `src/ops/list_automation_rule_preview_summaries.rs:288-312` — seven copies in all.
3. Note that `crate::state` already owns the other halves of this one walk (`page_size`, `PageCursor`, `encode_page_token`, `decode_page_token`), so the duplicated part is the walk itself.
4. Confirm `tests/all/page_size_rule.rs` and `tests/all/page_token_refusals.rs` drive every one of those operations over the wire, so a single shared helper is verifiable without changing any response or test.
