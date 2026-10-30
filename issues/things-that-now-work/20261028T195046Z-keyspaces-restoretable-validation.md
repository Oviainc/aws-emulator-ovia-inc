Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: a67556f919df77bd586efc9ebe2e6b88c4f9f9e7

# RestoreTable copies a schema into the target keyspace without resolving its user-defined type references, so a restored table holds columns of types that keyspace does not have

_Filed: 2026-10-28T19:50:46Z_
_Operation: RestoreTable | Class: validation_

> _Note: this issue was identified against the code base at commit `a67556f919df77bd586efc9ebe2e6b88c4f9f9e7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every other handler of this crate that stores a column or field type resolves the type expression against the keyspace it is about to store it in, and refuses an unresolvable reference with the modelled `ResourceNotFoundException` (`CreateTable`, `CreateType`, `UpdateTable` all call `crate::state::unresolvable_type_reference` with `keyspace.usable_type_names()`). `RestoreTable` is the fourth operation that writes a schema into a keyspace — and the only one that writes a schema into a keyspace it did not come from — and it performs no such check: it clones the source table's revision (`source.restorable_settings_at(restore_point)`) and inserts `restored.schema_definition` verbatim into `target_keyspace.tables`.

Because `targetKeyspaceName` is an independent input (the crate's own test suite restores from `restore_src_ks` into `restore_tgt_ks`), a source table with a column whose type names a user-defined type of the SOURCE keyspace is restored into a target keyspace that holds no such type. The result is stored state the service could never have created, and the crate says so in its own words: `update_table.rs:598-600` — "refused here, so no schema ever holds a column of a type the service could not have created".

The stored table then contradicts the rest of the crate:
* `GetTable` on the restored table reports a column whose `type` names a type that does not exist in its keyspace, while `GetType`/`DeleteType` in the SOURCE keyspace derive `directReferringTables` only from tables of that keyspace — so the source type is reported as unused and `DeleteType` deletes it even though a restored table's column still names it.
* `UpdateTable {addColumns}` on the restored table, naming that very same type for another column, is refused 404 `ResourceNotFoundException` — the table may hold a column of a type that the same table cannot be given a column of.

The spec-required answer is the one the siblings already give: the type is a resource that "doesn't exist" in the keyspace the column is being stored in, which is the `ResourceNotFoundException` (HTTP 404) `RestoreTable` also models.

## Evidence

Smithy, `FieldDefinition$type` (smithy_query query="doc" shape="FieldDefinition" member="type") — the sentence this crate cites for a column too:
> "Any supported Cassandra data type, including collections and other user-defined types that are contained in the same keyspace."

Smithy, `ResourceNotFoundException` (declared on RestoreTable, HTTP 404):
> "The operation tried to access a keyspace, table, or type that doesn't exist. The resource might not be specified correctly, or its status might not be ACTIVE."

The sibling that enforces it on a create — `services/service-com-amazonaws-keyspaces-keyspacesservice/src/ops/create_table.rs:348-366`:
```
        // A column's type may name "other user-defined types that are contained in the
        // same keyspace" (`FieldDefinition$type`, the same rule for a column), so a name
        // in it that is neither a native type nor a type constructor must be a type of
        // THIS keyspace, in status ACTIVE; ...
        let usable_type_names = keyspace.usable_type_names();
        for column in &input.schema_definition.all_columns {
            if let Some(referenced) =
                crate::state::unresolvable_type_reference(&column.r#type, &usable_type_names)
            {
                return Err(CreateTableErrorModeled::ResourceNotFoundException(
                    crate::state::type_not_found(&region, &input.keyspace_name, &referenced),
                )
                .into());
            }
        }
```

The sibling that enforces it on an update, with the rule stated outright — `src/ops/update_table.rs:593-613`:
```
                    // A column added to an existing table declares a type under the same
                    // rule a created one does: ... refused here, so no
                    // schema ever holds a column of a type the service could not have
                    // created.
                    if let Some(referenced) = crate::state::unresolvable_type_reference(
                        &column.r#type,
                        &usable_type_names,
                    ) {
                        return Err(UpdateTableErrorModeled::ResourceNotFoundException(
```

What `RestoreTable` does instead — `src/ops/restore_table.rs:395-399` and `src/ops/restore_table.rs:557-566`:
```
        // What the restore reproduces. The schema and the TTL settings come from the
        // revision of the source table that was in effect at the selected timestamp; ...
        let restored = source.restorable_settings_at(restore_point);
```
```
        target_keyspace.tables.insert(
            input.target_table_name.clone(),
            crate::state::Table {
                keyspace_name: input.target_keyspace_name.clone(),
                ...
                schema_definition: restored.schema_definition,
```
`grep -n "usable_type_names|unresolvable_type_reference" src/` matches only `state.rs`, `ops/create_type.rs:302-305`, `ops/update_table.rs:367,601-603` and `ops/create_table.rs:356-359` — never `ops/restore_table.rs`.

Test coverage: the cross-keyspace restore is the suite's normal path — `tests/all/restore_table.rs:42` `const SOURCE_KEYSPACE: &str = "restore_src_ks";`, `:45` `const TARGET_KEYSPACE: &str = "restore_tgt_ks";`, `:113-119` `restore_request` always restores from the source keyspace into `TARGET_KEYSPACE` — but no case gives the source table a user-defined column type (`grep -i "type|udt" tests/all/restore_table.rs` finds only native types `text`/`timestamp`/`int`/`blob` and `encryptionSpecification.type`), and `tests/all/type_lifecycle.rs` never calls `RestoreTable`. So nothing pins the current behaviour.

## Steps to Reproduce

1. CreateKeyspace {"keyspaceName":"restore_src_ks"} and CreateKeyspace {"keyspaceName":"restore_tgt_ks"}; wait for both to report ACTIVE.
2. CreateType {"keyspaceName":"restore_src_ks","typeName":"address","fieldDefinitions":[{"name":"street","type":"text"}]}; wait for GetType to report ACTIVE.
3. CreateTable {"keyspaceName":"restore_src_ks","tableName":"src","schemaDefinition":{"allColumns":[{"name":"pk","type":"text"},{"name":"home","type":"frozen<address>"}],"partitionKeys":[{"name":"pk"}]},"pointInTimeRecovery":{"status":"ENABLED"}}; wait for ACTIVE. (The same request against `restore_tgt_ks` is refused 404 `ResourceNotFoundException` "UDT address does not exist", because that keyspace holds no type `address` — that is the refusal this restore skips.)
4. RestoreTable {"sourceKeyspaceName":"restore_src_ks","sourceTableName":"src","targetKeyspaceName":"restore_tgt_ks","targetTableName":"restored"} — answers 200 with a `restoredTableARN`; expected the modelled 404 `ResourceNotFoundException` for the type `address`, which `restore_tgt_ks` does not hold.
5. GetTable {"keyspaceName":"restore_tgt_ks","tableName":"restored"} — reports `schemaDefinition.allColumns[1].type == "frozen<address>"` in a keyspace where ListTypes returns an empty `types` list.
6. UpdateTable {"keyspaceName":"restore_tgt_ks","tableName":"restored","addColumns":[{"name":"work","type":"frozen<address>"}]} — refused 404 "UDT address does not exist", i.e. the stored table holds a column of a type the same table may not be given a column of.
7. DeleteType {"keyspaceName":"restore_src_ks","typeName":"address"} after deleting only `restore_src_ks.src` — succeeds, although `restore_tgt_ks.restored` still names the type in its schema.
