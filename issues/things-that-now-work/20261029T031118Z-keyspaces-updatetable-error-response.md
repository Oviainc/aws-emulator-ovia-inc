Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 9a3acde976cfb747ba29bb78f67d8041a784763f

# UpdateTable interleaves the duplicate-column refusal and the unresolvable-type refusal per column, so the error depends on the order of addColumns entries

_Filed: 2026-10-29T03:11:18Z_
_Operation: UpdateTable | Class: error-response_

> _Note: this issue was identified against the code base at commit `9a3acde976cfb747ba29bb78f67d8041a784763f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateTable` walks `addColumns` once and applies BOTH per-column checks inside that single loop: the already-declared-name refusal (400 `ValidationException`) and the unresolvable-type refusal (404 `ResourceNotFoundException`). The first entry carrying either fault decides the whole request, so one and the same request content answers 400 or 404 depending only on the position of the two faulty entries in the list:

* `addColumns: [{"name":"pk","type":"text"}, {"name":"brand_new","type":"nosuchtype"}]` (duplicate first) -> 400;
* `addColumns: [{"name":"brand_new","type":"nosuchtype"}, {"name":"pk","type":"text"}]` (bad type first) -> 404.

Neither of the two sibling handlers that validate the same two things does that: `CreateTable` scans the WHOLE column list for duplicate names first (`crate::state::validate_schema`, before the state lock is even taken) and only then resolves the types, and `CreateType` likewise scans every field name for a duplicate before it resolves any field's type. For both of them the duplicate refusal always wins, whatever the list order. So the same pair of faults is graded three different ways across one crate, and position-dependently in one operation — against the rule this crate states explicitly, that "one request shape never gets two different answers depending on which sibling served it".

Nothing is stored on any of these paths, so the defect is purely in which modelled error the caller is told to fix first.

## Evidence

`UpdateTable` — one loop, both refusals interleaved, `services/service-com-amazonaws-keyspaces-keyspacesservice/src/ops/update_table.rs:581-614`:
```
                let mut declared: ::std::collections::BTreeSet<String> = schema
                    .all_columns
                    .iter()
                    .map(|column| crate::state::identifier_identity(&column.name))
                    .collect();
                for column in &added {
                    if !declared.insert(crate::state::identifier_identity(&column.name)) {
                        return Err(invalid(format!(
                            "addColumns names column '{}', which the table's schema already declares",
                            column.name
                        )));
                    }
                    ...
                    if let Some(referenced) = crate::state::unresolvable_type_reference(
                        &column.r#type,
                        &usable_type_names,
                    ) {
                        return Err(UpdateTableErrorModeled::ResourceNotFoundException(
                            crate::state::type_not_found(
                                &region,
                                &input.keyspace_name,
                                &referenced,
                            ),
                        )
                        .into());
                    }
                }
```

The siblings separate the two passes, duplicates first. `CreateTable` — `src/ops/create_table.rs:289` (whole-list duplicate scan, before the store lock at :339):
```
        crate::state::validate_schema(&input.schema_definition).map_err(invalid)?;
```
whose duplicate rule is `src/state.rs:2276-2284`:
```
    let mut declared: ::std::collections::BTreeSet<String> = ::std::collections::BTreeSet::new();
    for column in &schema.all_columns {
        if !declared.insert(identifier_identity(&column.name)) {
            return Err(format!(
                "schemaDefinition.allColumns declares column '{}' more than once",
                column.name
            ));
```
and only then the type scan, `create_table.rs:381-390`:
```
        for column in &input.schema_definition.all_columns {
            if let Some(referenced) =
                crate::state::unresolvable_type_reference(&column.r#type, &usable_type_names)
```
`CreateType` — the same split, `src/ops/create_type.rs:225-234` then `:303-312`:
```
        let mut declared: ::std::collections::BTreeSet<String> =
            ::std::collections::BTreeSet::new();
        for field in &input.field_definitions {
            if !declared.insert(crate::state::identifier_identity(&field.name)) {
                return Err(invalid(format!(
                    "fieldDefinitions declares field '{}' more than once",
                    field.name
                )));
```

The rule this breaks — `src/ops/create_table.rs:272-275`:
```
        // proportional to the cap, and grades this pair of faults in the order
        // `UpdateTable` and `CreateType` grade it, so one request shape never gets two
        // different answers depending on which sibling served it.
```

Test coverage: every duplicate-name row uses resolvable types and every unresolvable-type row uses fresh column names, so no test pins the interleaving and a whole-list duplicate pass first keeps them all green. `tests/all/column_name_identifier_identity.rs:340-364` (duplicates, types `text`/`int`, 400 asserted at `:377`), including the two-entry row `[{"name":"extra","type":"int"},{"name":"EXTRA","type":"text"}]`; `tests/all/update_table_add_columns_types.rs:155-215` and `:358` (unresolvable types on new column names, 404).

## Steps to Reproduce

1. CreateKeyspace {"keyspaceName":"ks"}
2. CreateTable {"keyspaceName":"ks","tableName":"tbl","schemaDefinition":{"allColumns":[{"name":"pk","type":"text"},{"name":"payload","type":"int"}],"partitionKeys":[{"name":"pk"}]}} and wait out the transient CREATING span
3. UpdateTable {"keyspaceName":"ks","tableName":"tbl","addColumns":[{"name":"payload","type":"text"},{"name":"brand_new","type":"nosuchtype"}]} -> 400 ValidationException ("addColumns names column 'payload', which the table's schema already declares")
4. Wait for the table to be ACTIVE again, then send the SAME two faulty entries in the opposite order: UpdateTable {"keyspaceName":"ks","tableName":"tbl","addColumns":[{"name":"brand_new","type":"nosuchtype"},{"name":"payload","type":"text"}]} -> 404 ResourceNotFoundException ("UDT nosuchtype does not exist")
5. Compare with the sibling: CreateTable {"keyspaceName":"ks","tableName":"other","schemaDefinition":{"allColumns":[{"name":"brand_new","type":"nosuchtype"},{"name":"pk","type":"text"},{"name":"pk","type":"text"}],"partitionKeys":[{"name":"pk"}]}} -> 400 ValidationException for the duplicate, whatever the order of the entries, because validate_schema scans the whole list first
