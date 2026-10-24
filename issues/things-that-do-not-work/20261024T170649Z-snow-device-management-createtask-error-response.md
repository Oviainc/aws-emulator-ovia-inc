Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 6f4880dda1aa7f89588445b4a5ba9a411741294f

# CreateTask refuses with ServiceQuotaExceededException once 1000 tasks have EVER been created, counting completed and canceled ones

_Filed: 2026-10-24T17:06:49Z_
_Operation: CreateTask | Class: error-response_

> _Note: this issue was identified against the code base at commit `6f4880dda1aa7f89588445b4a5ba9a411741294f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The quota check in `CreateTask` is `store.tasks.len() >= crate::state::TASK_QUOTA` (TASK_QUOTA = 1_000), and nothing ever removes a task from the store: this API models no DeleteTask, and `settle` only moves a task to COMPLETED/CANCELED, it never drops the record. The count is also global rather than per-Region.

Consequence: a long-running emulator that has created 1000 tasks — however long ago, and even if every single one of them has reached COMPLETED or CANCELED — answers every subsequent CreateTask with HTTP 402 `ServiceQuotaExceededException` forever, so the task lifecycle becomes permanently unusable with no API to recover it.

Expected: a service quota bounds ACTIVE/in-flight resources, not the number of resources ever created; creating a task after 1000 earlier tasks have finished succeeds (HTTP 200 with a fresh taskId/taskArn). `ServiceQuotaExceededException` ("The request would cause a service quota to be exceeded") is owed only while the quota is actually exceeded, and a terminal task does not consume one. If the emulator needs a bound to stop the store growing without end, it should be counted over IN_PROGRESS tasks (and/or scoped to the Region the request is signed for, the way every other record in this state is), not over the store's whole history.

## Evidence

Spec: `smithy_query(query="operation", operation="CreateTask")` declares `ServiceQuotaExceededException` (HTTP 402) documented as "The request would cause a service quota to be exceeded" — a statement about the quota's current consumption. A COMPLETED or CANCELED task is terminal (`TaskState` = IN_PROGRESS | COMPLETED | CANCELED) and consumes nothing; AWS quotas counting active resources rather than lifetime creations is well-established service behavior. The model also provides no operation that deletes a task, so there is no client-visible way to free the emulator's counter.

Handler: `src/ops/create_task.rs` lines ~221-231 — `if store.tasks.len() >= crate::state::TASK_QUOTA { return Err(... ServiceQuotaExceededException ...) }`, with no filter on `task.state` and no filter on `task.region`. `src/state.rs` lines ~48-55 define TASK_QUOTA and concede the bound "is the emulator's OWN bound on the store, not a claim about any account's AWS quota". `src/state.rs::settle` (lines ~614-648) only rewrites `task.state`/`completed_at`; `store.tasks` is never pruned, and `grep` finds no `tasks.remove` anywhere in `src/`.

Tests: not covered. `tests/all/device_task_lifecycle.rs` creates at most a handful of tasks per emulator and no case approaches TASK_QUOTA or asserts anything about 402, so nothing pins either the current behavior or the expected one.

## Steps to Reproduce

1. Start the emulator with an activated device inventory in the signed Region (the only seam that puts devices in the inventory).
2. POST /task with `targets=[<deviceId>]`, `command={"reboot":{}}` 1000 times, waiting past the execution spans so every task settles to COMPLETED (verify with POST /task/{taskId}: state COMPLETED, completedAt set).
3. POST /task once more with the same body.
4. Observe HTTP 402 `ServiceQuotaExceededException` ("this account already holds the maximum of 1000 tasks"); expected HTTP 200 with a new taskId/taskArn, because no task is in flight and a terminal task consumes no quota.
5. Also note the count is cross-Region: tasks created in Region A are counted against a CreateTask signed for Region B, even though that Region can neither describe nor list them.
