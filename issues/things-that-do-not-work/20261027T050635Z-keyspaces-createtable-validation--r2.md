Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 53db56bf3158c7205a85c81c618ec0b3c4193a64

# Table column names are compared verbatim while UDT field names are compared as CQL identifiers, so one column can be declared twice and a key reference differing only in case is refused

_Filed: 2026-10-27T05:06:35Z_
_Operation: CreateTable | Class: validation_

> _Note: this issue was identified against the code base at commit `53db56bf3158c7205a85c81c618ec0b3c4193a64`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate states and implements an identifier-identity rule for the names inside a definition — `crate::state::identifier_identity` — and `CreateType` applies it to `fieldDefinitions`, so `Id` and `id` are ONE field name there. `crate::state::validate_schema`, the rule `CreateTable` runs over `schemaDefinition`, compares the very same kind of name (`ColumnDefinition$name` / `PartitionKey$name` / `ClusteringKey$name` / `StaticColumn$name`, all `GenericString`, exactly like `FieldDefinition$name`) as a RAW string. `UpdateTable`'s `addColumns` duplicate check carries the same raw comparison.

Two consequences, both from input the model allows:

1. `CreateTable` accepts `allColumns: [{"name":"id"},{"name":"ID"}]` and stores a schema with two columns that are one unquoted CQL identifier — precisely the shape `validate_schema`'s own doc says it exists to refuse ("nor one in which `allColumns` declares a name twice"), and precisely the shape `CreateType` refuses for a type's fields. `GetTable` then reports the two-column schema back, and `UpdateTable` will likewise accept `addColumns: [{"name":"Id"}]` onto a table that already declares `id`.

2. The role-agreement check turns a valid request into a 400: `allColumns: [{"name":"Id",...}]` with `partitionKeys: [{"name":"id"}]` is refused with "schemaDefinition.partitionKeys names column 'id', which schemaDefinition.allColumns does not declare", even though by this crate's own identifier rule `Id` and `id` are the same column.

Note the asymmetry is inside ONE shape: `ColumnDefinition$type` IS resolved as a CQL identifier (`referenced_type_names` case-folds unquoted identifiers, which the crate's own suite pins), while `ColumnDefinition$name` beside it is not.

## Evidence

The rule `validate_schema` claims to enforce — `services/service-com-amazonaws-keyspaces-keyspacesservice/src/state.rs:2220-2224`:
```
/// column — the storage engine has no row shape in which a column is two of
/// those, nor one in which `allColumns` declares a name twice. The model
/// expresses only that each list is non-empty, not that the lists agree.
```
What it actually compares, `src/state.rs:2235-2243` (raw `&str`, no identity normalisation):
```
    let mut declared: ::std::collections::BTreeSet<&str> = ::std::collections::BTreeSet::new();
    for column in &schema.all_columns {
        if !declared.insert(column.name.as_str()) {
            return Err(format!(
                "schemaDefinition.allColumns declares column '{}' more than once",
```
and the role lookup against that raw set, `src/state.rs:2263-2268`:
```
    for (member, name) in roles {
        if !declared.contains(name) {
            return Err(format!(
                "schemaDefinition.{member} names column '{name}', which schemaDefinition.allColumns does not declare"
```

The sibling that DOES apply the rule, `src/ops/create_type.rs:214-233`:
```
        // "A field definition consists out of a name and a type" (`FieldDefinition`), and
        // a type holds each of its fields once: there is no value of the type in which one
        // field name addresses two fields. Two names are the same field name under the
        // same identifier rule the type name follows, so `Id` and `id` collide while
        // `"Id"` is its own name. The model expresses only that the list is non-empty.
        ...
        for field in &input.field_definitions {
            if !declared.insert(crate::state::identifier_identity(&field.name)) {
```
The helper it calls, which this crate already owns, `src/state.rs:1385-1398`:
```
/// What makes two identifiers the same one, for a comparison that is not a lookup of a
/// stored name: an unquoted identifier is case-folded (Amazon Keyspaces "converts upper
/// case characters automatically into lower case characters"), a double-quoted one
/// stands for itself. So `Id` and `id` are one field name, and `"Id"` is another.
...
pub fn identifier_identity(supplied: &str) -> String {
```

The second raw copy, `src/ops/update_table.rs:579-590`:
```
                let mut declared: ::std::collections::BTreeSet<&str> = schema
                    .all_columns
                    .iter()
                    .map(|column| column.name.as_str())
                    .collect();
                for column in &added {
                    if !declared.insert(column.name.as_str()) {
                        return Err(invalid(format!(
                            "addColumns names column '{}', which the table's schema already declares",
```
where `CreateTable` calls the helper-less check: `src/ops/create_table.rs:265` (`crate::state::validate_schema(&input.schema_definition).map_err(invalid)?;`).

The crate's own statement that an unquoted CQL identifier is case-insensitive, applied to the `type` member sitting beside `name` in the same `ColumnDefinition` — `tests/all/column_types_and_auto_scaling.rs:69-70`: "the constructors are then exercised at each of their arities, nested, and in upper case — an unquoted CQL identifier is case-insensitive" (rows `"MAP<TEXT,INT>"`, `"Text"` at `tests/all/column_types_and_auto_scaling.rs:99-100`), and `tests/all/update_table_add_columns_types.rs:115`: "one row is spelled in upper case because an unquoted CQL identifier is case-insensitive".

Test coverage: the field-name side is pinned, the column-name side is not. `tests/all/type_lifecycle.rs:359-391` pins the refusal for fields:
```
    // A type holds each field name once, under the same identifier rule the type name
    // follows: `Id` and `id` are one field name, so the pair is refused.
    ...
        (
            "a field declared twice in different casing",
            json!([
                {"name": "Id", "type": "int"},
                {"name": "id", "type": "text"},
            ]),
        ),
    ] { assert_failure(case, ... 400, "ValidationException"); }
```
and `tests/all/type_lifecycle.rs:393-408` pins that a quoted spelling IS a second field. No suite supplies two column names differing only in case, nor a key whose casing differs from `allColumns`: `tests/all/static_columns_require_clustering_keys.rs` varies only the counts of columns/keys/statics, and grep for "allColumns declares" / "does not declare" over `tests/` finds no row at all.

## Steps to Reproduce

1. CreateKeyspace {"keyspaceName":"ident_ks"} and wait for ACTIVE
2. CreateTable {"keyspaceName":"ident_ks","tableName":"t1","schemaDefinition":{"allColumns":[{"name":"id","type":"int"},{"name":"ID","type":"text"}],"partitionKeys":[{"name":"id"}]}} — answers 200, and GetTable reports a schema with both `id` and `ID`, two columns that are one unquoted CQL identifier. The same pair as fieldDefinitions is refused 400 by CreateType (tests/all/type_lifecycle.rs:370)
3. CreateTable {"keyspaceName":"ident_ks","tableName":"t2","schemaDefinition":{"allColumns":[{"name":"Id","type":"int"}],"partitionKeys":[{"name":"id"}]}} — refused 400 "schemaDefinition.partitionKeys names column 'id', which schemaDefinition.allColumns does not declare", although `Id` and `id` are one identifier by crate::state::identifier_identity
4. CreateTable {"keyspaceName":"ident_ks","tableName":"t3","schemaDefinition":{"allColumns":[{"name":"id","type":"int"}],"partitionKeys":[{"name":"id"}]}}, wait for ACTIVE, then UpdateTable {"keyspaceName":"ident_ks","tableName":"t3","addColumns":[{"name":"ID","type":"text"}]} — answers 200 and appends a second column of the same identifier (src/ops/update_table.rs:585)
