Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4ffec8ed9c134267b14eefb2b325da9a0803e3ce

# Describe omits CapacityUnits for a plan created with the default capacity instead of reporting zero

_Filed: 2026-10-24T00:50:04Z_
_Operation: DescribeRescoreExecutionPlan | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4ffec8ed9c134267b14eefb2b325da9a0803e3ce`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A rescore execution plan created without the optional `CapacityUnits` member is stored with `capacity_units: None` and `DescribeRescoreExecutionPlan` therefore omits `CapacityUnits` from its response body entirely.

The model states how the real service reports this case: `DescribeRescoreExecutionPlanResponse$CapacityUnits` is documented as "The capacity units set for the rescore execution plan. A capacity of zero indicates that the rescore execution plan is using the default capacity." A default-capacity plan is therefore reported as a PRESENT `CapacityUnits` structure whose required `RescoreCapacityUnits` is `0` — the value is what carries the "using the default capacity" meaning — not as an absent member. `CreateRescoreExecutionPlanRequest$CapacityUnits` matches that reading ("You are given a single capacity unit by default. If you want to use the default capacity, you don't set additional capacity units"): not setting the member means zero ADDITIONAL units, which is a capacity configuration the plan has, not the absence of one.

Expected: create a plan without `CapacityUnits`, then describe it -> `CapacityUnits.RescoreCapacityUnits == 0`.
Actual: the response has no `CapacityUnits` key at all, so a client reading `response['CapacityUnits']['RescoreCapacityUnits']` (or an SDK field that is non-optional in its own typing) sees nothing where AWS reports 0, and cannot distinguish "default capacity" from "field not reported".

The same omission reaches a plan whose create set no capacity and whose later `UpdateRescoreExecutionPlan` calls also carried none.

## Evidence

Spec: `DescribeRescoreExecutionPlanResponse$CapacityUnits` doc — "The capacity units set for the rescore execution plan. A capacity of zero indicates that the rescore execution plan is using the default capacity."; `CapacityUnitsConfiguration$RescoreCapacityUnits` is @required with @range(min 0); `CreateRescoreExecutionPlanRequest$CapacityUnits` doc — "You are given a single capacity unit by default. If you want to use the default capacity, you don't set additional capacity units."

Handler: services/.../src/ops/create_rescore_execution_plan.rs stores `capacity_units: input.capacity_units` verbatim (no default applied), and services/.../src/ops/describe_rescore_execution_plan.rs returns `capacity_units: plan.capacity_units.clone()`, i.e. `None` -> member omitted. `crate::state::RescoreExecutionPlan::capacity_units` is `Option<CapacityUnitsConfiguration>`.

Tests: no test covers describe of a plan created WITHOUT `CapacityUnits` — tests/all/rescore_execution_plan_lifecycle.rs always creates with `CapacityUnits: {RescoreCapacityUnits: n}` and asserts that value back, and tests/all/plan_update.rs's cases all start from `create_full_plan` which sets capacity 1. Nothing asserts the absent-member behaviour, so this is not a test-enshrined choice.

## Steps to Reproduce

1. CreateRescoreExecutionPlan {Name: "default-capacity-plan"} (no CapacityUnits member) -> 200 with Id/Arn.
2. Poll DescribeRescoreExecutionPlan {Id} until Status == ACTIVE.
3. Inspect the Describe response body: there is no `CapacityUnits` key.
4. Expected (per the modeled member doc): `CapacityUnits: {"RescoreCapacityUnits": 0}`, zero meaning the plan uses the default capacity.
