Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4af528f360b03ab2b05e9ad5a75be6ebefdc29e9

# CostTypes supplied for a budget type that has no cost types (USAGE / RI_* / SAVINGS_PLANS_*) is stored verbatim and echoed by every read

_Filed: 2026-10-24T21:45:23Z_
_Operation: CreateBudget | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4af528f360b03ab2b05e9ad5a75be6ebefdc29e9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The `CostTypes` shape documents, verbatim: "USAGE, RI_UTILIZATION, RI_COVERAGE, SAVINGS_PLANS_UTILIZATION, and SAVINGS_PLANS_COVERAGE budgets do not have CostTypes." A budget of one of those five types therefore has no cost-types member at all on the real service: a request that carries one is either refused as an invalid parameter or the member is dropped, and `DescribeBudget` / `DescribeBudgets` report the budget WITHOUT `CostTypes`.

The emulator's write path only decides whether to DEFAULT the cost types, never whether the budget may hold them. `crate::state::stamp_service_owned` calls `apply_cost_type_defaults` only when `cost_types_apply(budget)` is true (COST budget, legacy filter syntax); when it is false the caller's `CostTypes` map is left exactly as sent and is persisted with the budget, so every later read reports it.

Expected: a `USAGE` / `RI_UTILIZATION` / `RI_COVERAGE` / `SAVINGS_PLANS_UTILIZATION` / `SAVINGS_PLANS_COVERAGE` budget reads back with no `CostTypes` member whatever the create request carried (or the create is refused with `InvalidParameterException`). Actual: the caller's map is stored and reported, so the read reports a cost-types selection for a budget type that cannot have one.

The same write path serves `UpdateBudget` (`NewBudget` goes through the same `stamp_service_owned`), so the fix belongs in the shared helper rather than in one handler. This is distinct from the already-fixed CreateBudget wrong-behavior issue about caller-supplied SERVICE-OWNED members (CalculatedSpend / LastAutoAdjustTime / LookBackAvailablePeriods / the RI-SP BudgetLimit default): that one is about members the service computes, this one is about a member the budget TYPE forbids.

## Evidence

Spec: `CostTypes` shape doc in the Smithy model — "USAGE, RI_UTILIZATION, RI_COVERAGE, SAVINGS_PLANS_UTILIZATION, and SAVINGS_PLANS_COVERAGE budgets do not have CostTypes" (the emulator quotes this sentence itself in src/state.rs beside `cost_types_apply`).
Handler: services/.../src/ops/create_budget.rs calls `crate::state::stamp_service_owned(&mut budget, now, None)` and then inserts `budget` unchanged; src/state.rs `stamp_service_owned` → `if cost_types_apply(budget) { apply_cost_type_defaults(...) }` and `cost_types_apply` returns false for any non-COST budget type — nothing clears `budget.cost_types` on that branch, and `validate_budget` does not refuse the combination either. `project_budget` copies the stored budget, so the read reports it.
Tests: tests/all/budget_read_projection.rs::a_read_reports_the_cost_types_and_health_status_the_service_stamped asserts the DIVERGENT behaviour explicitly — for every non-COST `BudgetType` the row comments "A value the caller sent is stored and reported whatever the type" and asserts the read equals the supplied map (lines 569-586). So no test asserts the documented rule; one asserts its opposite and will need updating with the fix.

## Steps to Reproduce

1. CreateBudget {"AccountId":"123456789012","Budget":{"BudgetName":"usage-with-cost-types","TimeUnit":"MONTHLY","BudgetType":"USAGE","BudgetLimit":{"Amount":"5","Unit":"Hrs"},"CostTypes":{"IncludeTax":false,"UseAmortized":true}}} — the emulator answers 200.
2. DescribeBudget {"AccountId":"123456789012","BudgetName":"usage-with-cost-types"} — the emulator returns the budget with CostTypes {"IncludeTax":false,"UseAmortized":true}; the real service reports a USAGE budget with no CostTypes member.
3. Repeat with BudgetType "RI_UTILIZATION" (and "RI_COVERAGE", "SAVINGS_PLANS_UTILIZATION", "SAVINGS_PLANS_COVERAGE") — same echo on DescribeBudget and DescribeBudgets.
4. UpdateBudget the same budget with NewBudget carrying a different CostTypes map — the emulator stores and reports the new map for a budget type that has no cost types.
