Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 8ff612b8d27f8140aa4881a7fd23caef0de9fa96

# validate_budget decides which budget syntax was used by member PRESENCE, so an empty PlannedBudgetLimits / Metrics / CostFilters collection is counted as a selection and refuses a budget the crate's own rule accepts

_Filed: 2026-10-28T19:22:54Z_
_Operation: CreateBudget | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `8ff612b8d27f8140aa4881a7fd23caef0de9fa96`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::validate_budget` (run by `CreateBudget` and `UpdateBudget` before anything is stored) and `crate::state::cost_types_apply` decide "did the caller supply this limit / this filter syntax?" with `Option::is_some()`, i.e. on whether the member is PRESENT, never on whether it actually selects anything. The very next helper in the same validation chain, `validate_budget_limit_present`, decides the same question for the same member with `is_some_and(|planned| !planned.is_empty())` — and the crate's own test states that reason in words. The two rules therefore disagree about one request, and the disagreement turns requests the crate itself considers well-formed into `InvalidParameterException`, and silently drops a member a COST budget is documented to have.

Three concrete consequences, all from the same root cause:

1. `BudgetLimit` beside an EMPTY `PlannedBudgetLimits` map (`{}`) is refused at state.rs:1793 with "BudgetLimit can't be used with PlannedBudgetLimits" — although by the crate's own rule that empty map "carries no limit", so there is nothing for `BudgetLimit` to be used *with*. The request supplies exactly one limit, which is the shape the mutual-exclusion rule exists to enforce.
2. An empty `Metrics` list (`[]`) — a value the model explicitly admits, since `Budget$Metrics` is `@length(min: 0, max: 1)` — makes `expression_filters` true at state.rs:1801, so a plain legacy-syntax COST budget carrying `CostTypes` (or `CostFilters`) beside `Metrics: []` is refused with "FilterExpression and Metrics can't be used with CostFilters and CostTypes", although no metric and no filter expression was selected. The same holds for an empty `CostFilters` map making `legacy_filters` true against a `FilterExpression` budget.
3. A COST budget that sends `Metrics: []` and nothing else for scoping makes `cost_types_apply` false (state.rs:586-589), so `settle_cost_types` sets `budget.cost_types = None` (state.rs:608-610) and every later read of that COST budget reports NO `CostTypes` at all — the opposite of what that helper exists to do, and of `Budget$CostTypes` ("The types of costs that are included in this COST budget"), which says only the non-COST budget types have none.

The fix is the one the crate already uses one helper later: treat an empty collection as no selection (`!is_empty()`) in the presence tests at state.rs:1793, 1800 and 1801 and in `cost_types_apply`, so one rule about "did the caller supply this" is applied the same way everywhere in this file.

## Evidence

Side A — the presence tests, services/service-com-amazonaws-budgets-awsbudgetservicegateway/src/state.rs:1792-1808 (reached from src/ops/create_budget.rs:211 `crate::state::validate_budget(&input.budget)` and src/ops/update_budget.rs:197):
    pub fn validate_budget(budget: &Budget) -> Result<(), String> {
        if budget.budget_limit.is_some() && budget.planned_budget_limits.is_some() {
            return Err(
                "Unable to create/update budget: BudgetLimit can't be used with \
                 PlannedBudgetLimits."
                    .to_string(),
            );
        }
        let legacy_filters = budget.cost_filters.is_some() || budget.cost_types.is_some();
        let expression_filters = budget.filter_expression.is_some() || budget.metrics.is_some();
        if legacy_filters && expression_filters {
            return Err(
                "Unable to create/update budget: FilterExpression and Metrics can't be used with \
                 CostFilters and CostTypes."
                    .to_string(),
            );
        }

Side B — the SAME question about the SAME member, decided on emptiness, two helpers later in the same chain, src/state.rs:2076-2080:
    let planned = budget
        .planned_budget_limits
        .as_ref()
        .is_some_and(|planned| !planned.is_empty());
    if budget.budget_limit.is_some() || planned || budget.auto_adjust_data.is_some() {
        return Ok(());
    }

The crate states that rule in words, tests/all/budget_validation_rules.rs:271-283:
    // An EMPTY planned map carries no limit, so a cost budget is not satisfied by it.
    harness
        .check(
            "COST with an empty PlannedBudgetLimits",
            &json!({
                "BudgetName": "empty-plan",
                "TimeUnit": "MONTHLY",
                "BudgetType": "COST",
                "PlannedBudgetLimits": {}
            }),
            Verdict::Refused,
        )

The dropped-CostTypes half — src/state.rs:586-590 and :607-613:
    fn cost_types_apply(budget: &Budget) -> bool {
        budget.budget_type == COST_BUDGET_TYPE
            && budget.filter_expression.is_none()
            && budget.metrics.is_none()
    }
    ...
    fn settle_cost_types(budget: &mut Budget) {
        if !cost_types_apply(budget) {
            budget.cost_types = None;
            return;
        }

and that function's own doc, src/state.rs:583-585, which names the condition it means to detect — a budget actually WRITTEN in the other syntax, not one carrying an empty list:
    /// (`CreateBudget`), so a budget written in the `FilterExpression`/`Metrics` syntax has
    /// none either — defaulting them there would store exactly the combination that rule
    /// forbids (and which [`validate_budget`] refuses from a caller).

The model admits the empty list these branches misread — `smithy_query(query="shape",shape="Budget")`, member `Metrics`:
    "Metrics": {"target": "Metrics", "length": {"min": 0, "max": 1}, "type": "list", "doc": "<p>The definition for how the budget data is aggregated.</p>"}
and `Budget$CostTypes`: "The types of costs that are included in this <code>COST</code> budget. <code>USAGE</code>, <code>RI_UTILIZATION</code>, <code>RI_COVERAGE</code>, <code>SAVINGS_PLANS_UTILIZATION</code>, and <code>SAVINGS_PLANS_COVERAGE</code> budgets do not have <code>CostTypes</code>."

Test coverage touching these rules (none of it exercises an empty collection except the one row quoted above, which asserts the opposite reading): tests/all/budget_validation_rules.rs:200 `a_cost_or_usage_budget_must_carry_a_limit`; tests/all/budget_rule_conjunction.rs:314-320 (the `BudgetLimit` beside a NON-empty planned map row); tests/all/budget_cost_types_by_type.rs:249-252 (`Metrics` with one entry); tests/all/budget_lifecycle.rs:1323-1354.

## Steps to Reproduce

1. Call CreateBudget with the caller's own AccountId and Budget = {"BudgetName":"empty-plan-and-limit","TimeUnit":"MONTHLY","BudgetType":"COST","BudgetLimit":{"Amount":"10","Unit":"USD"},"PlannedBudgetLimits":{}}. Expected: accepted — exactly one limit is supplied, and tests/all/budget_validation_rules.rs:271 states that an empty planned map "carries no limit". Observed: HTTP 400 InvalidParameterException "Unable to create/update budget: BudgetLimit can't be used with PlannedBudgetLimits." from src/state.rs:1793.
2. Call CreateBudget with Budget = {"BudgetName":"empty-metrics-legacy","TimeUnit":"MONTHLY","BudgetType":"COST","BudgetLimit":{"Amount":"10","Unit":"USD"},"CostTypes":{"IncludeTax":false},"Metrics":[]}. Expected: accepted — `Budget$Metrics` is @length(min: 0, max: 1), and an empty list selects no metric, so only one filter syntax is in use. Observed: HTTP 400 InvalidParameterException "FilterExpression and Metrics can't be used with CostFilters and CostTypes." from src/state.rs:1800-1807.
3. Call CreateBudget with Budget = {"BudgetName":"empty-metrics-alone","TimeUnit":"MONTHLY","BudgetType":"COST","BudgetLimit":{"Amount":"10","Unit":"USD"},"Metrics":[]}, then DescribeBudget for it. Expected: a COST budget reads back with CostTypes carrying the documented per-member defaults (src/state.rs:607-613 exists to do exactly that). Observed: the response carries no CostTypes member at all, because cost_types_apply returned false on `metrics.is_some()` (src/state.rs:589) and settle_cost_types cleared it (src/state.rs:609).
4. Repeat step 1 against UpdateBudget (NewBudget instead of Budget) on an existing budget: the same helper is called at src/ops/update_budget.rs:197, so the same refusal is answered.
