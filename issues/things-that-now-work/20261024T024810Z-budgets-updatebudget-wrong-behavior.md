Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: ceffded050eaaeab66f25b25154c54549736906e

# UpdateBudget lets the caller overwrite the service-owned CalculatedSpend instead of leaving/resetting it

_Filed: 2026-10-24T02:48:10Z_
_Operation: UpdateBudget | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ceffded050eaaeab66f25b25154c54549736906e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The UpdateBudget documentation in the model states: "Updates a budget. You can change every part of a budget except for the `budgetName` and the `calculatedSpend`. When you modify a budget, the `calculatedSpend` drops to zero until Amazon Web Services has new usage data to use for forecasting."

The handler replaces the whole stored budget with `NewBudget` (src/ops/update_budget.rs:196-205: `stored.budget = budget;` after `stamp_service_owned`, which touches only LastUpdatedTime, TimePeriod, CostTypes and HealthStatus). Consequences observable through DescribeBudget/DescribeBudgets:

* A `NewBudget.CalculatedSpend` supplied by the caller becomes the budget's reported spend — an unprivileged caller can set ActualSpend/ForecastedSpend to any amount, which AWS does not allow (calculatedSpend is explicitly not changeable).
* A budget that already carried a CalculatedSpend loses it (or keeps a stale caller value) rather than having it dropped to zero / recomputed by the service on modification.
* The same wholesale replacement discards other service-owned members that are not re-stamped, e.g. `AutoAdjustData.LastAutoAdjustTime` and `AutoAdjustData.HistoricalOptions.LookBackAvailablePeriods` ("You can't set your own LookBackAvailablePeriods. The value is automatically calculated"), which are taken from the request instead.

Expected: UpdateBudget ignores caller-supplied values for the service-owned members (CalculatedSpend, LastAutoAdjustTime, LookBackAvailablePeriods) and resets CalculatedSpend on modification, exactly as it already ignores a caller-supplied LastUpdatedTime. Actual: whatever the caller put in NewBudget is stored and read back.

## Evidence

Spec: UpdateBudget operation documentation ("You can change every part of a budget except for the budgetName and the calculatedSpend. When you modify a budget, the calculatedSpend drops to zero..."), Budget$CalculatedSpend ("The actual and forecasted cost or usage that the budget tracks"), HistoricalOptions$LookBackAvailablePeriods ("You can't set your own LookBackAvailablePeriods").
Handler: src/ops/update_budget.rs:196-205 — `let mut budget = input.new_budget; stamp_service_owned(...); stored.budget = budget;`. `stamp_service_owned` (src/state.rs:203-225) stamps LastUpdatedTime/TimePeriod/CostTypes/HealthStatus only; `calculated_spend` and `auto_adjust_data` are never normalised.
Tests: tests/all/budget_lifecycle.rs:1407 asserts a caller-supplied LastUpdatedTime is replaced — the analogous assertion for CalculatedSpend does not exist (grep for CalculatedSpend in tests/ returns nothing).

## Steps to Reproduce

1. CreateBudget a MONTHLY COST budget "spend-demo" for account 123456789012.
2. UpdateBudget with NewBudget identical plus "CalculatedSpend":{"ActualSpend":{"Amount":"999999","Unit":"USD"}}.
3. DescribeBudget for "spend-demo": the emulator reports CalculatedSpend.ActualSpend = 999999 USD, i.e. the caller changed a member AWS documents as not changeable; AWS reports the service's own (reset/recomputed) value.
