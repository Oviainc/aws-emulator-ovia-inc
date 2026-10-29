Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 9a3acde976cfb747ba29bb78f67d8041a784763f

# CreateType grades an unresolvable field type AFTER the name-already-taken conflict, the opposite order to CreateTable

_Filed: 2026-10-29T03:11:18Z_
_Operation: CreateType | Class: error-response_

> _Note: this issue was identified against the code base at commit `9a3acde976cfb747ba29bb78f67d8041a784763f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The two create operations of this crate resolve the SAME pair of faults — "the resource name is already taken" and "a supplied type expression names a user-defined type this keyspace cannot resolve" — in opposite orders, so one request shape gets two different wire answers depending on which sibling served it.

`CreateTable` runs the per-column type-resolution scan FIRST (404 `ResourceNotFoundException`, `UDT <name> does not exist`) and only afterwards refuses a table name that is already taken (409 `ConflictException`). `CreateType` does it the other way round: it refuses a type name already taken (409) and the per-keyspace type cap (402) first, and only then walks `fieldDefinitions` for an unresolvable type reference (404).

So: `CreateTable` on an existing table name whose schema also names a non-existent column type answers 404, while `CreateType` on an existing type name whose `fieldDefinitions` also name a non-existent field type answers 409. Both operations model both errors (OPERATIONS.md:37 and :43), and this crate states twice, as an explicit rule, that siblings must not grade one request shape differently. `CreateTable`'s own comment even asserts the alignment that does not exist ("exactly as `CreateType` checks a field's type").

Nothing is stored on either path, so the defect is purely in which modelled error a two-fault request receives — the same defect class as the already-fixed `CreateTable`/`UpdateTable` cap-vs-scan ordering issue (20261028T190449Z-keyspaces-createtable-error-response), and the fix direction chosen there (request-only refusals first, then store-dependent ones) is untouched by this one: both faults here are store-dependent.

## Evidence

`CreateTable` — the type-resolution scan, with the comment claiming it is staged as `CreateType` stages it, `services/service-com-amazonaws-keyspaces-keyspacesservice/src/ops/create_table.rs:372-390`:
```
        // A column's type may name "other user-defined types that are contained in the
        // same keyspace" (`FieldDefinition$type`, the same rule for a column), so a name
        // in it that is neither a native type nor a type constructor must be a type of
        // THIS keyspace, in status ACTIVE; any other is a type "that doesn't exist. The
        // resource might not be specified correctly, or its status might not be ACTIVE"
        // (`ResourceNotFoundException`), and nothing is stored. Checked after the keyspace
        // is resolved, because its own types are what a reference resolves against, and
        // exactly as `CreateType` checks a field's type and `UpdateTable` an added column.
        let usable_type_names = keyspace.usable_type_names();
        for column in &input.schema_definition.all_columns {
            if let Some(referenced) =
                crate::state::unresolvable_type_reference(&column.r#type, &usable_type_names)
            {
                return Err(CreateTableErrorModeled::ResourceNotFoundException(
                    crate::state::type_not_found(&region, &input.keyspace_name, &referenced),
                )
                .into());
```
and only AFTER it, the name-already-taken conflict — `create_table.rs:392-401`:
```
        // "Within a keyspace, table names must be unique."
        if keyspace.tables.contains_key(&input.table_name) {
            return Err(CreateTableErrorModeled::ConflictException(ConflictException {
                message: Some(format!(
                    "Table '{}.{}' already exists.",
                    input.keyspace_name, input.table_name
                )),
```

`CreateType` — the conflict first, `services/service-com-amazonaws-keyspaces-keyspacesservice/src/ops/create_type.rs:268-279`:
```
        // A type name is taken once per keyspace: creating "a resource that already
        // exists" is the `ConflictException` this operation models. Checked before the
        // cap below, because a name already taken adds no entry.
        if keyspace.types.contains_key(&type_name) {
            return Err(CreateTypeErrorModeled::ConflictException(ConflictException {
                message: Some(format!(
                    "Type '{}.{type_name}' already exists.",
                    input.keyspace_name
                )),
```
and the type-resolution scan LAST — `create_type.rs:297-312`:
```
        // A field's type may name "other user-defined types that are contained in the
        // same keyspace" (`FieldDefinition$type`) — so a name that is not a type of THIS
        // keyspace, or that names one which is not `ACTIVE` yet, is a type "that doesn't
        // exist. ... or its status might not be ACTIVE" (`ResourceNotFoundException`).
        let usable_type_names = keyspace.usable_type_names();
        for field in &input.field_definitions {
            if let Some(referenced) =
                crate::state::unresolvable_type_reference(&field.r#type, &usable_type_names)
            {
                return Err(CreateTypeErrorModeled::ResourceNotFoundException(
                    crate::state::type_not_found(&region, &input.keyspace_name, &referenced),
                )
```

The rule the divergence breaks, stated by this crate twice —
`create_table.rs:272-275`:
```
        // state lock is held. That keeps what a refused oversized request costs
        // proportional to the cap, and grades this pair of faults in the order
        // `UpdateTable` and `CreateType` grade it, so one request shape never gets two
        // different answers depending on which sibling served it.
```
`src/ops/tag_resource.rs:174-179`:
```
        // It is decided before the service-managed check below, which is request-only
        // too: of two faults in one request, the one that is wrong about the TAGS the
        // request carries is the one the caller has to fix first — the resource it
        // addresses would take no tag with that key either. `UntagResource` grades the
        // two in the same order, so one request shape never gets two different messages
        // depending on which sibling served it.
```

Test coverage: each fault is pinned alone, never together, so nothing constrains the fix. `tests/all/type_lifecycle.rs:226-240` pins 409 for a repeated `CreateType` whose `fieldDefinitions` are `[{"name": "other", "type": "int"}]` (a resolvable type); `tests/all/type_lifecycle.rs:685-697` pins 404 (`UDT {missing} does not exist`) for a NEW type name with an unresolvable field type; `tests/all/keyspace_table_lifecycle.rs:864-865` and `:984-1000` pin 409 for `CreateTable`/`CreateKeyspace` name conflicts with resolvable types; `tests/all/column_types_and_auto_scaling.rs:155-167` pins `CreateTable`'s 404 for an unknown column type on a fresh table name.

## Steps to Reproduce

1. CreateKeyspace {"keyspaceName":"ord_ks"}
2. CreateType {"keyspaceName":"ord_ks","typeName":"addr","fieldDefinitions":[{"name":"street","type":"text"}]} and wait out the transient CREATING span
3. CreateType again with the SAME name and an unresolvable field type: {"keyspaceName":"ord_ks","typeName":"addr","fieldDefinitions":[{"name":"f","type":"nosuchtype"}]} -> 409 ConflictException ("Type 'ord_ks.addr' already exists."): the duplicate check at create_type.rs:271 ran before the type scan at create_type.rs:303
4. Now the sibling, same request shape: CreateTable {"keyspaceName":"ord_ks","tableName":"tbl","schemaDefinition":{"allColumns":[{"name":"pk","type":"text"}],"partitionKeys":[{"name":"pk"}]}} and wait for it to settle
5. CreateTable again with the SAME table name and an unresolvable column type: {"keyspaceName":"ord_ks","tableName":"tbl","schemaDefinition":{"allColumns":[{"name":"pk","type":"text"},{"name":"c","type":"nosuchtype"}],"partitionKeys":[{"name":"pk"}]}} -> 404 ResourceNotFoundException ("UDT nosuchtype does not exist"): the type scan at create_table.rs:381 ran before the duplicate check at create_table.rs:393
6. Two faults of the same kinds, two different modelled errors, from two handlers of one crate
