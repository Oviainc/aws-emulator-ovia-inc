Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 1f773e23b3e0bd2b9ce8ba90d57212de869e61a4

# Validate the replacement definition's column selections and destination on UpdateExport

_Filed: 2026-10-24T08:44:58Z_
_Operation: UpdateExport | Class: validation_

> _Note: this issue was identified against the code base at commit `1f773e23b3e0bd2b9ce8ba90d57212de869e61a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

UpdateExport "overwrit[es] all export parameters", so the replacement definition must be judged exactly as a create's is. It validates the statement's relations and demands a restated `TableConfigurations` entry per table, but — like CreateExport — never checks the COLUMNS the statement selects or filters on against the table dictionary's published schema for the configuration the request states, and never examines `DestinationConfigurations.S3Destination` (bucket existence/ownership) before overwriting the stored definition.

Expected: a replacement query naming a column the table does not publish (or a column the dictionary gates on a table property the request sets to a value that excludes it) is refused with the modelled `ValidationException` (400) naming `Export.DataQuery.QueryStatement`, and the previously stored, valid definition is left untouched.

Actual: the handler answers 200, replaces the stored definition with the unservable one, bumps `ExportStatus.LastUpdatedAt` and schedules a delivery of it — so a subsequent GetExport reports a definition the service could never deliver, and the caller has silently destroyed a working definition.

## Evidence

Spec: `UpdateExport` — "Updates an existing data export by overwriting all export parameters. All export parameters must be specified in the UpdateExport request" (quoted in src/ops/update_export.rs lines 203-209); the query subset/table-dictionary rule is the same one CreateExport's doc states ("To view the available tables and columns, see the Data Exports table dictionary").

Handler: src/ops/update_export.rs lines 210-262 calls only `validate_data_query(..., true)` and `effective_table_configurations(...)`, then assigns `stored.export.data_query` / `destination_configurations` / `refresh_cadence` wholesale. src/state/query.rs has no column-level validation (no `column`/`projection` logic; only the relation walk and table-property checks).

Tests: tests/all/export_definition_validation.rs covers the UpdateExport restatement rule and unpublished TABLES; no case covers a column name on update, so nothing asserts the current acceptance.

## Steps to Reproduce

1. CreateExport a valid export (e.g. QueryStatement "SELECT identity_line_item_id FROM COST_AND_USAGE_REPORT")
2. Call UpdateExport with the returned ExportArn, the same Export.Name, and QueryStatement "SELECT not_a_real_column FROM COST_AND_USAGE_REPORT" (restating TableConfigurations as GetExport reports them)
3. Observe 200 with the ExportArn instead of a 400 ValidationException
4. GetExport the ARN: the stored definition is now the unservable one and LastUpdatedAt has moved
