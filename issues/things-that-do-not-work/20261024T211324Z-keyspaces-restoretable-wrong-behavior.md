Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: e0e9a90161aff7ec754055c046e25b8f716c6f2d

# A table being restored reports status CREATING, so the modelled RESTORING status is never observable

_Filed: 2026-10-24T21:13:24Z_
_Operation: RestoreTable | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `e0e9a90161aff7ec754055c046e25b8f716c6f2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`RestoreTable` inserts the target table with `status: CREATING` and settles it to `ACTIVE`. Expected: while a point-in-time restore is in flight the target table reports `TableStatus = RESTORING` (the member of the modelled `TableStatus` value set that exists for exactly this case) and only then becomes `ACTIVE`; a client polling `GetTable` to watch a restore therefore sees `RESTORING`, not `CREATING`.

Actual: `GetTable` on the freshly restored table reports `CREATING`, and no operation in the emulator can ever produce `RESTORING` — the status is dead on the wire. (The crate's own test comment in `tests/all/delete_table_in_flight.rs` says `RESTORING` is "unreachable here (no operation produces them — RestoreTable is still a stub)"; RestoreTable is now implemented, but it still produces `CREATING`.)

## Evidence

Spec: `TableStatus` enum = ACTIVE | CREATING | UPDATING | DELETING | DELETED | RESTORING | INACCESSIBLE_ENCRYPTION_CREDENTIALS (smithy_query shape=TableStatus); `RestoreTable` is the only operation in this service that restores a table, so `RESTORING` is the status the model reserves for its in-flight target table, and `GetTable$status` is where it is reported. Handler: `src/ops/restore_table.rs` line ~526 — `status: crate::state::TABLE_STATUS_CREATING.to_string()`. Tests: `tests/all/restore_table.rs` lines ~457-464 currently assert `TABLE_STATUS_CREATING` for the restored table, i.e. the emulator's behaviour is pinned to the wrong status; nothing asserts RESTORING anywhere in the crate.

## Steps to Reproduce

1. CreateKeyspace {"keyspaceName":"ks1"}
2. CreateTable ks1.src with {"pointInTimeRecovery":{"status":"ENABLED"}} and wait for ACTIVE
3. RestoreTable {"sourceKeyspaceName":"ks1","sourceTableName":"src","targetKeyspaceName":"ks1","targetTableName":"dst"}
4. Immediately GetTable {"keyspaceName":"ks1","tableName":"dst"} — emulator reports status CREATING; AWS reports RESTORING while the restore is in progress
