Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 2a22026c0f737b6187455fb636429eae77d015a7

# UpdateExport accepts a replacement destination configuration it never validates

_Filed: 2026-10-24T16:42:38Z_
_Operation: UpdateExport | Class: validation_

> _Note: this issue was identified against the code base at commit `2a22026c0f737b6187455fb636429eae77d015a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateExport` "[updates] an existing data export by overwriting all export parameters" and the handler re-judges the replacement definition only along two axes: the `Export.Name` must match the stored name, and the `DataQuery` must read against the table catalog. The replacement `DestinationConfigurations` is written to the store unchecked (`stored.export.destination_configurations = input.export.destination_configurations`).

Expected (real AWS): the replacement destination is validated exactly as a create's is — an unsupported `S3OutputConfigurations` `Format`/`Compression` pair (e.g. `PARQUET` + `GZIP`), a bogus `S3Region`, or a bucket the service cannot write is refused with a 400 `ValidationException` and the stored export is left untouched.

Actual (emulator): the call returns 200, the export now carries a destination configuration no real export could have, and `GetExport` reports it.

## Evidence

Spec: `UpdateExport` takes `Export` `@required` (`smithy_query(query="operation",operation="UpdateExport")`), whose `DestinationConfigurations` is `@required` with the enum-typed `S3OutputConfigurations`; the operation doc states every parameter is overwritten, so the replacement is subject to the same create-time destination rules (supported Format/Compression pairings for AWS Data Exports).
Handler: `src/ops/update_export.rs` lines ~192-269 — name check, `validate_data_query`, `effective_table_configurations`, then straight assignment of `destination_configurations` and `refresh_cadence`.
Tests: no test in `tests/all/` references S3/destination validation (`grep -n 'Destination|S3|bucket' tests/all/export_definition_validation.rs` → no matches).

## Steps to Reproduce

1. CreateExport with a valid definition (Format TEXT_OR_CSV / Compression GZIP) and note the ExportArn
2. POST UpdateExport for that ARN with the same Export.Name but S3OutputConfigurations {"OutputType":"CUSTOM","Format":"PARQUET","Compression":"GZIP","Overwrite":"OVERWRITE_REPORT"}
3. Observe HTTP 200 (real AWS answers HTTP 400 ValidationException and leaves the export unchanged)
4. Call GetExport and observe the unsupported destination configuration now persisted
