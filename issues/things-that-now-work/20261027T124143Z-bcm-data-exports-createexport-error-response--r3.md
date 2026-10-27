Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: ac318820ce0cc5f19770024ed132bf63c4007170

# Table-property refusals on CreateExport/UpdateExport name the field "TableProperties", a member neither request has

_Filed: 2026-10-27T12:41:43Z_
_Operation: CreateExport | Class: error-response_

> _Note: this issue was identified against the code base at commit `ac318820ce0cc5f19770024ed132bf63c4007170`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A submitted export definition whose `DataQuery.TableConfigurations` entry names a table property the table does not publish, or gives a property a value outside its published values, is refused through `TableSpec::effective_properties`, which is shared with `GetTable`. Both refusals it builds hard-code the `ValidationExceptionField$Name` as `"TableProperties"` — the *GetTable* input member. `CreateExportRequest` and `UpdateExportRequest` have no `TableProperties` member at all: the invalid entry sits at `Export.DataQuery.TableConfigurations`, and that is exactly the member path this crate already declares for refusals about that member (`TABLE_CONFIGURATIONS_FIELD`) and uses for the sibling refusal raised from the very same function. So one call to `resolve_table` reports two refusals about the same submitted member under two different field names, and the one a client is most likely to hit names a field that only exists on another operation's input — leaving a caller that reads `Fields[].Name` to repair its request pointed at a member it never sent. The GetTable path, where `"TableProperties"` is correct, must keep it.

## Evidence

Smithy, `ValidationExceptionField$Name`: "The field name where the invalid entry was detected." (smithy_query shape=ValidationExceptionField).

The crate's own name for that member on this path — services/.../src/state/query.rs:134-136:
```
/// The modelled input member a table-configuration refusal names
/// (`DataQuery$TableConfigurations`).
const TABLE_CONFIGURATIONS_FIELD: &str = "Export.DataQuery.TableConfigurations";
```
and it is used for the sibling refusal from the same function — query.rs:693-695:
```
fn missing_table_configurations(table: &str) -> ValidationException {
    field_validation_failed(
        TABLE_CONFIGURATIONS_FIELD,
```

The refusals that take the other name, raised from the same `resolve_table` — query.rs:566-567:
```
    let properties =
        spec.effective_properties(requested.and_then(|configured| configured.get(table)))?;
```
and services/.../src/state/catalog.rs:455-456:
```
    field_validation_failed(
        "TableProperties",
```
services/.../src/state/catalog.rs:473-474:
```
    field_validation_failed(
        "TableProperties",
```
Both are reached by `CreateExport` (src/ops/create_export.rs:225 `crate::state::resolve_data_query(&export.data_query, false)`) and by `UpdateExport` (src/ops/update_export.rs:220 `crate::state::resolve_data_query(&input.export.data_query, true)`), whose stated contract is "The modelled 400 that reports ONE named input member at fault" (src/state.rs:1202-1203, `field_validation_failed`).

Test coverage: the create/update path is exercised in tests/all/data_query_resolution.rs:268-293 (`unsupported_property_message`, `unsupported_value_message` rows) but the assertions at tests/all/data_query_resolution.rs:371-387 check only status, `__type` and `Message` — nothing pins `Fields[].Name` on this path. The GetTable path IS pinned to "TableProperties" (tests/all/table_catalog.rs:535-592, `field: "TableProperties"`), so that answer must stay unchanged.

## Steps to Reproduce

1. Call CreateExport with a valid definition whose `Export.DataQuery` is `{"QueryStatement": "SELECT * FROM COST_AND_USAGE_REPORT", "TableConfigurations": {"COST_AND_USAGE_REPORT": {"NOT_A_PROPERTY": "TRUE"}}}`.
2. Observe HTTP 400 ValidationException whose `Fields[0].Name` is `TableProperties` — a member `CreateExportRequest` does not declare — instead of `Export.DataQuery.TableConfigurations`.
3. Repeat with `{"COST_AND_USAGE_REPORT": {"TIME_GRANULARITY": "NOT_A_VALUE"}}`: same wrong field name, from catalog.rs:473-474.
4. Now omit the `TableConfigurations` entry entirely on UpdateExport for the same export: the refusal from the SAME function (`resolve_table`) names `Export.DataQuery.TableConfigurations` (query.rs:695) — the two refusals about one submitted member disagree.
5. Call GetTable with `{"TableName": "COST_AND_USAGE_REPORT", "TableProperties": {"NOT_A_PROPERTY": "TRUE"}}` and confirm `Fields[0].Name` is `TableProperties`, which is correct there and is pinned by tests/all/table_catalog.rs:535.
