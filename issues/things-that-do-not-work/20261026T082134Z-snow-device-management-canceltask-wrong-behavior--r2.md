Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: b827135cf833dfec17047e08babbbec809d0cc25

# CancelTask terminalises a task without recording completedAt, so DescribeTask cannot tell a stopped task from a running one

_Filed: 2026-10-26T08:21:34Z_
_Operation: CancelTask | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `b827135cf833dfec17047e08babbbec809d0cc25`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CancelTask` moves the task record to `CANCELED` and advances `last_updated_at`, but never sets `completed_at`. The other path that terminalises a task — `settle`, when every execution has finished — sets `state`, `completed_at` AND `last_updated_at` together. Nothing ever fills it in afterwards either: `settle`'s task-finishing loop only considers tasks that are still in flight (`filter(|(_, task)| task.is_in_flight())`), and `is_in_flight()` is `state == "IN_PROGRESS"`, so a `CANCELED` record is skipped for ever.

The result is that `DescribeTask` reports a terminal task with `completedAt` absent, which is exactly the signal its own handler comment says marks a task that is STILL RUNNING. Two tasks that both stopped doing work — one by completing, one by being cancelled — are reported with and without `completedAt`, so the member no longer discriminates what the handler says it discriminates.

## Evidence

`services/.../src/ops/cancel_task.rs:210-218` — state and `last_updated_at` only:
```
        // The task reports CANCELED only when this call actually cancelled work;
        // a task whose every execution was already running or finished keeps the
        // state its executions carry it to.
        if cancelled_any {
            if let Some(task) = store.task_in_mut(&region, &input.task_id) {
                task.state = "CANCELED".to_string();
                task.last_updated_at = now;
            }
        }
```

The sibling terminalisation in `services/.../src/state.rs:1232-1234`:
```
            task.state = "COMPLETED".to_string();
            task.completed_at = Some(completed_at);
            task.last_updated_at = completed_at;
```
guarded by `state.rs:1202-1206`, which is why a cancelled record is never revisited:
```
        let task_ids: Vec<String> = self
            .tasks
            .iter()
            .filter(|(_, task)| task.is_in_flight())
```
with `state.rs:548-550`: `pub fn is_in_flight(&self) -> bool { self.state == "IN_PROGRESS" }`.

The rule the reporting handler states, `services/.../src/ops/describe_task.rs:192-194`:
```
            // "When the task was completed" — absent while the task is still
            // running, so a caller can tell a finished task from a running one.
            completed_at: task.completed_at.map(::aws_smithy_types::DateTime::from),
```

Test coverage: the cancel case in `tests/all/device_task_lifecycle.rs:939-988` asserts `state == CANCELED` and that `lastUpdatedAt` advanced, and asserts nothing about `completedAt`; the only `completedAt` assertions are `device_task_lifecycle.rs:757-760` (a RUNNING task has none) and `:1256-1260` (a COMPLETED task has one), so no test pins the cancelled case either way.

## Steps to Reproduce

1. Activate the inventory in the signed Region and `POST /task` with one target and `{"command":{"unlock":{}}}`.
2. Within the dispatch window (before `EXECUTION_START_SPAN` elapses) `POST /task/{taskId}/cancel`.
3. `POST /task/{taskId}` and observe `state = CANCELED`, `lastUpdatedAt` advanced, but `completedAt` absent — the same shape a task that is still `IN_PROGRESS` reports.
4. Repeat with no cancel, waiting for the executions to finish: `POST /task/{taskId}` now reports `state = COMPLETED` with a `completedAt`, showing the two terminal paths disagree on the member.
