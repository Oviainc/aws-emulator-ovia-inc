Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 15aaa5471fc3f7278352228b97d2216e65bea584

# An unpublished table named only in DataQuery.TableConfigurations is refused naming Export.DataQuery.QueryStatement, a member that carried no invalid entry

_Filed: 2026-10-28T14:47:07Z_
_Operation: CreateExport | Class: error-response_

> _Note: this issue was identified against the code base at commit `15aaa5471fc3f7278352228b97d2216e65bea584`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`resolve_data_query` resolves two sets of tables: the ones the `QueryStatement` names, and (second loop) the ones the request configures in `DataQuery.TableConfigurations` but the statement does NOT name. Both loops go through the same `resolve_table`, whose first step is `table_spec(table).ok_or_else(|| invalid_query_table(table))?`. `invalid_query_table` hard-codes the refused member as `QUERY_STATEMENT_FIELD` = `"Export.DataQuery.QueryStatement"`.

So a request whose `QueryStatement` is entirely valid but whose `TableConfigurations` carries a key the catalog does not publish is answered with `ValidationExceptionField.Name = "Export.DataQuery.QueryStatement"` and the sentence `Table name <key> is invalid`, plus the requirement `Must name a table data exports publishes; call ListTables for them.` — pointing a caller that reads `Fields[].Name` at the one member of its request that was NOT at fault. The invalid entry is a key of `Export.DataQuery.TableConfigurations`.

The crate already knows the right path on exactly this code path: the property refusals raised from the SAME `resolve_table` call are deliberately threaded `TABLE_CONFIGURATIONS_FIELD`, and the sibling `missing_table_configurations` refusal names it too. Only the unknown-table refusal is stuck on the statement's member, and `invalid_query_table`'s own doc comment says it is "for a `QueryStatement` naming a table outside the catalog" — which is precisely not the case on the configured-only loop. Reached by both `CreateExport` (create_export.rs:225) and `UpdateExport` (update_export.rs:220).

## Evidence

Smithy, `ValidationExceptionField$Name`: "The field name where the invalid entry was detected." (smithy_query query="shape" shape="ValidationExceptionField").

The loop that resolves a table the statement never names — services/service-com-amazonaws-bcmdataexports-awsbillingandcostmanagementdataexports/src/state/query.rs:527-540:
```
    // A table the request configures but the statement does not name is still the
    // caller's, so its table and its properties are resolved — and therefore checked and
    // reported — exactly as a queried table's are.
    ...
    for table in configured_only {
        let (_, properties) = resolve_table(table, requested, require_table_configurations)?;
```

The refusal that loop raises for a name the catalog does not publish — query.rs:558:
```
    let spec = table_spec(table).ok_or_else(|| invalid_query_table(table))?;
```
and query.rs:664-676, whose own doc states the rule the call above breaks:
```
/// The modelled 400 for a `QueryStatement` naming a table outside the catalog.
...
fn invalid_query_table(table: &str) -> ValidationException {
    field_validation_failed(
        QUERY_STATEMENT_FIELD,
        format!("Table name {} is invalid", clip_for_message(table)),
        "Must name a table data exports publishes; call ListTables for them.".to_string(),
    )
}
```
with query.rs:130-136:
```
/// The modelled input member a query refusal names (`DataQuery$QueryStatement`, reached
/// through `Export$DataQuery`).
const QUERY_STATEMENT_FIELD: &str = "Export.DataQuery.QueryStatement";

/// The modelled input member a table-configuration refusal names
/// (`DataQuery$TableConfigurations`).
const TABLE_CONFIGURATIONS_FIELD: &str = "Export.DataQuery.TableConfigurations";
```

The sibling refusals from the SAME function already use the other constant — query.rs:566-572:
```
    // The refusals a property map earns name the member the SUBMITTED definition carried
    // it at, which on this path is `Export.DataQuery.TableConfigurations` and not the
    // member `GetTable` reads one at.
    let properties = spec.effective_properties(
        requested.and_then(|configured| configured.get(table)),
        TABLE_CONFIGURATIONS_FIELD,
    )?;
```
and query.rs:698-701 (`missing_table_configurations` -> `field_validation_failed(TABLE_CONFIGURATIONS_FIELD, ...)`).

Test coverage: the rule is already stated in tests/all/table_property_refusal_field.rs:6-14 — "`ValidationExceptionField$Name` is \"[t]he field name where the invalid entry was detected\" ... so a refusal names the member path of the input THE REQUEST SENT" — but every row there submits its configuration for a table the statement names (tests/all/table_property_refusal_field.rs:216-217 `let configurations = json!({ row.table: row.properties.clone() }); let statement = format!("SELECT * FROM {}", row.table);`). The configured-only loop is exercised only with a PUBLISHED table name, tests/all/data_query.rs:270-324 (`a_configured_table_the_query_does_not_read_is_reported_too`), so nothing pins the field name for an unpublished one.

## Steps to Reproduce

1. Call CreateExport with `{"Export": {"Name": "cfg_only_bad_table", "DataQuery": {"QueryStatement": "SELECT line_item_usage_amount FROM COST_AND_USAGE_REPORT", "TableConfigurations": {"NOT_A_REAL_TABLE": {}}}, "DestinationConfigurations": {...}, "RefreshCadence": {"Frequency": "SYNCHRONOUS"}}}` — a QueryStatement that is valid against the published catalog, plus one TableConfigurations key the catalog does not publish.
2. Observe HTTP 400 ValidationException whose `Fields[0].Name` is `Export.DataQuery.QueryStatement` and whose `Fields[0].Message` is `Must name a table data exports publishes; call ListTables for them.` — pointing at the statement, which named only published tables.
3. Expected: `Fields[0].Name` = `Export.DataQuery.TableConfigurations`, the member whose key was the invalid entry (the constant the crate already declares at query.rs:136).
4. Repeat the same request against UpdateExport (which reaches the same `resolve_data_query`) and confirm the same wrong member path.
5. Contrast with a bad property KEY on the same path — `{"COST_AND_USAGE_REPORT": {"NOT_A_PROPERTY": "TRUE"}}` — which correctly names `Export.DataQuery.TableConfigurations`: one function reports two refusals about the same submitted member under two different member paths.
