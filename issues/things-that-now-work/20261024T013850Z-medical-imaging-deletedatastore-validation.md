Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 82d0a94f39949549fd2daeecac19e7039c2ed673

# DeleteDatastore accepts a data store that is still CREATING

_Filed: 2026-10-24T01:38:49Z_
_Operation: DeleteDatastore | Class: validation_

> _Note: this issue was identified against the code base at commit `82d0a94f39949549fd2daeecac19e7039c2ed673`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler refuses a delete only when the data store still holds image sets; it never inspects `datastoreStatus`. A data store that `CreateDatastore` just returned as `CREATING` (the emulator holds that status for `DATASTORE_ACTIVATION_DELAY`) is therefore moved straight to `DELETING` and answered 200.

Expected: a data store can only be deleted once it has finished being created; AWS refuses the delete of a store that is still `CREATING` with the operation's modeled `ConflictException` (HTTP 409, "Updating or deleting a resource can cause an inconsistent state"), and the store stays `CREATING`. Actual: HTTP 200 with `datastoreStatus: DELETING`, and the clock then drops the record — a create/delete race silently cancels an in-flight creation.

## Evidence

Spec: `DeleteDatastore` declares `ConflictException` (HTTP 409) whose documented meaning is "Updating or deleting a resource can cause an inconsistent state", and `DatastoreStatus` models `CREATING` as a non-terminal pre-ACTIVE state (smithy_query operation=DeleteDatastore, shape=DatastoreStatus). Handler: src/ops/delete_datastore.rs:145-169 — the only guard is `!datastore.image_sets.is_empty()`; status is then overwritten with `Deleting` for any current status. Tests: tests/all/datastore_lifecycle.rs always deletes via `active_datastore(..)` (which waits out the activation span) and asserts nothing about deleting a CREATING store.

## Steps to Reproduce

1. POST /datastore with a fresh clientToken; the response is `datastoreStatus: CREATING`.
2. Immediately (within DATASTORE_ACTIVATION_DELAY) DELETE /datastore/{datastoreId}.
3. Observe HTTP 200 with `datastoreStatus: DELETING` instead of a 409 `ConflictException` for a store that is still being created.
4. After the deletion span, GET /datastore/{datastoreId} returns 404 — the in-flight creation was cancelled.
