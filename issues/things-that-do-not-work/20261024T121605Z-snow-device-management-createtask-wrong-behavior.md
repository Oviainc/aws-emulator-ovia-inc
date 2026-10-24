Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 9e66ef5d78d13d37b68af2ed28461695d94c8b52

# A device can run two task executions at once: CreateTask does not serialize executions per device

_Filed: 2026-10-24T12:16:05Z_
_Operation: CreateTask | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `9e66ef5d78d13d37b68af2ed28461695d94c8b52`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model states for CreateTask's `command` member: "The task to be performed. Only one task is executed on a device at a time." A second task targeting a device that still has an in-flight execution must therefore wait: its execution stays QUEUED until the earlier execution of that device reaches a terminal state, and only then is it dispatched (IN_PROGRESS) and applied to the device.

The emulator gives every execution of a newly created task a dispatch instant of `now + EXECUTION_START_SPAN` regardless of what the target device is already doing, so two tasks created back to back against the same device produce two executions that are both IN_PROGRESS at the same instant and both apply their command. Expected: at most one non-terminal-in-flight (IN_PROGRESS) execution per managed device at any time, with the rest reported QUEUED.

## Evidence

Spec: smithy_query(query="operation", operation="CreateTask") / the model's `Command` member doc — "Only one task is executed on a device at a time."
Handler: src/ops/create_task.rs lines 256-276 — each new execution is inserted with `state: "QUEUED"` and `due: Some(now + crate::state::EXECUTION_START_SPAN)` with no check of the target device's other executions; src/state.rs `settle` (lines 577-630) then advances every due execution independently, so two executions of the same device can both be IN_PROGRESS and both push an effect.
Tests: tests/all/device_task_lifecycle.rs covers one task at a time per emulator (`queue_task` + the clock test `the_clock_carries_an_execution_to_succeeded_and_applies_the_command`); no case creates two overlapping tasks against one device, so nothing asserts the one-at-a-time rule.

## Steps to Reproduce

1. Start the emulator with an activated device in the signed region.
2. POST /task with targets=[<deviceId>], command={"unlock":{}} — note taskId A.
3. Immediately POST /task with targets=[<deviceId>], command={"reboot":{}} — note taskId B.
4. Wait just past EXECUTION_START_SPAN (>400 ms) and call POST /task/A/execution/<deviceId> and POST /task/B/execution/<deviceId>.
5. Both executions report IN_PROGRESS for the same device; the second should still be QUEUED until the first execution of that device is terminal.
