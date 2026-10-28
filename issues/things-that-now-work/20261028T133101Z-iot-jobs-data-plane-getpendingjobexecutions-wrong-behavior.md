Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 87b02e9e0446b34956525b9761a3171af51589ac

# GetPendingJobExecutions lists executions whose TIMED_OUT transition is already due as still queued/in progress

_Filed: 2026-10-28T13:31:01Z_
_Operation: GetPendingJobExecutions | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `87b02e9e0446b34956525b9761a3171af51589ac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The operation is documented — in the model and in the handler's own header — as reporting the thing's executions that are NOT in a terminal status, and it partitions on the stored `status` string via `JobExecutionStore::with_status`. It takes an immutable lock and never materialises the `TIMED_OUT` transition the clock has already made due, although the shared store provides `time_out_due_executions` for exactly that and documents that "a handler calls this before it reads", and `StartCommandExecution` in this same crate does call it (through `try_admit_command_execution`).

An execution whose step timer or execution/in-progress timer expired therefore keeps reporting `QUEUED`/`IN_PROGRESS` to this listing until some unrelated `StartCommandExecution` for the same thing happens to run and materialise it. The listing contradicts its own contract (a terminal execution is listed as pending) and it disagrees with the IoT control plane, which reads the SAME shared record after materialising the timer. The existing concurrency test only sees the right answer because the assertion is made after a `StartCommandExecution` call: tests/all/command_execution_concurrency.rs:389-400 expires the timer, then calls `start`, and only then asserts the execution is gone from the listing — a pure read never produces that result.

## Evidence

Handler contract — src/ops/get_pending_job_executions.rs:5-7 and the model's operation doc at line 47-48:
```
//! Partitions the thing's persisted executions that are not in a terminal status
//! on `status`: `QUEUED` into `queuedJobs`, `IN_PROGRESS` into `inProgressJobs`,
//! oldest-queued first. Reports only; it writes nothing.
...
//! Gets the list of all jobs for a thing that are not in a terminal status.
```
The read, on the stale status — src/ops/get_pending_job_executions.rs:161-171:
```
        let state = state.lock();
...
        let summaries = |status: &str| -> Vec<JobExecutionSummary> {
            state
                .with_status(&input.thing_name, status)
```
and services/common/src/iot_job_executions.rs:542: `.filter(|record| record.status == status)`.

The helper this handler skips — services/common/src/iot_job_executions.rs:865-868: "the move was owed at each execution's own deadline, so a handler calls this before it reads ... pub fn time_out_due_executions(&mut self, thing_name: &str, now: i64) -> usize".
The path in this crate that calls it — src/ops/start_command_execution.rs:538: `if let Err(limit) = state.try_admit_command_execution(&thing_name, now) {` (whose doc, services/common/src/iot_job_executions.rs:842-844, reads "Timers that have already come due are materialised first ([`Self::time_out_due_executions`])").

Test coverage: the only test that expires a timer, tests/all/command_execution_concurrency.rs:389-400, interposes a `StartCommandExecution` between the expiry and the listing assertion, so the read-only path is untested.

## Steps to Reproduce

1. Start a command execution for thing `laser-3` with `executionTimeoutSeconds: 1` (POST /command-executions).
2. Wait 3 seconds.
3. GET /things/laser-3/jobs.
4. Observe the execution still present in `queuedJobs`, although its `TIMED_OUT` deadline has passed and the operation reports only jobs "that are not in a terminal status".
