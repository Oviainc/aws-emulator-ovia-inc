Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 65fd8797851bb5c5937d1ea0792f5c718c73df47

# Factor the duplicated nextToken decode-or-refuse rule out of the four paginated handlers

_Filed: 2026-10-27T22:20:58Z_
_Operation: ListDevices | Class: maintainability_

> _Note: this issue was identified against the code base at commit `65fd8797851bb5c5937d1ea0792f5c718c73df47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule "no `nextToken` means start at the beginning; a `nextToken` this enumeration did not issue is refused with the modelled `ValidationException` built by `invalid_next_token(Self::NAME)`" is written out four times — once in each paginated handler — and the four copies differ only in the name of the per-operation error enum. They can drift independently (one copy could stop refusing, or refuse with a different shape) while the other three keep working, and the crate already owns both halves of the rule in `state.rs` (`decode_token`, `invalid_next_token`).

A single helper beside `encode_token`/`decode_token` in `src/state.rs` — taking `Option<&str>` and a `&CursorScope`, answering `Result<Option<String>, ValidationException>` — lets each handler keep its own error variant (`.map_err(|e| XErrorModeled::ValidationException(e))?`) while the branch exists once. No response body, status, message or existing test changes.

## Evidence

Four copies of the same branch:

src/ops/list_devices.rs:191-202:
```
let resume_at = match input.next_token.as_deref() {
    None => None,
    Some(token) => match crate::state::decode_token(token, &cursor) {
        Some(key) => Some(key),
        None => {
            return Err(ListDevicesErrorModeled::ValidationException(
                crate::state::invalid_next_token(Self::NAME),
            )
            .into())
        }
    },
};
```
src/ops/list_tasks.rs:185-196 — identical but for `ListTasksErrorModeled::ValidationException`.
src/ops/list_executions.rs:204-215 — identical but for `ListExecutionsErrorModeled::ValidationException`.
src/ops/list_device_resources.rs:204-215 — identical but for `ListDeviceResourcesErrorModeled::ValidationException`.

The rule's pieces already live in one place — src/state.rs:1842-1847 (`decode_token`: "The resume key `token` names, or `None` when it is not a cursor this service issued for the enumeration `scope` describes") and src/state.rs:1859-1863 (`invalid_next_token`).

Test coverage that must stay green and unchanged: tests/all/pagination_cursor_binding.rs:7 — "(`ListDevices`, `ListTasks`, `ListExecutions`, `ListDeviceResources`)" — with the per-operation refusal rows at lines 363-384 and the cross-operation/region/filter replay rows at 524-533, 566-697, 739-753.

## Steps to Reproduce

1. Open src/ops/list_devices.rs:191-202, src/ops/list_tasks.rs:185-196, src/ops/list_executions.rs:204-215 and src/ops/list_device_resources.rs:204-215 side by side: the same five-line branch appears four times, differing only in the error enum named.
2. Change the rule in one copy only (for example drop the refusal and treat an unknown token as 'start at the beginning'): the other three handlers keep refusing, so one paginated operation silently diverges from the other three while the crate still has one `decode_token` helper.
3. Confirm the fix is behaviour-preserving by running tests/all/pagination_cursor_binding.rs, which exercises all four operations' token issuing and refusal over the wire.
