Issue-Severity: HIGH
Issue-Gap-Class: validation
Issue-Commit: ceffded050eaaeab66f25b25154c54549736906e

# CreateBudget accepts budgets the model's documented rules reject (missing BudgetLimit, end date past 06/15/87, bad AutoAdjustData / PlannedBudgetLimits combinations)

_Filed: 2026-10-24T02:48:10Z_
_Operation: CreateBudget | Class: validation_

> _Note: this issue was identified against the code base at commit `ceffded050eaaeab66f25b25154c54549736906e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::validate_budget` (services/service-com-amazonaws-budgets-awsbudgetservicegateway/src/state.rs:481) enforces exactly three cross-member rules: BudgetLimit-vs-PlannedBudgetLimits, FilterExpression/Metrics-vs-CostFilters/CostTypes, and Start<End. Several other rules the Smithy model documents on the `Budget` shape are not enforced, so CreateBudget answers 200 and persists a budget that real AWS Budgets refuses with `InvalidParameterException` (HTTP 400):

1. `BudgetLimit` omitted for a `COST` or `USAGE` budget. Budget$BudgetLimit: "BudgetLimit is required for cost or usage budgets, but optional for RI or Savings Plans utilization or coverage budgets." The handler accepts a `COST`/`USAGE` budget with neither BudgetLimit nor PlannedBudgetLimits; DescribeBudget then reports a limit-less cost budget, which cannot exist in AWS.
2. `TimePeriod.End` after 06/15/87 00:00 UTC. Budget$TimePeriod: "The end date must come before 06/15/87 00:00 UTC." Only Start<End is checked, so an End of e.g. 2099-01-01 is stored.
3. `AutoAdjustData` combinations. AutoAdjustData$AutoAdjustType is `HISTORICAL | FORECAST`; HistoricalOptions describes the historical window an auto-adjusting budget is based on, and HistoricalOptions$BudgetAdjustmentPeriod's documented maximum depends on the budget's TimeUnit ("For the DAILY granularity, the maximum value is 60 ... MONTHLY 12 ... QUARTERLY 4 ... ANNUALLY 1"). The handler never looks at `budget.auto_adjust_data`, so a `HISTORICAL` budget with no HistoricalOptions, a `FORECAST` budget carrying HistoricalOptions, and a MONTHLY budget with BudgetAdjustmentPeriod=60 are all accepted.
4. `PlannedBudgetLimits` on a TimeUnit that does not support it. Budget$PlannedBudgetLimits: "PlannedBudgetLimits is available for cost or usage budget and supports both monthly and quarterly TimeUnit", with keys that are the start of the month/quarter in epoch seconds beginning at the current period. A DAILY or ANNUALLY budget with PlannedBudgetLimits, and a map whose keys are not period starts, are both stored as-is (`planned_limit_in_force` silently skips unparseable keys).
5. Subscriber addresses supplied through `NotificationsWithSubscribers` are not checked against their `SubscriptionType` (an `EMAIL` subscriber whose Address is not an email address, an `SNS` subscriber whose Address is not an SNS topic ARN). AWS refuses these at create time with InvalidParameterException; the handler stores them verbatim (create_budget.rs:251-263).

Expected: each of the above is refused with `InvalidParameterException` (HTTP 400, a modeled error of CreateBudget) and nothing is persisted. Actual: HTTP 200 and the invalid budget becomes readable through DescribeBudget/DescribeBudgets.

## Evidence

Spec: Smithy shape `Budget` member docs (BudgetLimit "required for cost or usage budgets"; TimePeriod "The end date must come before 06/15/87 00:00 UTC"; PlannedBudgetLimits "available for cost or usage budget and supports both monthly and quarterly TimeUnit"), `HistoricalOptions$BudgetAdjustmentPeriod` per-TimeUnit maxima, `AutoAdjustData`. CreateBudget declares `InvalidParameterException` (HTTP 400).
Handler: src/ops/create_budget.rs:194-198 calls `crate::state::validate_budget`, whose entire rule set is src/state.rs:481-511 — three rules only; `auto_adjust_data`, `budget_type`-vs-`budget_limit`, the 06/15/87 end bound, the TimeUnit/PlannedBudgetLimits pairing and subscriber addresses are never inspected.
Tests: tests/all/budget_lifecycle.rs:1238 (`mutually_exclusive_budget_members_are_refused`) covers only the two mutual-exclusion rules; no test asserts any of the rules above (grep for AutoAdjustData in tests/ returns nothing).

## Steps to Reproduce

1. POST / with X-Amz-Target: AWSBudgetServiceGateway.CreateBudget and body {"AccountId":"123456789012","Budget":{"BudgetName":"no-limit","TimeUnit":"MONTHLY","BudgetType":"COST"}} — emulator answers 200; AWS answers InvalidParameterException (BudgetLimit is required for a COST budget).
2. CreateBudget with Budget.TimePeriod.End = 4102444800 (2100-01-01) — emulator answers 200 and DescribeBudget reports that End; AWS answers InvalidParameterException (end date must come before 06/15/87 00:00 UTC).
3. CreateBudget with Budget.AutoAdjustData={"AutoAdjustType":"HISTORICAL"} (no HistoricalOptions), and again with {"AutoAdjustType":"FORECAST","HistoricalOptions":{"BudgetAdjustmentPeriod":3}}, and again with TimeUnit MONTHLY plus HistoricalOptions.BudgetAdjustmentPeriod=60 — all answer 200.
4. CreateBudget with TimeUnit DAILY and a PlannedBudgetLimits map — answers 200.
5. CreateBudget with NotificationsWithSubscribers[0].Subscribers=[{"SubscriptionType":"EMAIL","Address":"not-an-email"}] — answers 200.
