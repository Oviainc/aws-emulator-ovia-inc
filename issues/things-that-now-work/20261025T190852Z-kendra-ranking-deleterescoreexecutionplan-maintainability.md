Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: a2837dbdebef5b9a30be37f112a3c5b63a2de779

# The "a plan may only be mutated while ACTIVE" conflict rule is implemented twice, against this crate's single-source policy

_Filed: 2026-10-25T19:08:52Z_
_Operation: DeleteRescoreExecutionPlan | Class: maintainability_

> _Note: this issue was identified against the code base at commit `a2837dbdebef5b9a30be37f112a3c5b63a2de779`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule that a plan mid-transition cannot take a mutating call — settle the clock-owed status, compare it to `STATUS_ACTIVE`, and refuse with `ConflictException` carrying a sentence naming the plan, its status and `ACTIVE` — is written out twice, once in `DeleteRescoreExecutionPlan` and once in `UpdateRescoreExecutionPlan`. The two copies differ only in the verb of the message, so they can drift: a change to the predicate (for example also treating a pending transition as a conflict) or to the wording in one handler leaves the other stating something else. This is the one refusal in the crate that is NOT built in a single place: `src/state.rs` centralises every other one and says so explicitly (`plan_not_found`, `resource_unavailable`, `plan_not_active`, `document_member_choice`, `capacity_units_over_quota` are each documented as "the ONE place that message is built"), and `tests/all/arn_refusal_single_source.rs` exists specifically to pin that policy for the ARN-addressed refusal. Extracting the status check + message into one `crate::state` helper taking the verb leaves every response byte-identical, since no test asserts the conflict text.

## Evidence

Copy 1 — `src/ops/delete_rescore_execution_plan.rs:204-221`:
```
        plan.settle_status(now);
        ...
        if plan.status != crate::state::STATUS_ACTIVE {
            return Err(
                DeleteRescoreExecutionPlanErrorModeled::ConflictException(ConflictException {
                    message: Some(format!(
                        "Rescore execution plan {} is {} and can only be deleted while it is {}.",
                        plan.id, plan.status, crate::state::STATUS_ACTIVE
                    )),
                })
                .into(),
            );
        }
```
Copy 2 — `src/ops/update_rescore_execution_plan.rs:229-244`:
```
        plan.settle_status(now);
        ...
        if plan.status != crate::state::STATUS_ACTIVE {
            return Err(
                UpdateRescoreExecutionPlanErrorModeled::ConflictException(ConflictException {
                    message: Some(format!(
                        "Rescore execution plan {} is {} and can only be updated while it is {}.",
                        plan.id, plan.status, crate::state::STATUS_ACTIVE
                    )),
                })
                .into(),
            );
        }
```
The policy both copies sidestep — `src/state.rs:83-92`: "The modelled `ResourceNotFoundException` for a plan id the store does not hold — the ONE place that message is built, so every plan-lookup path states it identically", and `src/state.rs:119-125` already holds the sibling data-plane helper `plan_not_active()`.
`tests/all/arn_refusal_single_source.rs:3-11`: "The ARN-addressed not-found refusal, asserted to be ONE rule rather than one per operation … so a change made in one place can no longer leave the others stating something else."

Existing tests that cover both copies and must stay unchanged: `tests/all/wire_error_status.rs:139-152` (the `ConflictException` rows for the update and the delete of a plan left mid-update) and `tests/all/plan_update.rs:328` (a mutating call during the `UPDATING` window answers `ConflictException`). Neither asserts the message text, only the error code and status.

## Steps to Reproduce

1. Open `src/ops/delete_rescore_execution_plan.rs:204-221` and `src/ops/update_rescore_execution_plan.rs:229-244` side by side: the same `settle_status` + `status != STATUS_ACTIVE` + `ConflictException { message: format!("Rescore execution plan {} is {} and can only be <verb> while it is {}.") }` sequence appears in both.
2. Compare with `src/state.rs:83-125`, where every other refusal of this service (including the data-plane `plan_not_active`) is built in exactly one place and documented as such.
3. Change the predicate or the wording in one handler only (e.g. make a pending transition a conflict in the delete) and run `cargo test -p service-com-amazonaws-kendraranking-awskendrarerankingfrontendservice`: the update path keeps the old rule, with no test forcing the two back into agreement.
