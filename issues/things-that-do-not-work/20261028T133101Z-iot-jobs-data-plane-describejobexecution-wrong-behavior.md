Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 87b02e9e0446b34956525b9761a3171af51589ac

# DescribeJobExecution reports a due-TIMED_OUT execution as pending with approximateSecondsBeforeTimedOut 0 instead of materialising the transition

_Filed: 2026-10-28T13:31:01Z_
_Operation: DescribeJobExecution | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `87b02e9e0446b34956525b9761a3171af51589ac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler takes an immutable lock, resolves the record and decides between 200 and the modelled `TerminalStateException` (410) from the STORED status, without first materialising the `TIMED_OUT` transition the clock has already made due. This crate already has the helper (`JobExecutionStore::time_out_due_executions`, whose doc says "a handler calls this before it reads") and one of its own handlers calls it (`StartCommandExecution` via `try_admit_command_execution`); this read does not, and the handler's own header comment admits the gap.

The result is a self-contradictory 200: `execution.status` is still `QUEUED`/`IN_PROGRESS` while `execution.approximateSecondsBeforeTimedOut` — computed as `deadline.saturating_sub(now).max(0)` — is 0, i.e. the response states that no time remains before the status becomes `TIMED_OUT` while reporting a pending status. The `record.is_terminal()` branch at line 198, which exists precisely to answer 410 for a finished execution, can never fire for an execution the clock finished. The same shared record read through the IoT control plane's `DescribeJobExecution` DOES materialise the timer first, so the two readers of one store disagree.

## Evidence

The gap the handler itself admits — src/ops/describe_job_execution.rs:14-17:
```
//! `InvalidRequestException` (400)
//! is still not produced here: the request combinations the real service refuses are
//! filed as their own issue along with the rest of the long tail (clock-driven
//! `TIMED_OUT`, certificate validation, throttling).
```
(no such issue exists: `list_existing_issues` for this service / "TIMED_OUT" / "timeout" returns none).

The read and the branch that depends on it — src/ops/describe_job_execution.rs:175-215:
```
        let state = state.lock();
...
            state.resolve_record(&input.thing_name, &input.job_id, input.execution_number)
...
        if record.is_terminal() {
...
                execution: Some(crate::state::to_job_execution(
                    record,
                    include_job_document,
                    crate::state::now_epoch_seconds(),
```
where the reported remaining time is services/common/src/iot_job_executions.rs:363-365:
```
    pub fn approximate_seconds_before_timed_out(&self, now: i64) -> Option<i64> {
        let deadline = self.timeout_deadline()?;
        Some(deadline.saturating_sub(now).max(0))
```
The helper skipped here, with its contract — services/common/src/iot_job_executions.rs:865-868: "so a handler calls this before it reads"; the sibling that calls it — src/ops/start_command_execution.rs:538 `state.try_admit_command_execution(&thing_name, now)`.

The other reader of the same shared store, which does materialise first — services/service-com-amazonaws-iot-awsiotservice/src/ops/describe_job_execution.rs:204-209:
```
        // A timer the job or the device armed moves an execution to `TIMED_OUT` by
        // itself, so the transitions the clock has already made due are recorded
        // before anything is read: each was owed at its own deadline whether or not
        // this call happened, and the status a later reader of the shared store sees
        // must be the same one.
        store.time_out_due_executions(&input.thing_name, now);
```
Test coverage: tests/all/job_execution_lifecycle.rs:416 `the_execution_lifecycle_is_persisted_and_read_back_at_every_step` never lets a timer elapse; no describe-after-expiry test exists.

## Steps to Reproduce

1. Start a command execution for thing `laser-4` with `executionTimeoutSeconds: 1` (POST /command-executions) and note the returned `executionId`.
2. Wait 3 seconds.
3. GET /things/laser-4/jobs/{executionId}.
4. Observe 200 with `execution.status = "QUEUED"` and `execution.approximateSecondsBeforeTimedOut = 0` — a pending status with no time left — instead of the record being materialised as `TIMED_OUT` and the read answering the modelled `TerminalStateException` (410) the handler's own `is_terminal()` branch produces for a finished execution.
