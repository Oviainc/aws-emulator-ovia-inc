Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 388872f1987f74fe9682288e62ee8e18a6dfb8b9

# CancelTask re-implements Task::is_in_flight instead of calling it

_Filed: 2026-10-26T04:34:22Z_
_Operation: CancelTask | Class: maintainability_

> _Note: this issue was identified against the code base at commit `388872f1987f74fe9682288e62ee8e18a6dfb8b9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`Task::is_in_flight()` is the crate's single definition of "this task still holds work" (`state == "IN_PROGRESS"`), and the state module's quota path calls it. `CancelTask` needs the same predicate and inlines the string comparison instead, so the rule now exists twice in this crate and the two copies can drift (a later change to `TaskState` handling in `is_in_flight` would silently leave `CancelTask` on the old rule).

Fix is a call to the existing helper; no response and no existing test changes.

## Evidence

The helper, `services/service-com-amazonaws-snowdevicemanagement-snowdevicemanagement/src/state.rs:544-550`:
```
    /// True while this task still holds work: `TaskState` is `IN_PROGRESS`, the one
    /// non-terminal value of the three (`COMPLETED` and `CANCELED` are both final,
    /// and a task in either holds nothing a quota counts).
    pub fn is_in_flight(&self) -> bool {
        self.state == "IN_PROGRESS"
    }
```
used by the quota path, `state.rs:733-738`:
```
    pub fn tasks_in_flight_in(&self, region: &str) -> usize {
        self.tasks
            .values()
            .filter(|task| task.region == region && task.is_in_flight())
```
The second copy, `src/ops/cancel_task.rs:185-187`:
```
        // A task that already finished or was already cancelled has nothing left
        // to stop; its record keeps the state and the instant it reached it.
        let task_running = task.state == "IN_PROGRESS";
```
Covered by `tests/all/cancel_task_dispatch_window.rs` (the dispatch-window table) and `tests/all/device_task_lifecycle.rs` (cancel then describe), both of which only observe the response, so replacing the literal with `task.is_in_flight()` leaves them unchanged.

## Steps to Reproduce

1. Open `src/state.rs:548` and read `Task::is_in_flight` — the one definition of the `IN_PROGRESS` rule.
2. Open `src/ops/cancel_task.rs:187` and see the same rule written out as a string comparison rather than a call to that helper.
3. Note `src/state.rs:736` and `state.rs:746` call the helper, so the crate already has one path using it and one path duplicating it.
