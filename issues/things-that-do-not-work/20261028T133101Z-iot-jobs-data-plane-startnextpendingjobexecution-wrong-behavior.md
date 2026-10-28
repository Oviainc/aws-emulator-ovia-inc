Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 87b02e9e0446b34956525b9761a3171af51589ac

# StartNextPendingJobExecution hands out and re-starts an execution whose TIMED_OUT transition the clock already made due, so a polling device keeps a timed-out execution alive forever

_Filed: 2026-10-28T13:31:01Z_
_Operation: StartNextPendingJobExecution | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `87b02e9e0446b34956525b9761a3171af51589ac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The shared store owns one clock-driven transition — `TIMED_OUT` — and exposes `JobExecutionStore::time_out_due_executions(thing, now)` to materialise it, documenting that "a handler calls this before it reads". In this crate only `StartCommandExecution` does so (through `try_admit_command_execution`). `StartNextPendingJobExecution` never calls it: it takes the store lock, asks `next_pending_index` for the thing's next pending execution and starts whatever it gets back.

Because an execution whose step timer or in-progress/execution timer has already expired still carries the status `QUEUED`/`IN_PROGRESS` in the store, `next_pending_index` (which filters on `record.is_pending()`) still returns it, and the handler then WRITES to it: `record.start(now)`, `record.arm_step_timer(...)`, `record.last_updated_at = now`, `record.version_number += 1`. The execution is thereby handed to the device as a live job and, if the request carries `stepTimeoutInMinutes`, its timer is re-armed from `now` — so an execution that was owed `TIMED_OUT` minutes or days ago never reaches that status as long as the device keeps polling, and the queued execution behind it is never handed out. Note also that `time_out_if_due` stamps `last_updated_at` at the DEADLINE; starting the record first destroys that owed stamp.

The same shared record read through the IoT control plane DOES materialise the timer, so the two planes report different statuses for one record.

## Evidence

The helper and its contract — services/common/src/iot_job_executions.rs:862-868:
```
    /// Materialise the `TIMED_OUT` transition every one of `thing_name`'s executions
    /// has already come due for, and answer how many moved. See
    /// [`JobExecutionRecord::time_out_if_due`] for why a reporting handler may write
    /// this: the move was owed at each execution's own deadline, so a handler calls
    /// this before it reads, and the whole of the thing's queue is materialised at one
    /// instant rather than only the one record a caller asked about.
    pub fn time_out_due_executions(&mut self, thing_name: &str, now: i64) -> usize {
```
The path in THIS crate that does call it — services/service-com-amazonaws-iotjobsdataplane-iotlaserthingjobmanagerexternalservice/src/ops/start_command_execution.rs:534-538:
```
        // Executions that have finished — including one
        // whose own `executionTimeoutSeconds` the clock has already made `TIMED_OUT`
        // — have released their slot, which is why the count is taken through the
        // store (it materialises those timers first) and not from a counter.
        if let Err(limit) = state.try_admit_command_execution(&thing_name, now) {
```
The path that does not — src/ops/start_next_pending_job_execution.rs:208-211 and 239-259:
```
        let mut state = state.lock();
        // Pending is "IN_PROGRESS or QUEUED", as this operation documents; an
        // already-started execution is resumed before a queued one is started.
        let Some(index) = state.next_pending_index(&input.thing_name) else {
...
            if record.status != STATUS_IN_PROGRESS {
...
                record.start(now);
                record.arm_step_timer(input.step_timeout_in_minutes, now);
                record.last_updated_at = now;
                // Starting the execution IS a device update, so the version moves.
                record.version_number += 1;
```
`next_pending_index` selects on the stale status — services/common/src/iot_job_executions.rs:556: `.filter(|(_, record)| record.is_pending())`.

Model text the timer comes from (`StartNextPendingJobExecutionRequest$stepTimeoutInMinutes`, smithy_query): "If the job execution status is not set to a terminal state before this timer expires, or before the timer is reset ... the job execution status will be automatically set to TIMED_OUT."

Test coverage: no test in tests/all exercises a start after a timer elapsed — `expire_execution_timer` exists only in tests/all/command_execution_concurrency.rs:156 and is followed by a `StartCommandExecution`, which is the one call that materialises the timer.

## Steps to Reproduce

1. Start a command execution for thing `laser-1` (POST /command-executions with a valid targetArn/commandArn and `executionTimeoutSeconds: 1`), which stores a QUEUED execution with `execution_timeout_expires_at = now + 1`.
2. Wait 3 seconds — the record's `timeout_deadline()` has passed, so `TIMED_OUT` is owed.
3. PUT /things/laser-1/jobs/$next.
4. Observe 200 with `execution.status = "IN_PROGRESS"` and `versionNumber` incremented: the already-due TIMED_OUT execution was handed out and re-started instead of being materialised as terminal (and the next queued execution stays blocked behind it).
