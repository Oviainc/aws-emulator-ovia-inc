Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 8ae1b3d406553f92eef0619abe1571efd9650c6e

# DescribeBudget never reports a CalculatedSpend, so a stored budget reads back without the actual/forecasted spend AWS always returns

_Filed: 2026-10-24T03:08:44Z_
_Operation: DescribeBudget | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `8ae1b3d406553f92eef0619abe1571efd9650c6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Real AWS Budgets computes `Budget.CalculatedSpend` itself and reports it on every read: a freshly created budget comes back with `CalculatedSpend.ActualSpend` set to zero in the budget's unit (and usually a `ForecastedSpend` too), because `CalculatedSpend$ActualSpend` is a @required member of the shape and the service tracks spend for every budget it holds.

The emulator computes nothing here. `DescribeBudget` projects the stored budget through `crate::state::project_budget` (services/service-com-amazonaws-budgets-awsbudgetservicegateway/src/state.rs:271), which derives only the planned-limit `BudgetLimit`; neither the write path (`stamp_service_owned`, src/state.rs:203) nor the read path ever materialises a `CalculatedSpend`. So:

* a budget created without `CalculatedSpend` (the normal case — no SDK sends it) reads back with the member entirely absent, where AWS returns `{"ActualSpend":{"Amount":"0","Unit":"USD"}}`;
* a client that keys on `Budget.CalculatedSpend.ActualSpend.Amount` (the console view, any alerting/reporting script, `aws budgets describe-budget` output parsing) sees a missing field instead of a number.

Expected: every read path (`DescribeBudget`, and `DescribeBudgets` which shares `project_budget`) reports a service-computed `CalculatedSpend` whose `ActualSpend` is the spend this emulator has recorded for the budget — zero, in the budget's own unit, when no usage has been attributed. Actual: the member is omitted from the response.

This is distinct from the already-filed CreateBudget/wrong-behavior issue, which is about a caller-SUPPLIED `CalculatedSpend` surviving the write path and about the RI/SP `BudgetLimit` default; the gap here is that the READ reports no spend at all for the ordinary budget that was created without one.

## Evidence

Spec: Smithy shape `CalculatedSpend` — `ActualSpend` is @required, documented as "The amount of cost, usage, RI units, or Savings Plans units that you used"; `Budget$CalculatedSpend` is "The actual and forecasted cost or usage that the budget tracks" (a service-computed member, which `UpdateBudget` documents as one a caller cannot change). Well-established AWS behaviour: `aws budgets describe-budgets` / `describe-budget` responses always carry `CalculatedSpend.ActualSpend` (amount 0 for a budget with no usage yet).
Handler: src/ops/describe_budget.rs:171-193 reads the stored budget and returns `crate::state::project_budget(stored, now)`; `project_budget` (src/state.rs:271-279) only fills `BudgetLimit` from `PlannedBudgetLimits`. `StoredBudget` (src/state.rs:40) holds the caller's `Budget` verbatim, and `stamp_service_owned` (src/state.rs:203-225) stamps only LastUpdatedTime / TimePeriod / CostTypes / HealthStatus.
Tests: no test in tests/all/ mentions CalculatedSpend (grep across tests/all/budget_lifecycle.rs, budget_read_projection.rs, client_error_envelopes.rs, state_roundtrip.rs returns nothing), so nothing asserts either the present or the absent behaviour.

## Steps to Reproduce

1. POST / with `X-Amz-Target: AWSBudgetServiceGateway.CreateBudget` and body {"AccountId":"123456789012","Budget":{"BudgetName":"spend-read","TimeUnit":"MONTHLY","BudgetType":"COST","BudgetLimit":{"Amount":"100","Unit":"USD"}}} — 200.
2. POST / with `X-Amz-Target: AWSBudgetServiceGateway.DescribeBudget` and body {"AccountId":"123456789012","BudgetName":"spend-read"} — the response `Budget` object carries no `CalculatedSpend` member at all; AWS returns `CalculatedSpend.ActualSpend` = {"Amount":"0","Unit":"USD"}.
3. Same observation through `DescribeBudgets` for that account: each listed budget is missing `CalculatedSpend`.
