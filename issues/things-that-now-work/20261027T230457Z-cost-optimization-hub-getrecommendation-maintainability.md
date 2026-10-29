Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: c56dd21dd1ebda0a2d29e841f8875a573d0c3ba4

# GetRecommendation re-implements the shared recommendation_view rule instead of calling it

_Filed: 2026-10-27T23:04:56Z_
_Operation: GetRecommendation | Class: maintainability_

> _Note: this issue was identified against the code base at commit `c56dd21dd1ebda0a2d29e841f8875a573d0c3ba4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate declares ONE place where "the caller's effective savingsEstimationMode + the rows this account may read" is computed — `ComAmazonawsCostoptimizationhubCostoptimizationhubserviceState::recommendation_view` in `src/state.rs` — and both list readers go through it. `GetRecommendation` is the third reader of the same catalog, and it open-codes the same two steps (resolve `effective_preferences(caller).savings_estimation_mode`, then select `visible_recommendations(caller)`) inside its own handler instead of calling the helper. The two copies compute the same thing today, so no response changes; the defect is that the rule now exists twice in this crate's own code and the copies can drift — a change to how a reader resolves the mode or narrows the visible rows (the enrollment/analysis-due gate, the organization scoping) applied in `recommendation_view` would silently leave `GetRecommendation` behind, even though its own module doc promises it serves "the same set `ListRecommendations` pages over".

## Evidence

Copy 1 — the shared rule, documented as the single place it is written:
`services/service-com-amazonaws-costoptimizationhub-costoptimizationhubservice/src/state.rs:1055`: "/// The one place the rule both recommendation readers apply is written"
`src/state.rs:1078-1087`:
```
        Ok(RecommendationView {
            savings_estimation_mode: self
                .effective_preferences(account_id)
                .savings_estimation_mode,
            rows: self
                .visible_recommendations(account_id)
                .into_iter()
                .filter(|row| filter.is_none_or(|filter| row.matches_filter(filter)))
                .collect(),
        })
```
and the readers that call it: `src/ops/list_recommendations.rs:209-212` (`state.lock().recommendation_view(crate::state::caller_account_id(), input.filter.as_ref())`), `src/ops/list_recommendation_summaries.rs:275-278` (same call), with `src/ops/list_recommendations.rs:202-206`: "The gate on `filter.accountIds`, the caller's effective savings estimation mode and the rows the request selects are the one rule both recommendation readers apply, written once beside the catalog (`crate::state::...::recommendation_view`)".

Copy 2 — the same two steps re-written in the third reader, `src/ops/get_recommendation.rs:203-216`:
```
        let (mode, row) = {
            let store = state.lock();
            let mode = store
                .effective_preferences(crate::state::caller_account_id())
                .savings_estimation_mode;
            let row = store
                .visible_recommendations(crate::state::caller_account_id())
                .into_iter()
                .find(|row| row.recommendation_id == input.recommendation_id);
            (mode, row)
        };
```
while its own header claims identity with the list view (`src/ops/get_recommendation.rs:5-9`): "Looks `recommendationId` up among the recommendations the calling account may read — the same set `ListRecommendations` pages over".

Test coverage that must stay unchanged by the fix: `tests/all/cost_optimization_hub.rs:919` `get_recommendation_agrees_with_the_list_entry`, `tests/all/cost_optimization_hub.rs:1038` `savings_estimation_mode_changes_the_reported_money`, `tests/all/shared_recommendation_view.rs:293` `neither_reader_reports_a_catalog_the_account_has_not_opted_into`, `tests/all/rejection_fidelity.rs:111` `a_missing_recommendation_is_identified_by_resource_id`.

## Steps to Reproduce

1. Read `src/state.rs:1052-1088` and note the doc sentence "The one place the rule both recommendation readers apply is written" over `recommendation_view`, which resolves the effective savingsEstimationMode and the account's visible rows.
2. Read `src/ops/list_recommendations.rs:209-212` and `src/ops/list_recommendation_summaries.rs:275-278`: both readers obtain mode+rows only through that helper.
3. Read `src/ops/get_recommendation.rs:203-216`: the same mode resolution and the same `visible_recommendations(caller)` selection are written again in the handler, bypassing the helper.
4. Confirm the duplication is behaviour-neutral today (`recommendation_view(caller, None)` returns the same mode and the same unfiltered rows), i.e. the gap is drift risk, not a wrong response.
