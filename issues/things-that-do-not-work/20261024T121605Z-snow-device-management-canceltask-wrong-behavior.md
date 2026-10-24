Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 9e66ef5d78d13d37b68af2ed28461695d94c8b52

# CancelTask cancels executions that are already running

_Filed: 2026-10-24T12:16:05Z_
_Operation: CancelTask | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `9e66ef5d78d13d37b68af2ed28461695d94c8b52`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model documents CancelTask as: "Sends a cancel request for a specified task. You can cancel a task only if it's still in a QUEUED state. Tasks that are already running can't be cancelled." TaskState has no QUEUED value (IN_PROGRESS | CANCELED | COMPLETED), so the QUEUED state the sentence names is the per-device ExecutionState — i.e. only executions still waiting to be dispatched may be cancelled, and an execution the device has already started must keep running (and later report SUCCEEDED/FAILED).

The emulator cancels unconditionally: every non-terminal execution of the task, including executions whose state is already IN_PROGRESS, is rewritten to CANCELED with due cleared, and the task is always moved to CANCELED whenever it is IN_PROGRESS. Expected: a CancelTask that lands after an execution has started leaves that execution IN_PROGRESS (so the clock still carries it to SUCCEEDED and still applies the unlock/reboot effect to the device); only the still-QUEUED executions become CANCELED, and a task with no cancellable execution left is not reported CANCELED.

Actual: after the emulator's EXECUTION_START_SPAN (400 ms) has elapsed, DescribeExecution/ListExecutions report CANCELED for an execution that was already running, and DescribeDevice never shows the command's effect (e.g. an unlock that the device had already begun is silently discarded).

## Evidence

Spec: smithy_query(query="doc", shape="CancelTask") — "You can cancel a task only if it's still in a QUEUED state. Tasks that are already running can't be cancelled."; ExecutionState includes QUEUED and IN_PROGRESS.
Handler: services/service-com-amazonaws-snowdevicemanagement-snowdevicemanagement/src/ops/cancel_task.rs lines 178-195 — `let cancellable = task.state == "IN_PROGRESS"` then `for ((task_id, _), execution) in store.executions.iter_mut() { if task_id != &input.task_id || execution.is_terminal() { continue; } execution.state = "CANCELED"...}`; `Execution::is_terminal()` (src/state.rs lines 356-361) excludes only SUCCEEDED/FAILED/CANCELED/REJECTED/TIMED_OUT, so an IN_PROGRESS (already running) execution is cancelled.
Tests: tests/all/device_task_lifecycle.rs `cancel_task_moves_the_task_and_its_pending_executions_to_canceled` deliberately sleeps only 60 ms — "Far short of the span the clock owes the first execution, so the cancel lands while every execution is still pending" — so the already-running case is NOT asserted anywhere.

## Steps to Reproduce

1. Start the emulator with the Snow Device Management service and at least one activated device in the signed region.
2. POST /task with targets=[<deviceId>] and command={"unlock":{}} and note the returned taskId.
3. Wait longer than the emulator's EXECUTION_START_SPAN (>400 ms) so the execution has moved to IN_PROGRESS (verify with POST /task/{taskId}/execution/{deviceId}).
4. POST /task/{taskId}/cancel.
5. POST /task/{taskId}/execution/{deviceId} again: the emulator reports state CANCELED, where the running execution should have been left IN_PROGRESS and allowed to finish.
6. POST /managed-device/{deviceId}/describe: deviceState stays LOCKED, i.e. the already-started unlock was discarded.
