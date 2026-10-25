Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: ee21bdc0559bb252101e2375a2c132d26bd8f920

# A forged or foreign nextToken is accepted as a valid page cursor instead of being refused with ValidationException

_Filed: 2026-10-24T18:35:11Z_
_Operation: ListDevices | Class: pagination_

> _Note: this issue was identified against the code base at commit `ee21bdc0559bb252101e2375a2c132d26bd8f920`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The three paginated list operations (ListDevices, ListTasks, ListExecutions) treat `nextToken` as nothing more than a hex-encoded resume key: `crate::state::decode_token` hex-decodes the value and accepts it whenever the bytes happen to be valid UTF-8. Consequences, all observable over HTTP:

1. A token this service never issued is ACCEPTED whenever it is even-length hex decoding to UTF-8. `GET /managed-devices?nextToken=61626364` (hex of "abcd") is answered HTTP 200 with a full page of devices, because every `smd-…` id sorts after the decoded resume key "abcd". Expected: a `nextToken` the service did not issue is refused with the modeled `ValidationException` (HTTP 400) — which is exactly what the handler's own comment claims it does ("a value this service never issued is refused rather than silently serving page one").
2. A token is not bound to the operation or to the filter that issued it. A `nextToken` returned by ListDevices (hex of a `smd-…` id) is accepted by `GET /tasks?nextToken=…` and silently yields a page of tasks (the device id sorts after every `st-…` id, so the answer is an EMPTY task list with no token — i.e. a caller resuming with the wrong token is told "no more tasks" instead of being refused). Likewise a ListExecutions token minted for task A is accepted for task B.

Real AWS pagination tokens are opaque, service-issued and validated: a value the service did not mint (or one minted for a different request) is rejected with ValidationException rather than reinterpreted as a sort key.

## Evidence

Spec: `ListDevices`/`ListTasks`/`ListExecutions` are `@paginated(inputToken=nextToken, outputToken=nextToken)` with `NextToken` constrained `@length(1..1024) @pattern([a-zA-Z0-9+/=]*)`, and all three declare `ValidationException` (HTTP 400) — the modeled refusal for an input that fails the service's constraints (confirmed via smithy_query on the three operations). AWS pagination tokens being opaque/service-issued and rejected when forged is well-established behavior.

Handler: `src/state.rs::decode_token` (~lines 1052-1054) — `String::from_utf8(hex::decode(token).ok()?).ok()`: the ONLY validation is hex-decodability plus UTF-8, with no HMAC/marker, no operation tag and no filter binding. Used by `src/ops/list_devices.rs` (~lines 178-193), `src/ops/list_tasks.rs` (~lines 177-192) and `src/ops/list_executions.rs` (~lines 179-194), each of which maps `None` to ValidationException and otherwise uses the decoded value directly as the resume key (`id.as_str() < key`).

Tests: the hole is NOT covered. `tests/all/device_task_lifecycle.rs::the_list_operations_refuse_a_next_token_they_never_issued` (~lines 450-470) uses only the rows `["zz", "abc", "ffff", "ZZZZZZZZ", "===="]` — every one of them fails hex decoding or UTF-8 decoding, so no test exercises a well-formed-hex forged token or a token issued by a different list operation.

## Steps to Reproduce

1. Start the emulator with an activated device inventory in the signed Region (the only seam that populates devices) so the account-level device gate lifts.
2. Send `GET /managed-devices?nextToken=61626364` (hex of "abcd", which satisfies NextToken's pattern). Observe HTTP 200 with a page of devices; expected HTTP 400 ValidationException, since the emulator never issued that token.
3. Send `GET /managed-devices?maxResults=1` and copy the `nextToken` it returns (hex of a `smd-…` id).
4. Replay that token on a different operation: `GET /tasks?nextToken=<that token>`. Observe HTTP 200 with an empty `tasks` list and no `nextToken` (the device id sorts after every task id) instead of a 400 refusal of a token ListTasks never issued.
5. Create a task over two devices, call `GET /executions?taskId=<taskA>&maxResults=1`, then replay the returned token against `GET /executions?taskId=<taskB>&nextToken=…` and observe it is accepted as a cursor into task B's executions.
