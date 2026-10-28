Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 87b02e9e0446b34956525b9761a3171af51589ac

# UpdateJobExecution accepts a device update of an execution whose TIMED_OUT transition is already due, letting it finish SUCCEEDED after its timer expired

_Filed: 2026-10-28T13:31:01Z_
_Operation: UpdateJobExecution | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `87b02e9e0446b34956525b9761a3171af51589ac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateJobExecution` refuses an update of a FINISHED execution (`record.is_terminal()` -> 409) and refuses an illegal move (`record.transition_refusal(...)` -> 409). Both decisions read `record.status`, and the handler never materialises the clock-driven `TIMED_OUT` transition first, although this crate already reaches the helper that does it (`JobExecutionStore::time_out_due_executions`, called by `StartCommandExecution` via `try_admit_command_execution`) and the helper's own doc says "a handler calls this before it reads".

Consequently an execution whose step timer (`stepTimeoutInMinutes`) or in-progress timer expired long ago still holds a pending status in the store, so `is_terminal()` is false and `transition_refusal` sees `IN_PROGRESS`: the device's update is ACCEPTED, the status becomes e.g. `SUCCEEDED`, `version_number` is incremented and `last_updated_at` is stamped at `now`. The execution therefore finishes in the status the late device reported rather than in the `TIMED_OUT` the clock already decided, and the 409 `InvalidStateTransitionException` the crate's own `already_finished_message`/`transition_refusal` would produce for a terminal record never happens. `time_out_if_due` also stamps `last_updated_at` at the DEADLINE; the accepted update overwrites that owed value.

## Evidence

The two decisions that read the stale status — src/ops/update_job_execution.rs:285 and 307:
```
        if record.is_terminal() {
            return Err(
                UpdateJobExecutionErrorModeled::InvalidStateTransitionException(
...
        if let Some(reason) = record.transition_refusal(&input.status) {
```
Nothing between the lock (line 237 `let mut state = state.lock();`) and those checks materialises the due transition, while the sibling handler in the same crate does — src/ops/start_command_execution.rs:536-538:
```
        // which is why the count is taken through the store (it materialises those timers first) and not from a counter.
        if let Err(limit) = state.try_admit_command_execution(&thing_name, now) {
```
The helper's contract — services/common/src/iot_job_executions.rs:864-867:
```
    /// [`JobExecutionRecord::time_out_if_due`] for why a reporting handler may write
    /// this: the move was owed at each execution's own deadline, so a handler calls
    /// this before it reads, and the whole of the thing's queue is materialised at one
    /// instant rather than only the one record a caller asked about.
```
and services/common/src/iot_job_executions.rs:388-391: "A timer AWS arms moves the execution by itself — \"If the job execution status is not set to another terminal state before the time expires, it will be automatically set to TIMED_OUT\" — so the move was owed at [`Self::timeout_deadline`] whether or not anybody called".

Model text (`UpdateJobExecutionRequest$stepTimeoutInMinutes`, smithy_query): "If the job execution status is not set to a terminal state before this timer expires, or before the timer is reset ... the job execution status will be automatically set to TIMED_OUT."

Test coverage: tests/all/job_execution_state_machine.rs:185 `a_status_update_may_never_carry_an_execution_backward` and tests/all/job_execution_lifecycle.rs:1182 `the_step_timeout_is_honoured_inside_its_published_range_and_refused_outside` only exercise the range check and transitions against a live record — no test updates an execution after its timer elapsed.

## Steps to Reproduce

1. Start a command execution for thing `laser-2` with `executionTimeoutSeconds: 1` (POST /command-executions).
2. Wait 3 seconds, so the record's `execution_timeout_expires_at` has passed and `TIMED_OUT` is owed.
3. POST /things/laser-2/jobs/{executionId} with `{"status":"SUCCEEDED","includeJobExecutionState":true}`.
4. Observe 200 and `executionState.status = "SUCCEEDED"`: the update was accepted on a record that was already due `TIMED_OUT`, instead of being refused with the modelled `InvalidStateTransitionException` (409) a terminal record gets.
