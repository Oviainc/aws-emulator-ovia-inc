Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 1faf41e59ff01533a48e04d029136d23581e5c75

# Table-property refusal picks an arbitrary offender: effective_properties walks the caller's std HashMap, so two identical requests can get different ValidationExceptions

_Filed: 2026-10-28T21:25:12Z_
_Operation: GetTable | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `1faf41e59ff01533a48e04d029136d23581e5c75`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`TableSpec::effective_properties` decides a table-property refusal by iterating the caller-supplied property map and returning on the FIRST offending entry it meets. That map is a `std::collections::HashMap`, whose iteration order is neither the request order nor stable across map instances (std seeds every `RandomState` from a per-thread key that is bumped on each `RandomState::new()`), so when a request carries two or more offending entries for one table — two unpublished property keys, or an unpublished key plus a value outside an enumerated property's published values — WHICH offender the `ValidationException` names, and whether it is the unknown-property sentence or the invalid-value sentence, is arbitrary and can differ between two byte-identical requests in the same process run.

This crate has already decided the opposite rule for every other refusal it builds from a caller-supplied collection: `check_tag_keys_not_reserved` sorts and dedups the offenders precisely "so ... the text does not depend on iteration order", and `resolve_data_query` collects the configured-but-unqueried tables into a `BTreeSet` so that cross-TABLE resolution order is fixed. Only the walk over one table's properties is left on hash order, so the one refusal a caller has to act on is the one this crate does not make deterministic.

The reach is all three operations that accept a property map: `GetTable` (which hands `input.table_properties` straight in) and `CreateExport` / `UpdateExport` (through `resolve_data_query` -> `resolve_table`).

## Evidence

The non-deterministic walk — `services/.../src/state/catalog.rs:374-398`:

```rust
    pub fn effective_properties(
        &self,
        requested: Option<&HashMap<String, String>>,
        field: &str,
    ) -> Result<BTreeMap<String, String>, ValidationException> {
        ...
        for (name, value) in requested.into_iter().flatten() {
            let property = self
                .property(name)
                .ok_or_else(|| unknown_table_property(field, self, name))?;
            if !property.accepts(value) {
                return Err(invalid_table_property_value(field, property, value));
            }
```

The map really is a std `HashMap` — `services/.../src/ops/types/get_table_types.rs:18`:

```rust
    pub table_properties: Option<std::collections::HashMap<String, String>>,
```
(and `shared_types.rs:29`: `pub table_configurations: Option<std::collections::HashMap<String, std::collections::HashMap<String, String>>>,` for the create/update path).

The caller that hands it in unchanged — `services/.../src/ops/get_table.rs:174-179`:

```rust
        let properties = table
            .effective_properties(
                input.table_properties.as_ref(),
                crate::state::TABLE_PROPERTIES_FIELD,
            )
            .map_err(GetTableErrorModeled::ValidationException)?;
```

The rule this crate states for exactly this situation — `services/.../src/state.rs:1065-1067` and `:1083-1084`:

```
/// refuses that one namespace the same way and cannot drift apart. The offenders are
/// sorted and deduplicated, so a request naming one of them twice reports it once and the
/// text does not depend on iteration order, ...
```
```rust
    reserved.sort_unstable();
    reserved.dedup();
```

The sibling that does fix an order — `services/.../src/state/query.rs:530-536`:

```rust
    let queried: BTreeSet<&str> = shape.tables.iter().map(String::as_str).collect();
    let configured_only: BTreeSet<&str> = requested
```

Test coverage: no existing test pins the multi-offender case, so nothing in the suite depends on the current arbitrary pick. `tests/all/data_query_resolution.rs:264-289` submits exactly one offending property per row (`{ "COST_AND_USAGE_REPORT": { "NOT_A_TABLE_PROPERTY": "TRUE" } }`, then `{ "TIME_GRANULARITY": "WEEKLY" }`), and every row of `tests/all/table_property_refusal_field.rs:96-130` builds a one-entry map (`properties: json!({ UNPUBLISHED_PROPERTY: "TRUE" })`).

## Steps to Reproduce

1. POST / with `X-Amz-Target: AWSBillingAndCostManagementDataExports.GetTable` and body `{"TableName":"COST_AND_USAGE_REPORT","TableProperties":{"NOT_A_TABLE_PROPERTY_A":"TRUE","NOT_A_TABLE_PROPERTY_B":"TRUE"}}`
2. Record the refusal's `Message` — it names exactly one of the two keys (`Table property NOT_A_TABLE_PROPERTY_A is not supported ...` or `... NOT_A_TABLE_PROPERTY_B ...`)
3. Repeat the identical request several times (and across process restarts): the key named changes, because the offender is chosen by `HashMap` iteration order rather than by any rule of the request
4. Same with a mixed pair, so the SENTENCE also flips: `{"TableName":"COST_AND_USAGE_REPORT","TableProperties":{"NOT_A_TABLE_PROPERTY":"TRUE","TIME_GRANULARITY":"WEEKLY"}}` answers either `Table property NOT_A_TABLE_PROPERTY is not supported for table COST_AND_USAGE_REPORT. ...` or `Value WEEKLY is not supported for table property TIME_GRANULARITY. ...`
5. The same flip reaches the export-definition path: `CreateExport` with `Export.DataQuery.TableConfigurations = {"COST_AND_USAGE_REPORT":{"NOT_A_TABLE_PROPERTY":"TRUE","TIME_GRANULARITY":"WEEKLY"}}` (and the same body through `UpdateExport`)
6. Contrast a reserved-tag-key refusal, which is deterministic for the same class of input: `TagResource` with `ResourceTags:[{"Key":"aws:b",...},{"Key":"aws:a",...}]` always quotes the offenders in sorted order (state.rs:1083-1084)
