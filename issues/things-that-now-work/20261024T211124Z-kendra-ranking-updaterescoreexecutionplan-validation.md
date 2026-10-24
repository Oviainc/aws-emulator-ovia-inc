Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: cccf3b90d06957168c9a7f70092f27610e8c4105

# CapacityUnits above the documented 1000 extra-unit ceiling is accepted and stored instead of being refused

_Filed: 2026-10-24T21:11:24Z_
_Operation: UpdateRescoreExecutionPlan | Class: validation_

> _Note: this issue was identified against the code base at commit `cccf3b90d06957168c9a7f70092f27610e8c4105`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CapacityUnitsConfiguration$RescoreCapacityUnits` is documented as "The amount of extra capacity for your rescore execution plan. A single extra capacity unit for a rescore execution plan provides 0.01 rescore requests per second. You can add up to 1000 extra capacity units." The Smithy model can only express the lower bound (`@range min: 0`, no max), so the published ceiling of 1000 is a service-side rule a handler has to enforce.

Expected: an UpdateRescoreExecutionPlan (and likewise a CreateRescoreExecutionPlan) asking for more than 1000 extra capacity units is refused — with the modeled `ServiceQuotaExceededException` that this operation declares (HTTP 402 in the model, this service's client-error status on the wire; its documentation is literally "You have exceeded the set limits for your Amazon Kendra Intelligent Ranking service. Please see Quotas …"), or with `ValidationException` — and the plan's stored capacity is left unchanged.

Actual: the handler stores whatever integer the request carries (`plan.capacity_units = Some(capacity_units)`), so a plan can be provisioned at `RescoreCapacityUnits: 2147483647` and DescribeRescoreExecutionPlan reads that value back with HTTP 200. As a consequence `ServiceQuotaExceededException`, which UpdateRescoreExecutionPlan declares, is unreachable on this operation: no code path in `update_rescore_execution_plan.rs` can ever construct it, even though capacity is the only quota-bearing input the operation accepts.

## Evidence

Spec: `smithy_query(query="shape", shape="com.amazonaws.kendraranking#CapacityUnitsConfiguration")` → member `RescoreCapacityUnits`, required, `range {min: 0}` and no max; `smithy_query(query="doc", shape="...CapacityUnitsConfiguration", member="RescoreCapacityUnits")` → "You can add up to 1000 extra capacity units." `OPERATIONS.md` lists `ServiceQuotaExceededException (HTTP 402)` among UpdateRescoreExecutionPlan's errors, and `smithy_query(query="doc", shape="...ServiceQuotaExceededException")` ties it to exceeding the service's set limits.
Handler: `src/ops/update_rescore_execution_plan.rs` lines ~252-257 — `if let Some(capacity_units) = input.capacity_units { plan.capacity_units = Some(capacity_units); }` with no upper-bound check; the only quota check anywhere in the crate is the plan-count one in `src/ops/create_rescore_execution_plan.rs` (`MAX_RESCORE_EXECUTION_PLANS`). `src/state.rs` defines no capacity ceiling constant.
Tests: `tests/all/plan_capacity_units.rs` not only fails to cover the ceiling, it pins the unbounded behaviour — the create table's row "the top of the integer the member is modelled as" submits `RescoreCapacityUnits: i32::MAX` and asserts the describe reports `i32::MAX` back (lines ~100-104); the update table's largest value is 7. No test asserts a refusal above 1000, and no test reaches `ServiceQuotaExceededException` on Update (`tests/all/wire_error_status.rs` only reaches it via the plan-count quota on Create).

## Steps to Reproduce

1. CreateRescoreExecutionPlan with Name="capacity-plan" and no CapacityUnits; wait until DescribeRescoreExecutionPlan reports Status ACTIVE
2. Call UpdateRescoreExecutionPlan with that Id and CapacityUnits={"RescoreCapacityUnits": 5000} (or 2147483647)
3. Observe HTTP 200 instead of a refusal (ServiceQuotaExceededException / ValidationException) for a value past the documented 1000 extra-unit ceiling
4. Call DescribeRescoreExecutionPlan with the same Id and observe CapacityUnits.RescoreCapacityUnits reported back as 5000, i.e. the out-of-range capacity was persisted
5. The same request shape on CreateRescoreExecutionPlan (Name + CapacityUnits={"RescoreCapacityUnits": 5000}) is likewise accepted
