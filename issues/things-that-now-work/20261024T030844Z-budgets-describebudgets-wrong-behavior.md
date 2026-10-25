Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 8ae1b3d406553f92eef0619abe1571efd9650c6e

# Budgets whose TimePeriod.End has passed are still listed and readable instead of being gone

_Filed: 2026-10-24T03:08:44Z_
_Operation: DescribeBudgets | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `8ae1b3d406553f92eef0619abe1571efd9650c6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`Budget$TimePeriod` documents the end of a budget's life: "After the end date, Amazon Web Services deletes the budget and all the associated notifications and subscribers." A budget whose `TimePeriod.End` lies in the past therefore does not appear in `DescribeBudgets` on the real service, and `DescribeBudget` / `DescribeNotificationsForBudget` on it answer `NotFoundException`.

The emulator holds budgets forever. `CreateBudget` accepts any `End` as long as it comes after `Start` (`crate::state::validate_budget`, src/state.rs:498-509 — the only TimePeriod rule is Start < End), and the read paths never consult `End`: `budgets_for_account` (src/state.rs:342-358) returns every stored budget for the account, and `project_budget` (src/state.rs:271) copies the stored shape unchanged. So a budget created with, say, Start 2020-01-01 / End 2021-01-01 is listed indefinitely, keeps its notifications, and counts against the account's budget quota.

Expected: a budget past its `TimePeriod.End` is no longer part of the account's state — it is absent from `DescribeBudgets`, and `DescribeBudget` / `DescribeNotificationsForBudget` / `DeleteBudget` for it answer `NotFoundException`. Materialising that on read is the clock making a deletion due, not a read inventing state. Actual: the expired budget is reported like any other.

## Evidence

Spec: Smithy `Budget$TimePeriod` documentation — "After the end date, Amazon Web Services deletes the budget and all the associated notifications and subscribers" (the same sentence the emulator's own src/state.rs:8-11 quotes when it explains that notifications die with the budget).
Handler: src/ops/describe_budgets.rs:169-200 lists `crate::state::budgets_for_account(&store, &input.account_id, now)`; that helper (src/state.rs:342-358) filters only on AccountId, and nothing in src/state.rs compares `TimePeriod.End` against the current time. src/ops/describe_budget.rs:171-193 and src/ops/describe_notifications_for_budget.rs:179-208 resolve the stored budget the same way.
Tests: tests/all/budget_lifecycle.rs and tests/all/budget_read_projection.rs only create budgets with future or defaulted end dates (the default `06/15/87 00:00 UTC`); no test asserts anything about a budget whose end date has passed.

## Steps to Reproduce

1. CreateBudget {"AccountId":"123456789012","Budget":{"BudgetName":"already-ended","TimeUnit":"CUSTOM","BudgetType":"COST","BudgetLimit":{"Amount":"1","Unit":"USD"},"TimePeriod":{"Start":1577836800,"End":1609459200}}} — 200 (Start 2020-01-01, End 2021-01-01, both in the past).
2. DescribeBudgets {"AccountId":"123456789012"} — the expired budget is still listed; AWS no longer holds it.
3. DescribeBudget {"AccountId":"123456789012","BudgetName":"already-ended"} — 200 with the budget; expected NotFoundException because the budget and its notifications were deleted after the end date.
