Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: e0e9a90161aff7ec754055c046e25b8f716c6f2d

# DeleteKeyspace removes the keyspace synchronously, so the modelled DELETING keyspace status is never observable

_Filed: 2026-10-24T21:13:24Z_
_Operation: DeleteKeyspace | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `e0e9a90161aff7ec754055c046e25b8f716c6f2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteKeyspace` removes the keyspace (and every table inside it) from state inside the call, so the very next `GetKeyspace`/`ListKeyspaces` answers 404 / omits it.

Expected: the deletion is observable rather than instantaneous — the keyspace enters `KeyspaceStatus = DELETING` (the model's `KeyspaceStatus` value set is ACTIVE | CREATING | UPDATING | DELETING, and `ReplicationGroupStatus$keyspaceStatus` is where it is reported) and disappears once the deletion completes. That is exactly the pattern this crate already implements for tables (`DeleteTable` → DELETING → gone, materialised by `state::settle`) and for types (`DeleteType` → DELETING → gone); only the keyspace level skips it, so `DELETING` is dead on the wire for keyspaces.

Secondary symptom of the same synchronous path: a keyspace is deleted even while it or its tables are mid-change (keyspace `CREATING`/`UPDATING`, tables `CREATING`/`UPDATING`), taking tables with it that `DeleteTable` itself refuses to delete with ResourceInUseException.

## Evidence

Spec: `KeyspaceStatus` enum includes DELETING (smithy_query shape=KeyspaceStatus), reported through `ReplicationGroupStatus$keyspaceStatus` ("The status of the keyspace") which `GetKeyspace` returns for a MULTI_REGION keyspace — no operation in this emulator can produce it. Handler: `src/ops/delete_keyspace.rs` lines ~176-199 — `store.keyspaces.remove(&input.keyspace_name)` with no status transition and no `note_transition_due`, versus `src/ops/delete_table.rs` lines ~208-242 and `src/ops/delete_type.rs` lines ~264-276 which both set DELETING and let `state::settle` remove the resource. Tests: `tests/all/keyspace_table_lifecycle.rs` lines ~823-845 currently pin the synchronous behaviour (GetKeyspace 404 immediately after DeleteKeyspace); nothing asserts an observable DELETING keyspace.

## Steps to Reproduce

1. CreateKeyspace {"keyspaceName":"ks1","replicationSpecification":{"replicationStrategy":"MULTI_REGION","regionList":["us-east-1","eu-west-1"]}} and wait for it to settle
2. DeleteKeyspace {"keyspaceName":"ks1"}
3. GetKeyspace {"keyspaceName":"ks1"} immediately after — emulator answers 404 ResourceNotFoundException; AWS still reports the keyspace with replicationGroupStatuses[*].keyspaceStatus = DELETING until the deletion completes
4. Variant: CreateTable ks1.t1 and, while t1 is still CREATING, call DeleteKeyspace — the emulator deletes keyspace and table outright even though DeleteTable on that table answers ResourceInUseException
