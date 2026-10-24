Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: ba63228bbc1032132c1fbdbb0b54a8c844ce19f7

# Deletion is synchronous: the modeled DELETING status can never be observed after a delete

_Filed: 2026-10-24T03:42:27Z_
_Operation: DeleteRescoreExecutionPlan | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ba63228bbc1032132c1fbdbb0b54a8c844ce19f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`RescoreExecutionPlanStatus` models five values — CREATING, UPDATING, ACTIVE, DELETING, FAILED — and both `DescribeRescoreExecutionPlanResponse$Status` and `RescoreExecutionPlanSummary$Status` report that enum. Plan provisioning on this service is asynchronous, which is exactly why the emulator already models CREATING -> ACTIVE and UPDATING -> ACTIVE transitions (`crate::state::TRANSIENT_STATUS_MILLIS`). Deletion is treated differently: the handler removes the plan from `plans`/`arn_index` inside the same call, so the instant the delete returns 200 the plan is gone.

Expected: a delete of an ACTIVE plan starts an asynchronous teardown — the plan reports Status DELETING to DescribeRescoreExecutionPlan and appears in ListRescoreExecutionPlans with that status for the teardown window, and only afterwards does the id stop resolving (ResourceNotFoundException). A second delete / an update / a Rescore during that window is refused, as it already is for the other transient statuses.

Actual: DescribeRescoreExecutionPlan immediately after a successful delete answers ResourceNotFoundException, the plan vanishes from the listing in the same breath, and `DELETING` is a value this emulator can never put on the wire (the crate defines STATUS_CREATING / STATUS_ACTIVE / STATUS_UPDATING constants and no DELETING constant at all).

## Evidence

Spec: `RescoreExecutionPlanStatus` enum values include `DELETING` (`smithy_query(query="shape", shape="RescoreExecutionPlanStatus")`), reported by `DescribeRescoreExecutionPlanResponse$Status`; the same status machine the emulator already honours for CREATING/UPDATING.
Handler: `src/ops/delete_rescore_execution_plan.rs` lines ~221-227 — `store.plans.remove(&id); store.arn_index.remove(&arn); store.client_tokens.retain(..)` all under the one call, with no transient status set via `begin_transition`.
State: `src/state.rs` declares `STATUS_ACTIVE`, `STATUS_CREATING`, `STATUS_UPDATING` — no DELETING constant exists, so no code path can report it.
Tests: `tests/all/rescore_execution_plan_lifecycle.rs:180-197` pins the current synchronous behaviour (describe after delete = 400 not-found, plan absent from the listing); nothing asserts a DELETING window.

## Steps to Reproduce

1. CreateRescoreExecutionPlan, wait until DescribeRescoreExecutionPlan reports ACTIVE
2. Call DeleteRescoreExecutionPlan with that Id (200)
3. Immediately call DescribeRescoreExecutionPlan with the same Id
4. Observe ResourceNotFoundException instead of a plan whose Status is DELETING; also note ListRescoreExecutionPlans no longer lists the plan at all
