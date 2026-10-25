Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: e0e9a90161aff7ec754055c046e25b8f716c6f2d

# UpdateTable accepts addColumns whose type is not a CQL data type or an existing ACTIVE user-defined type

_Filed: 2026-10-24T21:13:24Z_
_Operation: UpdateTable | Class: validation_

> _Note: this issue was identified against the code base at commit `e0e9a90161aff7ec754055c046e25b8f716c6f2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateTable`'s `addColumns` entries are appended to the stored `schemaDefinition.allColumns` after only two checks: the per-table column cap and a duplicate-name check. The *type* string of each added column is never checked.

Expected (the rule `CreateTable` already enforces in this same crate): a column's `type` must be a CQL data type or a type constructor, or else name a user-defined type "contained in the same keyspace" whose status is ACTIVE; anything else is the modelled `ResourceNotFoundException` (404, "The operation tried to access a keyspace, table, or type that doesn't exist. The resource might not be specified correctly, or its status might not be ACTIVE"), and nothing is stored.

Actual: `UpdateTable {"addColumns":[{"name":"junk","type":"nosuchtype"}]}` — or a column whose type names a UDT that does not exist / is still CREATING — is accepted with 200, the bogus column is persisted, and `GetTable` afterwards reports a schema containing a column of a type the service could never have created. A later `CreateType` of that name is then also wrongly reported as "still used by table ..." by `DeleteType`, because the referring-table derivation (`Keyspace::direct_referring_tables`) resolves the phantom reference.

## Evidence

Spec: `ColumnDefinition$type` — "The data type of the column. For a list of available data types, see Data types" (Smithy model, `GenericString`, so the boundary validator cannot refuse it); `FieldDefinition$type` / `ColumnDefinition$type` UDT rule — a referenced type must be a type of the same keyspace in status ACTIVE, which is what `ResourceNotFoundException` documents. Handler: `src/ops/update_table.rs` lines ~418-472 (`added_columns`) — cap check and duplicate-name check only, then `schema.all_columns.extend(added)` at line ~518; it never calls `crate::state::referenced_type_names` / checks `keyspace.types`, unlike `src/ops/create_table.rs` lines ~331-344 which does exactly that for every `schemaDefinition.allColumns` entry. Tests: `tests/all/update_table.rs` exercises addColumns duplicates, the column cap and an empty list, but no row supplies an unknown column type (grep for `addColumns` across `tests/` shows only valid types `text`, `int`, `timestamp`). Related, already fixed for the sibling operation: issue `20261022T071259Z-keyspaces-createtable-accepts-unknown-column-types-refuses-autoscalingspecification.md`.

## Steps to Reproduce

1. CreateKeyspace {"keyspaceName":"ks1"}
2. CreateTable {"keyspaceName":"ks1","tableName":"t1","schemaDefinition":{"allColumns":[{"name":"pk","type":"text"}],"partitionKeys":[{"name":"pk"}]}} and wait for ACTIVE
3. UpdateTable {"keyspaceName":"ks1","tableName":"t1","addColumns":[{"name":"junk","type":"nosuchtype"}]} — emulator answers 200; AWS answers 404 ResourceNotFoundException ("UDT nosuchtype does not exist")
4. GetTable {"keyspaceName":"ks1","tableName":"t1"} — the emulator's schemaDefinition.allColumns now contains the column with type 'nosuchtype'
5. Repeat step 3 with a type that names a UDT created moments earlier and still CREATING: also accepted instead of refused
