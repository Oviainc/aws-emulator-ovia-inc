Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 9c3b4064a1735206e464a3fb30963f5d1ed2eddb

# The mutating-plan admission rule (not-found, then in-progress) is implemented twice

_Filed: 2026-10-27T13:45:00Z_
_Operation: DeleteScalingPlan | Class: maintainability_

> _Note: this issue was identified against the code base at commit `9c3b4064a1735206e464a3fb30963f5d1ed2eddb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Both mutating handlers resolve the plan they are about to write with the same two-step admission rule: look the plan up by `(ScalingPlanName, ScalingPlanVersion)` and refuse with `ObjectNotFoundException` carrying `crate::state::plan_not_found_message(..)` when nothing matches, then refuse with `ConcurrentUpdateException` when `ScalingPlanRecord::is_in_progress()` — the state the operation docs describe as "You cannot update a scaling plan if it is in the process of being created, updated, or deleted".

The crate already centralises the two halves this rule is built from — `plan_mut`, `is_in_progress` and `plan_not_found_message` all live in `state.rs`, and `plan_not_found_message`'s own doc says "Every operation that looks a plan up by name and version refuses with this one text" — but the BRANCH that combines them is copy-pasted into the two handlers, so the copies can drift: a future change to the admission order, or to which statuses count as in progress, has to be made twice and nothing fails if only one copy is updated. The rule could live in `state.rs` as one helper that answers which of the two refusals applies (each handler still mints the refusal in its own error channel, with its own verb), leaving every response byte-identical.

## Evidence

Copy one — services/service-com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice/src/ops/delete_scaling_plan.rs:150-164:
```
        let plan = state
            .plan_mut(&input.scaling_plan_name, input.scaling_plan_version)
            .ok_or_else(|| {
                not_found(crate::state::plan_not_found_message(
                    &input.scaling_plan_name,
                    input.scaling_plan_version,
                ))
            })?;

        if plan.is_in_progress() {
            return Err(concurrent_update(format!(
                "The scaling plan '{}' is in status {} and cannot be deleted",
                input.scaling_plan_name, plan.status_code
            )));
        }
```

Copy two — services/service-com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice/src/ops/update_scaling_plan.rs:209-223:
```
        let plan = state
            .plan_mut(&input.scaling_plan_name, input.scaling_plan_version)
            .ok_or_else(|| {
                not_found(crate::state::plan_not_found_message(
                    &input.scaling_plan_name,
                    input.scaling_plan_version,
                ))
            })?;

        if plan.is_in_progress() {
            return Err(concurrent_update(format!(
                "The scaling plan '{}' is in status {} and cannot be updated",
                input.scaling_plan_name, plan.status_code
            )));
        }
```

The crate states that this is meant to be one rule — services/service-com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice/src/state.rs:1372-1382:
```
/// AWS's wording for "the name and version in this request name no stored plan",
/// built from the request's own `ScalingPlanName` and `ScalingPlanVersion`.
///
/// Every operation that looks a plan up by name and version refuses with this one
/// text, whatever error shape it models the refusal in ...
pub fn plan_not_found_message(name: &str, version: i64) -> String {
```
and services/service-com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice/src/state.rs:232-238:
```
    /// True while the plan is being created, updated or deleted — the state in which
    /// `UpdateScalingPlan` and `DeleteScalingPlan` answer `ConcurrentUpdateException`
    /// ("You cannot update a scaling plan if it is in the process of being created,
    /// updated, or deleted").
    pub fn is_in_progress(&self) -> bool {
```

Existing tests that pin both copies' responses (so a fold must leave them green): tests/all/scaling_plan_lifecycle.rs:773 `an_unknown_plan_or_version_answers_each_operations_own_envelope`, tests/all/scaling_plan_lifecycle.rs:536 `a_plan_with_a_pending_transition_is_reported_but_not_mutated`, and tests/all/plan_version_member_validation.rs:206 `the_operations_that_model_a_not_found_shape_still_answer_in_that_shape` (which asserts the exact sentence "No scaling plan found for scaling plan name: {name}, version: {version}" for both operations).

## Steps to Reproduce

1. Open src/ops/delete_scaling_plan.rs and read lines 150-164.
2. Open src/ops/update_scaling_plan.rs and read lines 209-223; the lookup, the not-found refusal and the in-progress refusal are the same branch, differing only in the verb in the message and the error channel they are minted in.
3. Confirm no shared helper exists: grep the crate for `is_in_progress` — it is defined in src/state.rs:236 and called only from those two handlers, each of which re-implements the surrounding refusal itself.
