Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: d0704f2fe7a69fbeda61cd50377e188330130aec

# ListTasks refuses with 403 AccessDeniedException instead of returning an empty task list

_Filed: 2026-10-24T15:14:37Z_
_Operation: ListTasks | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d0704f2fe7a69fbeda61cd50377e188330130aec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GET /tasks` consults the invented account-level device gate before doing anything: when the Region holds no activated device, the handler answers HTTP 403 `AccessDeniedException` rather than enumerating tasks. Expected: ListTasks reports the tasks of the account in that Region; when there are none the answer is HTTP 200 with an empty `tasks` list (and no `nextToken`), the standard AWS list-operation semantics. `AccessDeniedException` is an authorization outcome, not an "inventory is empty" outcome.

Since nothing in `src/` ever puts a device in the inventory (see the ListDevices issue), this means a live emulator answers ListTasks 403 unconditionally, even right after a successful call sequence would have created tasks.

## Evidence

Spec: Smithy model `ListTasks` -> `GET /tasks`, @paginated(items=tasks), output `tasks` + `nextToken`. The model/doc describes a filtered enumeration; nothing conditions the call on the account owning devices. Well-established AWS behavior: list/enumerate APIs return an empty collection with HTTP 200 when nothing matches.

Handler: `src/ops/list_tasks.rs` lines ~162-172 — `if !store.has_activated_device(&region) { return Err(ListTasksErrorModeled::AccessDeniedException(no_devices_in_region())) }`, placed before the token is decoded and before any task is examined.

Tests: `tests/all/account_device_gate.rs` asserts this 403 for ListTasks (including per TaskState filter), so the divergent behavior is locked in by a test rather than caught by one.

## Steps to Reproduce

1. Start the emulator with a fresh engine.
2. Call ListTasks (GET /tasks), optionally with `state=IN_PROGRESS`.
3. Observe HTTP 403 AccessDeniedException; expected HTTP 200 with `{"tasks": []}`.
