Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 8c118d1d43e9fad6226f410894186130d2b1ac53

# INCLUDE_IAM_PRINCIPAL_DATA gates no column, so GetTable reports the same schema whether it is TRUE or FALSE

_Filed: 2026-10-25T18:56:16Z_
_Operation: GetTable | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `8c118d1d43e9fad6226f410894186130d2b1ac53`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The catalog publishes `INCLUDE_IAM_PRINCIPAL_DATA` for `COST_AND_USAGE_REPORT` with the description "Whether IAM principal columns should be included or not", but NO `ColumnSpec` in the crate is gated on that property: `grep INCLUDE_IAM_PRINCIPAL_DATA` over the crate finds only the constant, the `TablePropertySpec` entry, the `pub use` re-export and two tests — never a `col_when(..., PROPERTY_INCLUDE_IAM_PRINCIPAL_DATA, ...)`.

So `GetTable` with `TableProperties: {"INCLUDE_IAM_PRINCIPAL_DATA": "TRUE"}` returns byte-identical `Schema` to a request that sets it to `FALSE` or omits it — the property is advertised as changing the column set and changes nothing — and `CreateExport`/`UpdateExport`, which validate a submitted `QueryStatement`'s column references against `columns_in_force` (the same filter `GetTable` reports through), refuse every IAM-principal column name at either value of the property.

The catalog's own classifying comment accounts for every OTHER property of this table (which ones change the DATA, which ones gate columns, which one changes granularity) and silently omits this one, so nothing in the crate states that it is meant to gate nothing; the two sibling boolean properties of the same table are each wired with `col_when`.

## Evidence

The property, with the description that claims columns:
- `src/state/catalog.rs:528-535` — `TablePropertySpec { name: PROPERTY_INCLUDE_IAM_PRINCIPAL_DATA, valid_values: BOOLEAN_PROPERTY_VALUES, published_default: PropertyDefault::Unset, description: "Whether IAM principal columns should be included or not.", }`
- `src/state/catalog.rs:112-113` — `/// Table property: whether the table carries the IAM principal columns.` / `pub const PROPERTY_INCLUDE_IAM_PRINCIPAL_DATA: &str = "INCLUDE_IAM_PRINCIPAL_DATA";`

The comment that classifies every other property of the same list and omits this one:
- `src/state/catalog.rs:500-506` — "Not every property gates a column: INCLUDE_CAPACITY_RESERVATION_DATA, INCLUDE_MANUAL_DISCOUNT_COMPATIBILITY and BILLING_VIEW_ARN change the DATA the table reports … rather than the set of columns … INCLUDE_RESOURCES and INCLUDE_SPLIT_COST_ALLOCATION_DATA do gate columns, and TIME_GRANULARITY changes the granularity of the rows."

Siblings that ARE wired:
- `src/state/catalog.rs:879-885` — `col_when("line_item_resource_id", STRING, …, PROPERTY_INCLUDE_RESOURCES, PROPERTY_TRUE,)`
- `src/state/catalog.rs:1325-1401` — eleven `col_when(..., PROPERTY_INCLUDE_SPLIT_COST_ALLOCATION_DATA, PROPERTY_TRUE,)` entries

The filter that therefore never adds anything for it:
- `src/state/catalog.rs:405-410` (`columns_in_force`) — `self.columns().filter(move |column| match column.requires { None => true, Some((property, value)) => properties.get(property).map(String::as_str) == Some(value) })`, read by `GetTable` through `src/ops/get_table.rs:171-174` (`table.effective_properties(...)` then `table.schema(&properties)`).

Test coverage: `tests/all/table_catalog.rs:400-410` pins `GATED_COLUMNS` for `INCLUDE_RESOURCES` and `INCLUDE_SPLIT_COST_ALLOCATION_DATA` only; `tests/all/catalog_dictionary.rs:240-241` pins only the property's valid values and absent default ("Published with no default: in force only for a request that names it."). No test states that this property gates no column.

## Steps to Reproduce

1. POST / with X-Amz-Target: AWSBillingAndCostManagementDataExports.GetTable and body {"TableName":"COST_AND_USAGE_REPORT"} — record the returned Schema.
2. Repeat with {"TableName":"COST_AND_USAGE_REPORT","TableProperties":{"INCLUDE_IAM_PRINCIPAL_DATA":"TRUE"}} — the Schema is identical, although the response's own TablePropertyDescription for that property says "Whether IAM principal columns should be included or not".
3. grep -n INCLUDE_IAM_PRINCIPAL_DATA over the crate: no `col_when(... PROPERTY_INCLUDE_IAM_PRINCIPAL_DATA ...)` exists, so `columns_in_force` can never add a column for it, and a CreateExport whose QueryStatement selects an IAM-principal column is refused with "Column name <name> is invalid" at either value of the property.
