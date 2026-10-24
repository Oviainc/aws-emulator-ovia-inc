Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 1f773e23b3e0bd2b9ce8ba90d57212de869e61a4

# Validate the QueryStatement's column selections against the table dictionary

_Filed: 2026-10-24T08:44:58Z_
_Operation: CreateExport | Class: validation_

> _Note: this issue was identified against the code base at commit `1f773e23b3e0bd2b9ce8ba90d57212de869e61a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateExport validates a submitted `Export.DataQuery.QueryStatement` only down to the RELATIONS it names (parseable, single query, every `FROM`/join relation a published catalog table) and validates `TableConfigurations` property names/values. The COLUMNS the statement selects or filters on are never looked at, so a definition selecting a column the table does not publish is persisted and read back by GetExport/ListExports as a healthy export.

Expected (real service): a query statement may only select columns the Data Exports table dictionary publishes for the table it reads — the service has to build the export file's schema from the projection, so an unknown column cannot be delivered and CreateExport answers the modelled `ValidationException` (400) naming `Export.DataQuery.QueryStatement`. The same holds for a column the dictionary GATES on a table property that the request's `TableConfigurations` leaves at a default that excludes it (e.g. selecting a resource/split-cost-allocation column of `COST_AND_USAGE_REPORT` while `INCLUDE_RESOURCES` / `INCLUDE_SPLIT_COST_ALLOCATION_DATA` are `FALSE`): that column is not in the effective schema of the table as configured.

Actual (emulator): `SELECT not_a_real_column FROM COST_AND_USAGE_REPORT` and `SELECT line_item_resource_id FROM COST_AND_USAGE_REPORT` with `INCLUDE_RESOURCES=FALSE` both answer 200 with a minted ExportArn.

Secondary symptom of the same class: `DestinationConfigurations.S3Destination` is stored entirely unexamined — `S3Bucket` is never resolved against the S3 emulator, so an export whose destination bucket does not exist (or whose `S3BucketOwner` names an account that owns no such bucket) is accepted, where the real service refuses a destination it cannot write to with a `ValidationException`. The free-form `BILLING_VIEW_ARN` table-property value is likewise accepted at any string (catalog.rs publishes it with the `*` wildcard value set) rather than being required to name an existing billing view.

## Evidence

Spec: `CreateExport`'s modelled doc — "The QueryStatement is an SQL statement. Data Exports only supports a limited subset of the SQL syntax... To view the available tables and columns, see the Data Exports table dictionary" (quoted verbatim in src/ops/create_export.rs lines 52-64). The emulator already holds the full per-table, per-property column schema (`src/state/catalog.rs`: `ColumnSpec`, `TableSpec::schema(&properties)`, `column.requires` gating), and the crate's own test header states the intent: "every column AWS publishes and cannot select one AWS would reject" (tests/all/catalog_schema.rs line 15).

Handler: src/ops/create_export.rs lines 196-230 calls only `crate::state::validate_data_query(...)` and `effective_table_configurations(...)`. src/state/query.rs contains no projection/column handling at all (grep for `column|projection` matches only doc comments, lines 8/22/134/147); `validate_data_query` (query.rs line 338) checks relations and table-configuration entries only.

Tests: tests/all/export_definition_validation.rs covers unpublished TABLES, non-queries, bounds and CTEs — no case covers a column name, and no test asserts that an unpublished column is accepted. tests/all/export_lifecycle.rs only ever selects published columns or literals.

## Steps to Reproduce

1. POST / with X-Amz-Target: AWSBillingAndCostManagementDataExports.CreateExport and body {"Export":{"Name":"bogus-column","DataQuery":{"QueryStatement":"SELECT not_a_real_column FROM COST_AND_USAGE_REPORT"},"DestinationConfigurations":{"S3Destination":{"S3Bucket":"no-such-bucket","S3Prefix":"p","S3Region":"us-east-1","S3OutputConfigurations":{"OutputType":"CUSTOM","Format":"TEXT_OR_CSV","Compression":"GZIP","Overwrite":"OVERWRITE_REPORT"}}},"RefreshCadence":{"Frequency":"SYNCHRONOUS"}}}
2. Observe 200 with an ExportArn instead of a 400 ValidationException naming Export.DataQuery.QueryStatement
3. Repeat with "QueryStatement":"SELECT line_item_resource_id FROM COST_AND_USAGE_REPORT" and "TableConfigurations":{"COST_AND_USAGE_REPORT":{"INCLUDE_RESOURCES":"FALSE"}} — the column is gated off by that property (GetTable omits it from the schema for that configuration) yet the create still succeeds
4. GetExport the returned ARN: the unservable definition is reported back as a HEALTHY export
