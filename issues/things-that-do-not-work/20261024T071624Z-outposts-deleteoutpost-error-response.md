Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 7805cfa6f36158072c58d31caff72f5961880244

# DeleteOutpost never answers ConflictException: an Outpost with an in-flight capacity task is deleted and the task silently discarded

_Filed: 2026-10-24T07:16:24Z_
_Operation: DeleteOutpost | Class: error-response_

> _Note: this issue was identified against the code base at commit `7805cfa6f36158072c58d31caff72f5961880244`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteOutpost` removes the Outpost record unconditionally as soon as the identifier resolves. The only refusals the handler can produce are `ValidationException` (malformed identifier) and `NotFoundException` (unknown id); the `ConflictException` (HTTP 409) the model declares for this operation is unreachable from the handler.

Concretely, an Outpost that still has work or dependents attached is deleted anyway:

* a capacity task in a non-terminal status (`REQUESTED` / `IN_PROGRESS`) on that Outpost — the handler deletes the Outpost and then does `store.capacity_tasks.retain(|_, task| task.outpost_id != outpost_id)`, so the in-flight task is destroyed and a later `GetCapacityTask` answers 404 for a task that was never cancelled or completed;
* an Outpost that carries a private connectivity configuration (`CreatePrivateConnectivityConfig`);
* an Outpost a quote is associated with (`CreateQuote` with `OutpostIdentifier`) — the quote keeps reporting an `OutpostArn` that names nothing.

Expected: deleting an Outpost that still has dependent state is refused with the modelled `ConflictException` (HTTP 409, `ResourceId`/`ResourceType` naming the Outpost), leaving the Outpost and its capacity task intact. Actual: HTTP 200, the Outpost disappears and the running capacity task is silently dropped.

## Evidence

Spec: the Smithy model declares `ConflictException` (HTTP 409, doc "Updating or deleting this resource can cause an inconsistent state", with `ResourceId`/`ResourceType` members) among `DeleteOutpost`'s errors — for a delete, dependent state is the only thing that can cause that inconsistent state, so the handler making it unreachable means a documented failure mode is never produced. Well-established AWS behaviour: an Outpost cannot be deleted while work (an order, a capacity task) is still outstanding on it.

Handler: `src/ops/delete_outpost.rs` lines ~153-175 — `store.outposts.remove(&outpost_id)` with no dependency check, followed by the `capacity_tasks.retain(...)` that drops the Outpost's tasks regardless of their status. Compare `delete_site.rs`, which at least checks for dependents.

Tests: `tests/all/capacity_task_lifecycle.rs::deleting_an_outpost_takes_its_capacity_tasks_with_it` (line ~819) pins the current behaviour — it starts a task (status `REQUESTED`, still active), deletes the Outpost with a 200 and then asserts the task is 404. No test asserts a conflict refusal.

## Steps to Reproduce

1. CreateSite, then CreateOutpost against it; keep the OutpostId.
2. POST /outposts/{OutpostId}/capacity with {"InstancePools":[{"InstanceType":"m5.4xlarge","Count":1}]} and note the returned CapacityTaskId with CapacityTaskStatus REQUESTED.
3. Immediately DELETE /outposts/{OutpostId} — observe HTTP 200 instead of a 409 ConflictException naming the Outpost.
4. GET /outposts/{OutpostId}/capacity/{CapacityTaskId} — observe 404 NotFoundException for a task that was never cancelled or completed.
