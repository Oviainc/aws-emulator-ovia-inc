Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: f2163892bf40cea6da7d1662f28c14ec75c797c6

# Fold the duplicated page-size default/floor rule into one helper

_Filed: 2026-10-26T23:23:22Z_
_Operation: ListAccounts | Class: maintainability_

> _Note: this issue was identified against the code base at commit `f2163892bf40cea6da7d1662f28c14ec75c797c6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The same page-size computation — "default to `DEFAULT_MAX_RESULTS` when the caller sent no `maxResults`, convert to `usize`, and floor at 1 so a zero page cannot produce a non-advancing token" — is written out independently in all nine paginated handlers of this crate, and the copies have ALREADY drifted into two different spellings: six use `.max(1)` and three use `.clamp(1, max_page_size)` with a locally recomputed `max_page_size`. A tenth copy or a changed default has to be edited in nine places, and the state module's doc for the shared constant is already wrong because of the same scattering (it asserts a bound that does not hold for `ListAccounts`).

The fix is one `crate::state` helper that every handler calls. It changes no response: the three `clamp(1, max_page_size)` sites all target operations whose `maxResults` carries `@range(min: 1, max: 100)`, which the generated request walk enforces before the handler runs (as those handlers' own comments state), so `clamp(1, 100)` and `.max(1)` are the same function on every value that can reach them; and `ListAccounts`, whose model range is `1..=1000`, keeps the `.max(1)` behaviour it has today.

## Evidence

The `.max(1)` copy, written six times — e.g.
`src/ops/list_accounts.rs:195-201`
```
        let page_size = usize::try_from(
            input
                .max_results
                .unwrap_or(crate::state::DEFAULT_MAX_RESULTS),
        )
        .unwrap_or(1)
        .max(1);
```
Byte-identical at `src/ops/list_automation_rules.rs:204-210`, `src/ops/list_automation_events.rs:230-236`, `src/ops/list_automation_event_steps.rs:204-210`, `src/ops/list_automation_event_summaries.rs:289-295`, `src/ops/list_recommended_actions.rs:225-231`.

The drifted `clamp` copy, written three times — e.g.
`src/ops/list_recommended_action_summaries.rs:238-245`
```
        let max_page_size = usize::try_from(crate::state::DEFAULT_MAX_RESULTS).unwrap_or(1);
        let page_size = usize::try_from(
            input
                .max_results
                .unwrap_or(crate::state::DEFAULT_MAX_RESULTS),
        )
        .unwrap_or(1)
        .clamp(1, max_page_size);
```
Identical at `src/ops/list_automation_rule_preview.rs:247-254` and `src/ops/list_automation_rule_preview_summaries.rs:252-259`.

The drift has already corrupted the shared constant's documentation — `src/state.rs:112-116`:
```
/// The page size a paginated list uses when the caller sends no `maxResults`.
/// The model bounds every `maxResults` member at `range(min: 1, max: 100)`
/// (`ListAutomationRules`, `ListAutomationRulePreview`, …), so the largest page
/// a caller may ask for is also the default.
pub const DEFAULT_MAX_RESULTS: i32 = 100;
```
which `src/ops/list_accounts.rs:190-191` contradicts in the same crate:
```
        // The model bounds this operation's `maxResults` at 1..=1000, so a
        // caller that sends none gets `crate::state::DEFAULT_MAX_RESULTS`.
```
(`smithy_query(query="operation",operation="ListAccounts")` gives `"maxResults": {"target": "Integer", "range": {"min": 1, "max": 1000}}`.)

Why the clamp sites are equivalent to the floor: `src/ops/list_recommended_actions.rs:216-224` states the shared premise — "`maxResults` carries a member `@range` of min 1 / max 100 ..., which the generated request walk enforces BEFORE this handler runs — a request for 1000 ... is refused ... and never arrives here."

Tests that cover these nine call sites and must keep passing unchanged: `tests/all/account_listing.rs::the_listing_pages_over_one_stable_sequence`, `tests/all/account_listing.rs::the_modelled_max_results_range_is_enforced` (pins `maxResults: 1000` -> HTTP 200 for `ListAccounts`), `tests/all/automation_rule_lifecycle.rs::list_pages_over_the_stored_rules`, `tests/all/recommended_actions.rs::pages_over_the_recorded_actions`, `tests/all/recommended_action_summaries.rs::paging_walks_every_group_once_for_every_page_size`, `tests/all/automation_rule_preview.rs::{paging_walks_the_whole_selection_once_for_every_page_size, max_results_honours_its_modelled_bounds}`, `tests/all/automation_rule_preview_summaries.rs::{paging_walks_every_group_once_for_every_page_size, max_results_honours_its_modelled_bounds}`, `tests/all/page_token_refusals.rs`.

## Steps to Reproduce

1. `grep -n 'unwrap_or(crate::state::DEFAULT_MAX_RESULTS)' services/service-com-amazonaws-computeoptimizerautomation-computeoptimizerautomationservice/src/ops/*.rs` — nine independent copies of the same default/convert/floor computation.
2. `grep -n '\.max(1);\|\.clamp(1, max_page_size);' services/.../src/ops/*.rs` — the copies have already split into two spellings of one rule (six `.max(1)`, three `.clamp(1, max_page_size)`).
3. Read `src/state.rs:112-116` next to `src/ops/list_accounts.rs:190-191`: the constant's doc claims a model bound (max 100 for every `maxResults`) that the handler beside it and the Smithy model both contradict for `ListAccounts` (1..=1000).
4. Confirm no response can change: the three clamping operations all carry `@range(min: 1, max: 100)` on `maxResults` (`smithy_query(query="operation",operation="ListRecommendedActionSummaries,ListAutomationRulePreview,ListAutomationRulePreviewSummaries")`), enforced before the handler, so `clamp(1, 100)` == `max(1)` on every reachable value.
