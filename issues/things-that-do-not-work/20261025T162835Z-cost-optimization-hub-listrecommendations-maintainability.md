Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: b3c7a1d3b5e40780898598751dac20435d474802

# The organization accountIds gate and the visible+filtered row computation are duplicated in the two recommendation readers

_Filed: 2026-10-25T16:28:35Z_
_Operation: ListRecommendations | Class: maintainability_

> _Note: this issue was identified against the code base at commit `b3c7a1d3b5e40780898598751dac20435d474802`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListRecommendations` and `ListRecommendationSummaries` each hold their own copy of the same locked block: refuse a populated `filter.accountIds` from a caller that is not the organization's management account, then resolve the effective `savingsEstimationMode`, then collect `visible_recommendations(caller)` narrowed by `row.matches_filter(filter)`. The two copies are character-for-character the same rule (only the error constructor differs, and the message is already shared through `crate::service::organization_view_denied`), so a change to the gate — which members count as the organization view, or how the rows are narrowed — has to be made twice and can drift between the two readers. The responses, the error shapes and every existing test stay unchanged if the block is extracted into one helper (e.g. beside `visible_recommendations` in `src/state.rs`) that both handlers call, each mapping the refusal into its own modelled error.

## Evidence

Copy 1 — services/service-com-amazonaws-costoptimizationhub-costoptimizationhubservice/src/ops/list_recommendations.rs:202-232:
```
        let (mode, mut rows) = {
            let store = state.lock();
...
            if input
                .filter
                .as_ref()
                .and_then(|filter| filter.account_ids.as_ref())
                .is_some_and(|account_ids| !account_ids.is_empty())
                && !store.is_management_account(crate::state::caller_account_id())
            {
                return Err(not_an_organization_view());
            }

            let mode = store
                .effective_preferences(crate::state::caller_account_id())
                .savings_estimation_mode;
            let rows: Vec<crate::state::StoredRecommendation> = store
                .visible_recommendations(crate::state::caller_account_id())
                .into_iter()
                .filter(|row| {
                    input
                        .filter
                        .as_ref()
                        .is_none_or(|filter| row.matches_filter(filter))
                })
                .collect();
```
Copy 2 — src/ops/list_recommendation_summaries.rs:262-296:
```
        let (mode, rows) = {
            let store = state.lock();
...
            if input
                .filter
                .as_ref()
                .and_then(|filter| filter.account_ids.as_ref())
                .is_some_and(|account_ids| !account_ids.is_empty())
                && !store.is_management_account(crate::state::caller_account_id())
            {
                return Err(not_an_organization_view());
            }

            let mode = store
                .effective_preferences(crate::state::caller_account_id())
                .savings_estimation_mode;
...
            let rows: Vec<crate::state::StoredRecommendation> = store
                .visible_recommendations(crate::state::caller_account_id())
                .into_iter()
                .filter(|row| {
                    input
                        .filter
                        .as_ref()
                        .is_none_or(|filter| row.matches_filter(filter))
                })
                .collect();
```
The summaries copy even states it is the same rule — list_recommendation_summaries.rs:265-268: "an organization view, which only the organization's management account has — the same gate `ListRecommendations` applies to the same filter member."

Existing coverage that must keep passing unchanged: tests/all/management_account_gating.rs:62-137 (`an_account_ids_filter_is_refused_to_a_non_management_account`, 403 for ListRecommendations and the served control without the member), tests/all/recommendation_summaries.rs:463-512 and tests/all/summary_metrics_and_refusals.rs:135-189 (the summary filters, whose comments note `accountIds` is "the organization view this caller does not have"), and the filter table in tests/all/cost_optimization_hub.rs:581 onward.

## Steps to Reproduce

1. Open services/service-com-amazonaws-costoptimizationhub-costoptimizationhubservice/src/ops/list_recommendations.rs at lines 202-232.
2. Open src/ops/list_recommendation_summaries.rs at lines 262-296 and compare: the accountIds gate, the effective-mode read and the visible+matches_filter collection are the same logic written twice.
3. Change the gate in one file only (e.g. treat an empty `accountIds` as the organization view) and observe the two readers now disagree about the same filter member, with no test forcing them back into agreement.
