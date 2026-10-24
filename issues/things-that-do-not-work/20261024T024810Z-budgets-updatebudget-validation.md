Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: ceffded050eaaeab66f25b25154c54549736906e

# UpdateBudget accepts NewBudget values the model's documented rules reject

_Filed: 2026-10-24T02:48:10Z_
_Operation: UpdateBudget | Class: validation_

> _Note: this issue was identified against the code base at commit `ceffded050eaaeab66f25b25154c54549736906e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

UpdateBudget shares `crate::state::validate_budget` with CreateBudget (src/ops/update_budget.rs:176-180), so the same documented rules go unchecked on the update path and a previously valid budget can be mutated into a state AWS refuses:

* `NewBudget` for a `COST`/`USAGE` budget with neither BudgetLimit nor PlannedBudgetLimits (Budget$BudgetLimit: "BudgetLimit is required for cost or usage budgets") — accepted, and the stored budget loses its limit entirely because the handler replaces the whole budget.
* `NewBudget.TimePeriod.End` after 06/15/87 00:00 UTC (Budget$TimePeriod: "The end date must come before 06/15/87 00:00 UTC").
* `NewBudget.AutoAdjustData` with an AutoAdjustType/HistoricalOptions combination the model documents as invalid, or a `BudgetAdjustmentPeriod` above the per-TimeUnit maximum (HistoricalOptions$BudgetAdjustmentPeriod: DAILY 60, MONTHLY 12, QUARTERLY 4, ANNUALLY 1).
* `NewBudget.PlannedBudgetLimits` on a TimeUnit other than MONTHLY/QUARTERLY (Budget$PlannedBudgetLimits: "supports both monthly and quarterly TimeUnit"), or with keys that are not period starts in epoch seconds.

Expected: `InvalidParameterException` (HTTP 400, modeled for UpdateBudget) and the stored budget left untouched. Actual: HTTP 200 and the budget is overwritten with the invalid shape, which subsequent DescribeBudget calls report.

## Evidence

Spec: Budget member documentation as quoted (BudgetLimit required for cost/usage budgets; End before 06/15/87; PlannedBudgetLimits monthly/quarterly only; BudgetAdjustmentPeriod maxima per TimeUnit). UpdateBudget declares `InvalidParameterException` (HTTP 400).
Handler: src/ops/update_budget.rs:176-180 → src/state.rs:481-511, which checks only BudgetLimit-vs-PlannedBudgetLimits, FilterExpression/Metrics-vs-CostFilters/CostTypes and Start<End; the update path adds no rules of its own.
Tests: tests/all/budget_lifecycle.rs:1238 drives the two mutual-exclusion rules through both CreateBudget and UpdateBudget; nothing asserts the rules above for UpdateBudget.

## Steps to Reproduce

1. CreateBudget a MONTHLY COST budget "limit-drop" with BudgetLimit 100 USD for account 123456789012.
2. UpdateBudget {"AccountId":"123456789012","NewBudget":{"BudgetName":"limit-drop","TimeUnit":"MONTHLY","BudgetType":"COST"}} — emulator answers 200.
3. DescribeBudget "limit-drop": the budget now has no BudgetLimit, a state AWS rejects with InvalidParameterException at step 2.
4. Repeat with NewBudget.TimePeriod.End = 4102444800, with AutoAdjustData={"AutoAdjustType":"FORECAST","HistoricalOptions":{"BudgetAdjustmentPeriod":60}}, and with TimeUnit "ANNUALLY" plus PlannedBudgetLimits — all answer 200.
