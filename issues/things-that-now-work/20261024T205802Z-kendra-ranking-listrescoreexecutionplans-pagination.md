Issue-Severity: LOW
Issue-Gap-Class: pagination
Issue-Commit: 438e57f04f6559b0a435f22a577d0c7dbfc3b209

# ListRescoreExecutionPlans returns an unbounded page when MaxResults is omitted

_Filed: 2026-10-24T20:58:02Z_
_Operation: ListRescoreExecutionPlans | Class: pagination_

> _Note: this issue was identified against the code base at commit `438e57f04f6559b0a435f22a577d0c7dbfc3b209`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListRescoreExecutionPlans` is `@paginated` (inputToken/outputToken `NextToken`) and its `MaxResults` is modeled `@range(1, 50)`, i.e. a client may never ask for a page larger than 50 items. The real service applies its own default page size when `MaxResults` is omitted and hands back a `NextToken` for the remainder, so no single response ever exceeds the published page bound.

Expected: a request with no `MaxResults` returns at most the service's default page (bounded by the modeled maximum of 50) plus a `NextToken` when more plans exist.

Actual: the handler treats an absent `MaxResults` as `usize::MAX` (`let limit = input.max_results.map_or(usize::MAX, ...)`), so it returns EVERY plan in one response and omits `NextToken`. With the emulator's own ceiling of 100 plans (`MAX_RESCORE_EXECUTION_PLANS`), a caller can observe a 100-item single page — twice the largest page the model allows a client to request — and a client that pages only while `NextToken` is present never exercises pagination at all.

## Evidence

Spec: smithy_query/OPERATIONS.md for ListRescoreExecutionPlans — `@paginated(inputToken=NextToken, outputToken=NextToken)`, `MaxResults` integer `@range(min 1, max 50)`. Handler: services/.../src/ops/list_rescore_execution_plans.rs lines ~180-220 — `limit = usize::MAX` when `MaxResults` is absent, then `.take(limit.saturating_add(1))`, so `has_more` is always false and `next_token` is always omitted for a default request. State: `MAX_RESCORE_EXECUTION_PLANS = 100` in src/state.rs, so the store can hold more plans than one modeled page. Tests: tests/all/rescore_execution_plan_lifecycle.rs::a_paged_listing_visits_every_plan_exactly_once includes `(3, None)` and `(0, None)` rows but only asserts a page never EXCEEDS an explicitly supplied MaxResults; no test asserts a default page bound, so this is unasserted behavior.

## Steps to Reproduce

1. Create 60 rescore execution plans with CreateRescoreExecutionPlan
2. Call ListRescoreExecutionPlans with an empty body (no MaxResults, no NextToken)
3. Observe SummaryItems containing all 60 plans and no NextToken in the response, i.e. a page larger than the modeled MaxResults maximum of 50 and no pagination cursor
