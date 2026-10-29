Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 1f1c94accd1d7961aa84a7b9995986ca278396b6

# The default/clamp page-size resolution for the 100-entry listings is implemented four times in four handlers, and the copies have already drifted

_Filed: 2026-10-27T12:42:50Z_
_Operation: DescribeBudgetActionsForAccount | Class: maintainability_

> _Note: this issue was identified against the code base at commit `1f1c94accd1d7961aa84a7b9995986ca278396b6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule "a request that names no `MaxResults` is served at most 100 entries, and no page may exceed 100" is resolved independently in four operation handlers instead of in the one shared pagination home (`crate::state::page`, whose own doc defines where that resolution belongs). Two of the copies are byte-identical computations over a privately duplicated constant; the other two implement the same rule with a different expression, which is the drift this duplication invites:

- `describe_budget_actions_for_account.rs`: `input.max_results.unwrap_or(ACTIONS_MAX_PAGE).clamp(1, ACTIONS_MAX_PAGE)`
- `describe_budget_actions_for_budget.rs`: the same expression over its own `ACTIONS_MAX_PAGE`
- `describe_budget_action_histories.rs`: `input.max_results.unwrap_or(ACTION_HISTORIES_MAX_PAGE)` — no clamp
- `describe_budget_performance_history.rs`: `input.max_results.unwrap_or(PERFORMANCE_HISTORY_MAX_PAGE)` — no clamp

All four resolve the same `MaxResults` shape (`@range(min: 1, max: 100)`) against the same ceiling of 100, so the four copies must agree; two of them already say the rule differently. A single helper beside `crate::state::page` (the module that already documents "resolved by the calling handler against whatever default its own `MaxResults` member documents") would hold it once. Every response stays identical — the clamp is a no-op for values the boundary validator already admits — and no existing test changes.

## Evidence

Copy 1 — services/service-com-amazonaws-budgets-awsbudgetservicegateway/src/ops/describe_budget_actions_for_account.rs:145 and :210-217:
    const ACTIONS_MAX_PAGE: i32 = 100;
    ...
        let (page, next_token) = crate::state::page(
            &actions,
            Some(
                input
                    .max_results
                    .unwrap_or(ACTIONS_MAX_PAGE)
                    .clamp(1, ACTIONS_MAX_PAGE),
            ),

Copy 2 — src/ops/describe_budget_actions_for_budget.rs:149 and :220-227: the same constant and the same `.unwrap_or(ACTIONS_MAX_PAGE).clamp(1, ACTIONS_MAX_PAGE)` expression.

Drifted copies of the same rule — src/ops/describe_budget_action_histories.rs:155,243:
    const ACTION_HISTORIES_MAX_PAGE: i32 = 100;
    ... Some(input.max_results.unwrap_or(ACTION_HISTORIES_MAX_PAGE)),
and src/ops/describe_budget_performance_history.rs:153,306:
    const PERFORMANCE_HISTORY_MAX_PAGE: i32 = 100;
    ... Some(input.max_results.unwrap_or(PERFORMANCE_HISTORY_MAX_PAGE)),

The shared home that already owns the rest of paging — src/state.rs:1502-1510 (`page` doc): "`max_results` is the EFFECTIVE page size: the modelled `@paginated` page size (already range-checked at the boundary), resolved by the calling handler against whatever default its own `MaxResults` member documents."

Test coverage that must stay unchanged: tests/all/budget_actions_for_account.rs:384 `an_absent_max_results_is_capped_at_the_page_the_output_list_holds` and :429 `bad_tokens_page_sizes_and_accounts_are_refused`; tests/all/budget_actions_for_budget.rs:335 `max_results_partitions_the_budgets_actions_in_creation_order` and :430 `a_token_this_list_never_minted_and_a_page_size_out_of_range_are_refused`; tests/all/budget_action_history.rs:775-830 (page walk over `MaxResults` including none at all); tests/all/budget_performance_history.rs:551-600; tests/all/default_page_sizes.rs:262 `an_absent_max_results_is_served_the_page_size_the_model_documents`.

## Steps to Reproduce

1. Open src/ops/describe_budget_actions_for_account.rs:210-217 and src/ops/describe_budget_actions_for_budget.rs:220-227 — the page-size resolution and its 100-entry constant are the same computation written twice.
2. Open src/ops/describe_budget_action_histories.rs:243 and src/ops/describe_budget_performance_history.rs:306 — the same rule, same ceiling, written without the clamp, so the four copies already disagree about how the rule is expressed.
3. Confirm the shared home exists: src/state.rs:1511 `pub fn page<T: Clone>(items, max_results, next_token)` with the doc at :1502-1510 that defines who resolves the effective page size.
4. Re-run the cited tests after folding the resolution into one helper: every response body and every assertion is unchanged, since the clamp is a no-op for the values `MaxResults`' `@range(1,100)` admits.
