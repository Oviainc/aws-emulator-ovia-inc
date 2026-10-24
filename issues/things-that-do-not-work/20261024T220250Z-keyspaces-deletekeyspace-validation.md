Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 564feb857476f3247a0693cb65aa2d44e3322d3a

# DeleteKeyspace destroys tables and types whose own action is still in progress, with no in-flight check

_Filed: 2026-10-24T22:02:50Z_
_Operation: DeleteKeyspace | Class: validation_

> _Note: this issue was identified against the code base at commit `564feb857476f3247a0693cb65aa2d44e3322d3a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteKeyspace` removes the keyspace record — and with it every table and user-defined type it holds — without looking at a single status. Every other write in this crate refuses to act on a resource whose own action is still settling, and `DeleteTable` refuses exactly this case: a table in `CREATING` or `UPDATING` cannot be deleted. Because the tables live inside the keyspace record, `DeleteKeyspace` deletes those same tables anyway, so the identical table is undeletable through `DeleteTable` and deletable through `DeleteKeyspace` one call later. A keyspace that is itself `CREATING` (or `UPDATING` while `UpdateKeyspace` replicates it to a new Region) is likewise removed without the modelled conflict, and `retire_auto_scaling` then deregisters the Application Auto Scaling targets of a table whose creation was still in flight.

The effect is lost state that the rest of the crate protects: a `CreateTable`/`UpdateTable`/`CreateType`/`DeleteType` transition the clock still owes is silently discarded, and the caller gets 200 for a request the model's `ConflictException` ("the same or a different action is already in progress") is declared on this very operation for.

## Evidence

No status is consulted anywhere in the handler — `src/ops/delete_keyspace.rs:176-186`:
```
// "The DeleteKeyspace operation deletes a keyspace and all of its tables." The
// tables are held INSIDE the keyspace record, so removing it removes them with
// it and no table can outlive its keyspace in state.
let Some(removed) = store.keyspaces.remove(&input.keyspace_name) else {
```
(the only check above it is `is_service_managed_keyspace`, lines 169-174).

The sibling rule it skips, `src/ops/delete_table.rs:221-236`: "// \"If a table is either in the CREATING or UPDATING states, then Amazon Keyspaces returns a ResourceInUseException\" — the code the service's own documentation names for this case" → `return Err(crate::scaffolded::Scaffolded::resource_in_use(...))`. Model, `DeleteTable`: "If a table is either in the `CREATING` or `UPDATING` states, then Amazon Keyspaces returns a `ResourceInUseException`."

The same crate applies the in-flight refusal in every other write: `src/ops/update_table.rs:350-366` ("A table is updatable only from `ACTIVE` ... this is the modelled conflict"), `src/ops/update_keyspace.rs:283-293` (keyspace must be ACTIVE) and `src/ops/update_keyspace.rs:326-342` ("a table that is itself mid-change makes this the modelled conflict" — the keyspace cannot even be *widened* while one table is not ACTIVE, yet it can be destroyed), `src/ops/delete_type.rs:223-233`, `src/ops/restore_table.rs:323-334`.

The error the model declares for `DeleteKeyspace`: `ConflictException` (HTTP 409) — "Amazon Keyspaces couldn't complete the requested action. This error may occur if you try to perform an action and the same or a different action is already in progress" (smithy_query shape=ConflictException), listed among DeleteKeyspace's errors in `OPERATIONS.md:49`.

Test coverage: the two suites that delete a keyspace both wait for everything to settle first, so nothing pins the current behaviour — `tests/all/keyspace_table_lifecycle.rs:745` and `:783` sleep `settle_wait()` before the `DeleteKeyspace` at `:824`, and `tests/all/auto_scaling_propagation.rs:731-734` sleeps `TRANSIENT_STATUS_SPAN + 250ms` before its `DeleteKeyspace`.

## Steps to Reproduce

1. CreateKeyspace {"keyspaceName":"inflight_ks"} — 200
2. CreateTable {"keyspaceName":"inflight_ks","tableName":"t1","schemaDefinition":{...}} — 200, table status CREATING for TRANSIENT_STATUS_SPAN
3. DeleteTable {"keyspaceName":"inflight_ks","tableName":"t1"} immediately — 400 ResourceInUseException, table untouched (as tests/all/delete_table_in_flight.rs pins)
4. DeleteKeyspace {"keyspaceName":"inflight_ks"} immediately — 200: the same CREATING table is destroyed and its scalable targets retired, with no ConflictException
5. Same with a keyspace still in CREATING (DeleteKeyspace right after CreateKeyspace) and with a type still in CREATING/DELETING (CreateType then DeleteKeyspace) — all answered 200
