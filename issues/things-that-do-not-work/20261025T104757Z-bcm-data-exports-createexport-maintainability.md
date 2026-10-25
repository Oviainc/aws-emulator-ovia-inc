Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 9ff0151e93167cf00d59909edc96bc1d4e3529be

# Collapse the duplicated table-resolution pass in validate_data_query and effective_table_configurations into one

_Filed: 2026-10-25T10:47:57Z_
_Operation: CreateExport | Class: maintainability_

> _Note: this issue was identified against the code base at commit `9ff0151e93167cf00d59909edc96bc1d4e3529be`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::query` resolves the tables of a submitted `DataQuery` and computes each table's effective properties TWICE, in two separate functions, and parses the caller's `QueryStatement` twice to do it. Both operations that accept an export definition (`CreateExport`, `UpdateExport`) call `validate_data_query(..)` and then `effective_table_configurations(..)` back to back, and each of those functions independently (a) parses the statement on its own sized-stack thread and (b) runs the same two-statement rule per table: `table_spec(table).ok_or_else(|| invalid_query_table(table))?` followed by `spec.effective_properties(requested.and_then(|configured| configured.get(table)))?`.

Because the rule lives in two places the copies can drift: a change to how a table name resolves to a catalog entry, or to how a requested property map is matched to a table, applied at only one of the two sites would make the validator and the storer disagree about the same definition — the validator accepting a definition the storer then refuses (or vice versa), from one request. The duplication is already acknowledged in prose rather than removed ("whose refusals it repeats rather than hides"), so nothing records that the two must be edited together.

The defect is the duplicated rule, not the responses: one function that reads the statement's shape ONCE and returns both the verdict and the effective `TableConfigurations` (with the extra tables the request configures on its own folded in, as today) keeps every refusal, every stored configuration and every existing test byte-identical, and halves the statement parses per create/update.

## Evidence

Copy 1 — services/service-com-amazonaws-bcmdataexports-awsbillingandcostmanagementdataexports/src/state/query.rs:485-502:
```
    let shape = query_shape(&query.query_statement)?;
    let requested = query.table_configurations.as_ref();
...
    for table in &shape.tables {
        let spec = table_spec(table).ok_or_else(|| invalid_query_table(table))?;
...
        let properties =
            spec.effective_properties(requested.and_then(|configured| configured.get(table)))?;
```

Copy 2 — same file, services/.../src/state/query.rs:548-563:
```
    let requested = query.table_configurations.as_ref();
...
    let mut tables: BTreeSet<String> = query_tables(&query.query_statement)?
...
    for table in &tables {
        let spec = table_spec(table).ok_or_else(|| invalid_query_table(table))?;
        let properties =
            spec.effective_properties(requested.and_then(|configured| configured.get(table)))?;
```

The repetition is admitted in the doc comment at services/.../src/state/query.rs:542-544: "Called AFTER [`validate_data_query`], whose refusals it repeats rather than hides: a table outside the catalog, a property the table does not publish, or a value outside the property's published values is the modelled 400 either way."

Both functions re-enter the parser: `query_tables` is defined at services/.../src/state/query.rs:254-256 as `Ok(query_shape(statement)?.tables)`, and `query_shape` spawns a dedicated thread per call — services/.../src/state/query.rs:314-318: `std::thread::scope(|scope| { let spawned = ::std::thread::Builder::new() .name(QUERY_PARSE_THREAD_NAME.to_string()) .stack_size(QUERY_PARSE_STACK_BYTES)` (`QUERY_PARSE_STACK_BYTES: usize = 16 * 1024 * 1024`, line 193).

Both call sites, in order, per request:
- services/.../src/ops/create_export.rs:217 `crate::state::validate_data_query(&input.export.data_query, false).map_err(` and :244 `crate::state::effective_table_configurations(&export.data_query).map_err(`
- services/.../src/ops/update_export.rs:219 `crate::state::validate_data_query(&input.export.data_query, true).map_err(` and :248 `crate::state::effective_table_configurations(&input.export.data_query).map_err(`

Existing tests that pin the behaviour a fix must leave unchanged: tests/all/data_query.rs, tests/all/query_column_selection.rs, tests/all/query_statement_subset.rs, tests/all/export_definition_validation.rs (the unpublished-table, unpublished-column, missing-TableConfigurations and effective-configuration round-trip rows), plus tests/all/export_lifecycle.rs and tests/all/state_roundtrip.rs for the stored definition a reader reports.

## Steps to Reproduce

1. Open services/service-com-amazonaws-bcmdataexports-awsbillingandcostmanagementdataexports/src/state/query.rs and compare lines 490-501 with lines 560-567: the same `table_spec(..).ok_or_else(|| invalid_query_table(..))?` + `spec.effective_properties(requested.and_then(|configured| configured.get(table)))?` pair appears in both loops.
2. Note that `validate_data_query` (line 485) and `effective_table_configurations` (line 552, via `query_tables` at line 254) each call `query_shape`, which spawns a 16 MiB-stack thread and re-parses the same `QueryStatement`.
3. Follow the two call sites: src/ops/create_export.rs:217 then :244, and src/ops/update_export.rs:219 then :248 — every CreateExport/UpdateExport runs both passes over one submitted definition.
4. Simulate a drift: change the table lookup rule at one site only (e.g. make the catalog match ignore case in `validate_data_query`'s loop) and observe that a definition now passes validation at line 491 and is then refused with `invalid_query_table` at line 561 — one request, two verdicts, from one rule kept in two places.
5. Verify the fix is response-neutral by running the suites above (tests/all/data_query.rs, query_column_selection.rs, query_statement_subset.rs, export_definition_validation.rs, export_lifecycle.rs, state_roundtrip.rs) after collapsing the two passes into one shape read.
