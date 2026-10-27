Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 83df1cdc32a5bdf510e34eb14380f1c3f1e573c9

# Alias-addressed ingest creates a disassociated data stream and accepts nullValue even when the account's storage configuration disallows both

_Filed: 2026-10-27T14:55:57Z_
_Operation: BatchPutAssetPropertyValue | Class: validation_

> _Note: this issue was identified against the code base at commit `83df1cdc32a5bdf510e34eb14380f1c3f1e573c9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`PutStorageConfiguration` stores `disassociatedDataStorage` and `disallowIngestNullNaN`, `DescribeStorageConfiguration` reports them, and `set_storage_configuration` even enforces the "once enabled, cannot be disabled" rule over the first of them — but no ingestion path ever reads either setting, so both are inert settings:

1. With `disassociatedDataStorage = DISABLED`, an entry naming a `propertyAlias` no asset property and no stored stream answers to still CREATES a disassociated time series and ingests into it (batch_put_asset_property_value.rs:319-328), and the stream then shows up in DescribeTimeSeries/ListTimeSeries. The model says the service does not accept such time series at all in that state.
2. With `disallowIngestNullNaN = true`, a TQV whose variant is `nullValue` is still accepted: `reject_value` returns `None` for `nullValue` unconditionally, whatever the property's data type and whatever the account configured.

This is not behaviour the crate never attempts: the configuration is modelled, stored, defaulted (`DEFAULT_DISALLOW_INGEST_NULL_NAN`), validated as a whole (`check_storage_selection`), and a sibling handler already gates a request on it — `CreateBulkImportJob` reads `store.storage_configuration(now)` to refuse a job when neither the warm nor the cold tier is enabled. The ingest path is the one that skips the helper.

## Evidence

Model text that itself states the behaviour — smithy_query(query="shape", shape="PutStorageConfigurationRequest"), member `disassociatedDataStorage`:
  "ENABLED – IoT SiteWise accepts time series that aren't associated with asset properties." / "DISABLED – IoT SiteWise doesn't accept time series (data streams) that aren't associated with asset properties."
and member `disallowIngestNullNaN`:
  "Describes the configuration for ingesting NULL and NaN data. By default the feature is allowed. The feature is disallowed if the value is true."

Handler that does the opposite:
- services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/batch_put_asset_property_value.rs:313-328 — "// Alias-addressed ingestion: the stream the alias names is CREATED by / the first write that reaches it ..." then "if creates && !accepted.is_empty() { ... store.create_disassociated_time_series(alias, &data_type, now);" — no read of the storage configuration anywhere in the file (grep for `storage_configuration` in src/ops matches only create_bulk_import_job.rs and the two storage-configuration handlers).
- services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/batch_put_asset_property_value.rs:579-582, 615 — "A `nullValue` is the modelled way to report an absent reading, so it is accepted whatever the data type is." / "if set[0] == expected || set[0] == \"nullValue\" { None }".

Sibling that DOES consult the same helper:
- services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/create_bulk_import_job.rs:258-270 — "let configuration = store.storage_configuration(now);" ... "return Err(invalid_request(\"Before you create a bulk import job that ingests data into time series outside of a workspace, you must enable IoT SiteWise warm tier or IoT SiteWise cold tier.\"".
- services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/state.rs:7628-7652 — the crate already enforces one rule over this member ("The one member that does not go back is `disassociatedDataStorage` — \"After the disassociatedDataStorage is enabled, you can't disable it\""), so the setting is understood, just never applied to ingestion.

Tests: tests/all/storage_configuration.rs asserts only what the two storage-configuration operations report (it imports DEFAULT_DISALLOW_INGEST_NULL_NAN and asserts the echoed JSON); tests/all/time_series_ingestion.rs ingests by alias with no storage configuration set, so no test pins acceptance under DISABLED / true.

## Steps to Reproduce

1. PutStorageConfiguration with storageType=SITEWISE_DEFAULT_STORAGE and disassociatedDataStorage=DISABLED; DescribeStorageConfiguration confirms DISABLED.
2. BatchPutAssetPropertyValue with one entry carrying propertyAlias='/plant/line1/temp' (an alias no asset property owns) and one DOUBLE value.
3. Observe errorEntries == [] and DescribeTimeSeries(alias='/plant/line1/temp') now answering 200 with a new DISASSOCIATED stream, although the model says the service does not accept time series that aren't associated with asset properties in this state.
4. PutStorageConfiguration again with disallowIngestNullNaN=true, then BatchPutAssetPropertyValue a TQV whose variant is nullValue against an existing property: accepted with errorEntries == [] and readable through GetAssetPropertyValueHistory.
