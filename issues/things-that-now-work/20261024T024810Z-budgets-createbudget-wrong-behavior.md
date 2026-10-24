Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: ceffded050eaaeab66f25b25154c54549736906e

# CreateBudget stores caller-supplied service-owned budget members and skips the documented RI/Savings-Plans BudgetLimit default

_Filed: 2026-10-24T02:48:10Z_
_Operation: CreateBudget | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ceffded050eaaeab66f25b25154c54549736906e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The write path stamps only some of the members the service owns (`stamp_service_owned`, src/state.rs:203): LastUpdatedTime, TimePeriod, CostTypes, HealthStatus. Three service-owned values are left exactly as the caller sent them, so a later DescribeBudget/DescribeBudgets reports caller data where AWS reports its own:

1. `Budget.CalculatedSpend` — "The actual and forecasted cost or usage that the budget tracks", and UpdateBudget documents calculatedSpend as a member a caller cannot change. A CreateBudget that supplies CalculatedSpend has it persisted verbatim and echoed by every read, so the emulator reports an invented actual/forecasted spend for a budget that has no usage behind it.
2. `Budget.AutoAdjustData.LastAutoAdjustTime` ("The last time that your budget was auto-adjusted") and `AutoAdjustData.HistoricalOptions.LookBackAvailablePeriods` ("You can't set your own LookBackAvailablePeriods. The value is automatically calculated from the BudgetAdjustmentPeriod and your historical cost data") are stored from the request and never computed, and no BudgetLimit is derived for an auto-adjusting budget.
3. `Budget.BudgetLimit` for an RI/Savings-Plans budget — Budget$BudgetLimit: "RI and Savings Plans utilization or coverage budgets default to 100. This is the only valid value for RI or Savings Plans utilization or coverage budgets." The handler applies no such default, so a `RI_UTILIZATION` / `RI_COVERAGE` / `SAVINGS_PLANS_UTILIZATION` / `SAVINGS_PLANS_COVERAGE` budget created without BudgetLimit reads back with no BudgetLimit at all, whereas AWS reports 100 (percent).

Expected: caller values for CalculatedSpend / LastAutoAdjustTime / LookBackAvailablePeriods are discarded on the write path the way a caller-supplied LastUpdatedTime already is, and an RI/SP utilization or coverage budget is stored with the documented default limit of 100. Actual: caller values survive and the RI/SP default is missing.

## Evidence

Spec: Budget$BudgetLimit ("RI and Savings Plans utilization or coverage budgets default to 100"), Budget$CalculatedSpend, UpdateBudget doc ("except for the budgetName and the calculatedSpend"), AutoAdjustData$LastAutoAdjustTime, HistoricalOptions$LookBackAvailablePeriods ("You can't set your own").
Handler: src/ops/create_budget.rs:245-271 stores `input.budget` after `crate::state::stamp_service_owned`, which (src/state.rs:203-225) touches only last_updated_time, time_period, cost_types and health_status; `project_budget` (src/state.rs:271) derives only the planned-limit BudgetLimit on read.
Tests: tests/all/budget_read_projection.rs covers TimePeriod/CostTypes/HealthStatus/LastUpdatedTime/planned limits; no test mentions CalculatedSpend, AutoAdjustData or an RI/SP budget created without BudgetLimit.

## Steps to Reproduce

1. CreateBudget {"AccountId":"123456789012","Budget":{"BudgetName":"echoed-spend","TimeUnit":"MONTHLY","BudgetType":"COST","BudgetLimit":{"Amount":"100","Unit":"USD"},"CalculatedSpend":{"ActualSpend":{"Amount":"777","Unit":"USD"}}}} then DescribeBudget — the emulator returns the caller's CalculatedSpend of 777 USD.
2. CreateBudget a budget with BudgetType "RI_UTILIZATION", TimeUnit "MONTHLY" and no BudgetLimit, then DescribeBudget — the emulator returns no BudgetLimit; AWS returns BudgetLimit Amount 100.
3. CreateBudget with AutoAdjustData={"AutoAdjustType":"HISTORICAL","HistoricalOptions":{"BudgetAdjustmentPeriod":3,"LookBackAvailablePeriods":42},"LastAutoAdjustTime":0} then DescribeBudget — the emulator echoes LookBackAvailablePeriods 42 and the caller's LastAutoAdjustTime, both of which the service owns.
