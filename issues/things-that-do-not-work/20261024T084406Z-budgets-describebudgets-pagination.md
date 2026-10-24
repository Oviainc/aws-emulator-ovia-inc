Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 1f773e23b3e0bd2b9ce8ba90d57212de869e61a4

# DescribeBudgets ignores the documented default page size of 100 and serves every budget in one page

_Filed: 2026-10-24T08:44:05Z_
_Operation: DescribeBudgets | Class: pagination_

> _Note: this issue was identified against the code base at commit `1f773e23b3e0bd2b9ce8ba90d57212de869e61a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DescribeBudgetsRequest$MaxResults` is documented in the Smithy model as "An integer that represents how many budgets a paginated response contains. The default is 100." AWS therefore caps an unbounded DescribeBudgets call at 100 budgets and returns a `NextToken` while more remain.

The emulator treats an absent `MaxResults` as "every remaining item": `crate::state::page()` sets `page_size = items.len()` when `max_results` is `None`, so a `DescribeBudgets` call with no `MaxResults` against an account holding, say, 150 budgets answers with all 150 `Budgets` and NO `NextToken`.

Expected (AWS): first page holds 100 budgets (ordered), plus a `NextToken`; the second page holds the remaining 50 and no token.
Actual (emulator): a single page with all 150 budgets and no `NextToken`, so a client that relies on the service-side default never paginates and sees a page size AWS cannot produce.

Note this only affects `DescribeBudgets`: the other paginated reads in this slice (`DescribeNotificationsForBudget`, `DescribeSubscribersForNotification`) document `MaxResults` as "an optional integer" with no default, so "everything remaining" is a defensible answer there.

## Evidence

Spec: smithy_query(query="doc", shape="DescribeBudgetsRequest", member="MaxResults") → "An integer that represents how many budgets a paginated response contains. The default is 100."; the member is also the `inputToken`-paired page size of the operation's @paginated trait.
Handler: services/service-com-amazonaws-budgets-awsbudgetservicegateway/src/ops/describe_budgets.rs:175-184 calls `crate::state::page(&budgets, input.max_results, ...)`; src/state.rs `page()` ("`max_results` ... absent means \"every remaining item\"", `None => items.len()`) returns the whole list and mints no token.
Tests: tests/all/budget_lifecycle.rs:657-702 (`pagination_partitions_the_account_in_budget_name_order`) exercises page sizes [None, 1, 2, 3, 7, 1000] but only over totals of 0/1/2/5/13 budgets — never more than 100 — and `walk_budget_pages` skips the page-size assertion entirely when `max_results` is `None`, so no test asserts the documented default.

## Steps to Reproduce

1. CreateBudget 150 times for account 123456789012 with distinct BudgetNames (e.g. budget-000 … budget-149), each MONTHLY/COST with a BudgetLimit.
2. POST / with X-Amz-Target: AWSBudgetServiceGateway.DescribeBudgets and body {"AccountId":"123456789012"} (no MaxResults).
3. Observe the response: the emulator returns all 150 entries in `Budgets` and no `NextToken`; AWS returns 100 entries plus a `NextToken`.
4. Follow the token AWS would return: in the emulator there is none, so the remaining budgets are only reachable by re-reading the whole list.
